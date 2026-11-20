//! Ingestion of SQLite's stored definitions.

use diesel::{
    prelude::*,
    sql_types::{BigInt, Nullable, Text},
};
use sql_traits::prelude::*;
use sqlparser::{
    ast::{Ident, Statement},
    dialect::SQLiteDialect,
    parser::{Parser, ParserError},
};

use crate::{
    DieseliteBuildError,
    models::{AttachedDatabase, TableColumn, TableListEntry},
    schema::{pragma_database_list, pragma_table_list, pragma_table_xinfo},
};

diesel::define_sql_function! { fn sqlite_version() -> Text; }

/// Builds a parsed database from a live SQLite connection.
#[derive(Default)]
pub struct DieseliteDatabaseBuilder<'connection> {
    connection: Option<&'connection mut SqliteConnection>,
}

impl<'connection> DieseliteDatabaseBuilder<'connection> {
    /// Supplies the connection whose catalogs are read.
    #[must_use]
    pub fn connection(mut self, connection: &'connection mut SqliteConnection) -> Self {
        self.connection = Some(connection);
        self
    }
}

impl TryFrom<DieseliteDatabaseBuilder<'_>> for ParserDB {
    type Error = DieseliteBuildError;

    /// Reads stored definitions and checks the resulting model against SQLite.
    ///
    /// # Errors
    ///
    /// Returns an error when the connection, catalog, definition, or model
    /// cannot be read or disagrees.
    fn try_from(builder: DieseliteDatabaseBuilder<'_>) -> Result<Self, Self::Error> {
        let connection = builder.connection.ok_or(DieseliteBuildError::MissingConnection)?;
        let version: String = diesel::select(sqlite_version()).get_result(connection)?;
        let numbers = version.split('.').map(str::parse::<u32>).collect::<Result<Vec<_>, _>>();
        if !matches!(numbers.as_deref(), Ok([major, minor, patch]) if (*major, *minor, *patch) >= (3, 37, 0))
        {
            return Err(DieseliteBuildError::UnsupportedSqliteVersion { found: version });
        }
        let databases = pragma_database_list::table
            .select(AttachedDatabase::as_select())
            .order(pragma_database_list::seq)
            .load::<AttachedDatabase>(connection)?;
        let relations = pragma_table_list::table
            .select(TableListEntry::as_select())
            .load::<TableListEntry>(connection)?;
        let mut input = ParseOptions::default()
            .with_identifier_case(IdentifierCase::Folded)
            .ingestor::<SQLiteDialect>("main".to_owned());
        for database in
            databases.iter().filter(|database| database.name != "main" && database.name != "temp")
        {
            let sql = format!(
                "ATTACH DATABASE '{}' AS \"{}\"",
                database.file.replace('\'', "''"),
                database.name.replace('"', "\"\"")
            );
            input = input
                .apply_statement(parse_one(&database.name, &database.name, &sql)?)
                .map_err(|source| ingestion(&database.name, &database.name, source))?;
        }
        for database in databases
            .iter()
            .filter(|database| database.name == "main")
            .chain(
                databases
                    .iter()
                    .filter(|database| database.name != "main" && database.name != "temp"),
            )
            .chain(databases.iter().filter(|database| database.name == "temp"))
        {
            let catalog =
                diesel_dynamic_schema::schema(database.name.as_str()).table("sqlite_schema");
            let definitions = catalog
                .select((
                    catalog.column::<Text, _>("type"),
                    catalog.column::<Text, _>("name"),
                    catalog.column::<Nullable<Text>, _>("sql"),
                ))
                .order(catalog.column::<BigInt, _>("rowid"))
                .load::<(String, String, Option<String>)>(connection)?;
            for (_object_type, name, sql) in definitions {
                let Some(sql) = sql.filter(|_| !internal(&name)) else { continue };
                let mut statement = parse_one(&database.name, &name, &sql)?;
                qualify(&mut statement, &database.name);
                input = if matches!(statement, Statement::CreateVirtualTable { .. }) {
                    let columns = pragma_table_xinfo::table
                        .filter(pragma_table_xinfo::arg.eq(&name))
                        .filter(pragma_table_xinfo::schema.eq(&database.name))
                        .order(pragma_table_xinfo::cid)
                        .select(TableColumn::as_select())
                        .load::<TableColumn>(connection)?;
                    let suffixes = relations
                        .iter()
                        .filter(|relation| {
                            relation.schema == database.name && relation.table_type == "shadow"
                        })
                        .filter_map(|relation| relation.name.rsplit_once('_'))
                        .filter(|(prefix, _)| prefix.eq_ignore_ascii_case(&name))
                        .map(|(_, suffix)| suffix);
                    input.apply_virtual_table(
                        statement,
                        DeclaredVirtualTable::new(
                            columns.into_iter().map(|column| {
                                DeclaredColumn::new(
                                    column.name,
                                    column.declared_type,
                                    column.hidden == 1,
                                )
                            }),
                            suffixes,
                        ),
                    )
                } else {
                    input.apply_statement(statement)
                }
                .map_err(|source| ingestion(&database.name, &name, source))?;
            }
        }
        let database = input.finish();
        let catalogs = crate::cross_check::load(connection, relations)?;
        crate::cross_check::check(&database, &catalogs)?;
        Ok(database)
    }
}

fn ingestion(database: &str, name: &str, source: sql_traits::errors::Error) -> DieseliteBuildError {
    DieseliteBuildError::Ingestion {
        database: database.to_owned(),
        name: name.to_owned(),
        source: Box::new(source),
    }
}

pub(crate) fn internal(name: &str) -> bool {
    name.get(..7).is_some_and(|prefix| prefix.eq_ignore_ascii_case("sqlite_"))
}

fn parse_one(database: &str, name: &str, sql: &str) -> Result<Statement, DieseliteBuildError> {
    let failure = |source| {
        DieseliteBuildError::UnparsableDefinition {
            database: database.to_owned(),
            name: name.to_owned(),
            sql: sql.to_owned(),
            source,
        }
    };
    let mut statements = Parser::parse_sql(&SQLiteDialect {}, sql).map_err(failure)?;
    if statements.len() != 1 {
        return Err(failure(ParserError::ParserError(
            "expected exactly one stored statement".to_owned(),
        )));
    }
    statements
        .pop()
        .ok_or_else(|| failure(ParserError::ParserError("missing stored statement".to_owned())))
}

fn qualify(statement: &mut Statement, database: &str) {
    let name = match statement {
        Statement::CreateTable(table) => &mut table.name,
        Statement::CreateIndex(index) => {
            let Some(name) = &mut index.name else { return };
            name
        }
        Statement::CreateView(view) => &mut view.name,
        Statement::CreateTrigger(trigger) => &mut trigger.name,
        Statement::CreateVirtualTable { name, .. } => name,
        _ => return,
    };
    name.0.insert(0, sqlparser::ast::ObjectNamePart::Identifier(Ident::with_quote('"', database)));
}
