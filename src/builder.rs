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
        check_version(diesel::select(sqlite_version()).get_result(connection)?)?;
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
            input = applied(input.apply_sql(&sql), &database.name, &database.name, &sql)?;
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
                input = ingest(input, connection, &relations, &database.name, &name, &sql)?;
            }
        }
        let database = input.finish();
        let catalogs = crate::cross_check::load(connection, relations)?;
        crate::cross_check::check(&database, &catalogs)?;
        Ok(database)
    }
}

/// Refuses a SQLite older than 3.37.0, the first release with
/// `pragma_table_list`.
fn check_version(version: String) -> Result<(), DieseliteBuildError> {
    let numbers = version.split('.').map(str::parse::<u32>).collect::<Result<Vec<_>, _>>();
    if matches!(numbers.as_deref(), Ok([major, minor, patch]) if (*major, *minor, *patch) >= (3, 37, 0))
    {
        Ok(())
    } else {
        Err(DieseliteBuildError::UnsupportedSqliteVersion { found: version })
    }
}

/// Applies the stored definition `sql` of `name` to `input` under
/// `database`, which SQLite stores unqualified. A virtual table goes in with
/// the columns and shadow tables `connection` reports for it.
fn ingest(
    input: ParserDBIngestor,
    connection: &mut SqliteConnection,
    relations: &[TableListEntry],
    database: &str,
    name: &str,
    sql: &str,
) -> Result<ParserDBIngestor, DieseliteBuildError> {
    let virtual_table = relations.iter().any(|relation| {
        relation.table_type == "virtual"
            && relation.schema == database
            && relation.name.eq_ignore_ascii_case(name)
    });
    let result = if virtual_table {
        let mut statement = parse_one(database, name, sql)?;
        if let Statement::CreateVirtualTable { name: created, .. } = &mut statement {
            created.0.insert(
                0,
                sqlparser::ast::ObjectNamePart::Identifier(Ident::with_quote('"', database)),
            );
        }
        let declared = declared_virtual_table(connection, relations, database, name)?;
        input.apply_virtual_table(statement, declared)
    } else {
        input.apply_sql_in(&Ident::with_quote('"', database), sql)
    };
    applied(result, database, name, sql)
}

/// Names the object `name` of `database` in an ingestion failure, and reports
/// its definition `sql` when it does not parse.
fn applied(
    result: Result<ParserDBIngestor, sql_traits::errors::Error>,
    database: &str,
    name: &str,
    sql: &str,
) -> Result<ParserDBIngestor, DieseliteBuildError> {
    result.map_err(|source| {
        match source {
            sql_traits::errors::Error::SqlParserError { error, .. } => {
                DieseliteBuildError::UnparsableDefinition {
                    database: database.to_owned(),
                    name: name.to_owned(),
                    sql: sql.to_owned(),
                    source: error,
                }
            }
            source => {
                DieseliteBuildError::Ingestion {
                    database: database.to_owned(),
                    name: name.to_owned(),
                    source: Box::new(source),
                }
            }
        }
    })
}

/// Reads the columns SQLite declares for the virtual table `name` in
/// `database`, and the suffixes of its shadow tables among `relations`.
fn declared_virtual_table(
    connection: &mut SqliteConnection,
    relations: &[TableListEntry],
    database: &str,
    name: &str,
) -> Result<DeclaredVirtualTable, diesel::result::Error> {
    let columns = pragma_table_xinfo::table
        .filter(pragma_table_xinfo::arg.eq(name))
        .filter(pragma_table_xinfo::schema.eq(database))
        .order(pragma_table_xinfo::cid)
        .select(TableColumn::as_select())
        .load::<TableColumn>(connection)?;
    let suffixes = relations
        .iter()
        .filter(|relation| relation.schema == database && relation.table_type == "shadow")
        .filter_map(|relation| relation.name.rsplit_once('_'))
        .filter(|(prefix, _)| prefix.eq_ignore_ascii_case(name))
        .map(|(_, suffix)| suffix);
    Ok(DeclaredVirtualTable::new(
        columns.into_iter().map(|column| {
            DeclaredColumn::new(column.name, column.declared_type, column.hidden == 1)
        }),
        suffixes,
    ))
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
    let statements = Parser::parse_sql(&SQLiteDialect {}, sql).map_err(failure)?;
    let [statement] = <[Statement; 1]>::try_from(statements).map_err(|_| {
        failure(ParserError::ParserError("expected exactly one stored statement".to_owned()))
    })?;
    Ok(statement)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_sqlite_before_3_37() {
        assert!(check_version("3.37.0".to_owned()).is_ok());
        for version in ["3.36.9", "3.37", "3.37.x"] {
            assert!(matches!(
                check_version(version.to_owned()),
                Err(DieseliteBuildError::UnsupportedSqliteVersion { found }) if found == version
            ));
        }
    }

    fn ingested(
        relations: &[TableListEntry],
        sql: &str,
    ) -> Result<ParserDBIngestor, DieseliteBuildError> {
        let mut connection =
            SqliteConnection::establish(":memory:").expect("in-memory database opens");
        let input = ParseOptions::default()
            .with_identifier_case(IdentifierCase::Folded)
            .ingestor::<SQLiteDialect>("main".to_owned());
        ingest(input, &mut connection, relations, "main", "t", sql)
    }

    #[test]
    fn refuses_an_unparsable_definition() {
        assert!(matches!(
            ingested(&[], "CREATE TABLE t("),
            Err(DieseliteBuildError::UnparsableDefinition { name, sql, .. })
                if name == "t" && sql == "CREATE TABLE t("
        ));
    }

    fn virtual_table() -> TableListEntry {
        TableListEntry {
            schema: "main".to_owned(),
            name: "t".to_owned(),
            table_type: "virtual".to_owned(),
            ncol: 1,
            wr: false,
            strict: false,
        }
    }

    #[test]
    fn refuses_a_virtual_definition_of_several_statements() {
        assert!(matches!(
            ingested(&[virtual_table()], "CREATE VIRTUAL TABLE t USING fts5(a); CREATE TABLE u(b)"),
            Err(DieseliteBuildError::UnparsableDefinition { name, .. }) if name == "t"
        ));
    }

    #[test]
    fn refuses_a_virtual_table_stored_as_a_plain_table() {
        assert!(matches!(
            ingested(&[virtual_table()], "CREATE TABLE t(a)"),
            Err(DieseliteBuildError::Ingestion { name, .. }) if name == "t"
        ));
    }

    #[test]
    fn reports_a_definition_the_model_refuses() {
        assert!(matches!(
            ingested(&[], "CREATE TABLE t(a, a)"),
            Err(DieseliteBuildError::Ingestion { database, name, .. })
                if database == "main" && name == "t"
        ));
    }
}
