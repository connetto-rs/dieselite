//! Pure comparisons between pragma rows and the parsed model.

use std::iter::once;

use diesel::prelude::*;
use sql_traits::{errors::LookupError, prelude::*};
use sqlparser::ast::GeneratedExpressionMode;

use crate::{
    DieseliteBuildError,
    builder::internal,
    models::{ForeignKeyColumn, IndexColumn, IndexListEntry, TableColumn, TableListEntry},
    schema::{pragma_foreign_key_list, pragma_index_list, pragma_index_xinfo, pragma_table_xinfo},
};

pub(crate) struct CatalogRelation {
    pub(crate) relation: TableListEntry,
    pub(crate) columns: Vec<TableColumn>,
    pub(crate) foreign_keys: Vec<ForeignKeyColumn>,
    pub(crate) indexes: Vec<(IndexListEntry, Vec<IndexColumn>)>,
}

pub(crate) fn load(
    connection: &mut SqliteConnection,
    relations: Vec<TableListEntry>,
) -> Result<Vec<CatalogRelation>, DieseliteBuildError> {
    relations
        .into_iter()
        .filter(|relation| !internal(&relation.name))
        .map(|relation| {
            let columns = pragma_table_xinfo::table
                .filter(pragma_table_xinfo::arg.eq(&relation.name))
                .filter(pragma_table_xinfo::schema.eq(&relation.schema))
                .order(pragma_table_xinfo::cid)
                .select(TableColumn::as_select())
                .load(connection)?;
            let foreign_keys = pragma_foreign_key_list::table
                .filter(pragma_foreign_key_list::arg.eq(&relation.name))
                .filter(pragma_foreign_key_list::schema.eq(&relation.schema))
                .order((pragma_foreign_key_list::id, pragma_foreign_key_list::seq))
                .select(ForeignKeyColumn::as_select())
                .load(connection)?;
            let listed = pragma_index_list::table
                .filter(pragma_index_list::arg.eq(&relation.name))
                .filter(pragma_index_list::schema.eq(&relation.schema))
                .select(IndexListEntry::as_select())
                .load::<IndexListEntry>(connection)?;
            let mut indexes = Vec::with_capacity(listed.len());
            for index in listed {
                let columns = pragma_index_xinfo::table
                    .filter(pragma_index_xinfo::arg.eq(&index.name))
                    .filter(pragma_index_xinfo::schema.eq(&relation.schema))
                    .order(pragma_index_xinfo::seqno)
                    .select(IndexColumn::as_select())
                    .load(connection)?;
                indexes.push((index, columns));
            }
            Ok(CatalogRelation { relation, columns, foreign_keys, indexes })
        })
        .collect()
}

fn mismatch(
    relation: &TableListEntry,
    fact: &'static str,
    catalog: impl std::fmt::Debug,
    model: impl std::fmt::Debug,
) -> DieseliteBuildError {
    DieseliteBuildError::Mismatch {
        database: relation.schema.clone(),
        object: relation.name.clone(),
        fact,
        catalog: format!("{catalog:?}"),
        model: format!("{model:?}"),
    }
}

fn equal<T: PartialEq + std::fmt::Debug>(
    relation: &TableListEntry,
    fact: &'static str,
    catalog: T,
    model: T,
) -> Result<(), DieseliteBuildError> {
    if catalog == model { Ok(()) } else { Err(mismatch(relation, fact, catalog, model)) }
}

fn answer<T>(
    relation: &TableListEntry,
    fact: &'static str,
    result: Result<T, LookupError>,
) -> Result<T, DieseliteBuildError> {
    result.map_err(|error| mismatch(relation, fact, "readable", error.to_string()))
}

pub(crate) fn check(
    database: &ParserDB,
    catalogs: &[CatalogRelation],
) -> Result<(), DieseliteBuildError> {
    for (schema, name) in database
        .tables()
        .map(|table| (table.table_schema().unwrap_or("main"), table.table_name()))
        .chain(
            database.views().map(|view| (view.view_schema().unwrap_or("main"), view.view_name())),
        )
    {
        if !catalogs.iter().any(|catalog| {
            catalog.relation.schema.eq_ignore_ascii_case(schema)
                && catalog.relation.name.eq_ignore_ascii_case(name)
        }) {
            return Err(DieseliteBuildError::Mismatch {
                database: schema.to_owned(),
                object: name.to_owned(),
                fact: "relations",
                catalog: "absent".to_owned(),
                model: "present".to_owned(),
            });
        }
    }
    for catalog in catalogs {
        let relation = &catalog.relation;
        if relation.table_type == "view" {
            equal(
                relation,
                "relation kind",
                true,
                database.views().any(|view| {
                    view.view_name().eq_ignore_ascii_case(&relation.name)
                        && view
                            .view_schema()
                            .unwrap_or("main")
                            .eq_ignore_ascii_case(&relation.schema)
                }),
            )?;
            continue;
        }
        let table = database
            .tables()
            .find(|table| {
                table.table_name().eq_ignore_ascii_case(&relation.name)
                    && table.table_schema().unwrap_or("main").eq_ignore_ascii_case(&relation.schema)
            })
            .ok_or_else(|| mismatch(relation, "relations", "table", "absent"))?;
        equal(
            relation,
            "virtual",
            relation.table_type == "virtual",
            answer(relation, "virtual", table.is_virtual(database))?,
        )?;
        let owner = answer(relation, "shadow owner", table.shadow_owner(database))?;
        equal(relation, "shadow owner", relation.table_type == "shadow", owner.is_some())?;
        if let Some(owner) = owner {
            let expected =
                relation.name.rsplit_once('_').map_or(relation.name.as_str(), |(prefix, _)| prefix);
            names_equal(relation, "shadow owner", once(expected), once(owner.table_name()))?;
        }
        equal(relation, "strict", relation.strict, table.is_strict())?;
        equal(relation, "without rowid", relation.wr, table.is_without_rowid())?;
        check_columns(database, catalog, table)?;
        check_foreign_keys(database, catalog, table)?;
        check_indexes(database, catalog, table)?;
    }
    Ok(())
}

fn check_columns(
    database: &ParserDB,
    catalog: &CatalogRelation,
    table: &<ParserDB as DatabaseLike>::Table,
) -> Result<(), DieseliteBuildError> {
    let relation = &catalog.relation;
    let columns = answer(relation, "column names", table.columns(database))?;
    names_equal(
        relation,
        "column names",
        catalog.columns.iter().map(|column| column.name.as_str()),
        columns.map(ColumnLike::column_name),
    )?;
    let mut primary = catalog.columns.iter().filter(|column| column.pk > 0).collect::<Vec<_>>();
    primary.sort_by_key(|column| column.pk);
    names_equal(
        relation,
        "primary key",
        primary.iter().map(|column| column.name.as_str()),
        answer(relation, "primary key", table.primary_key_columns(database))?
            .map(ColumnLike::column_name),
    )?;
    let rowid_alias = !relation.wr
        && relation.table_type != "virtual"
        && primary.len() == 1
        && primary[0].declared_type.eq_ignore_ascii_case("INTEGER")
        && !catalog.indexes.iter().any(|(index, _)| index.origin == "pk");
    for (catalog_column, column) in
        catalog.columns.iter().zip(answer(relation, "column names", table.columns(database))?)
    {
        let not_null = catalog_column.notnull || rowid_alias && catalog_column.pk > 0;
        equal(
            relation,
            "not null",
            not_null,
            !answer(relation, "not null", column.is_nullable(database))?,
        )?;
        equal(
            relation,
            "hidden",
            catalog_column.hidden == 1,
            answer(relation, "hidden", column.is_hidden(database))?,
        )?;
        equal(
            relation,
            "generated",
            match catalog_column.hidden {
                2 => Some(GeneratedExpressionMode::Virtual),
                3 => Some(GeneratedExpressionMode::Stored),
                _ => None,
            },
            column.generation_storage(database),
        )?;
    }
    Ok(())
}

fn names_equal<'catalog, 'model>(
    relation: &TableListEntry,
    fact: &'static str,
    catalog: impl Iterator<Item = &'catalog str>,
    mut model: impl Iterator<Item = &'model str>,
) -> Result<(), DieseliteBuildError> {
    for expected in catalog {
        let actual = model.next();
        if !actual.is_some_and(|actual| expected.eq_ignore_ascii_case(actual)) {
            return Err(mismatch(relation, fact, expected, actual));
        }
    }
    if let Some(extra) = model.next() {
        return Err(mismatch(relation, fact, "end", extra));
    }
    Ok(())
}

fn check_foreign_keys(
    database: &ParserDB,
    catalog: &CatalogRelation,
    table: &<ParserDB as DatabaseLike>::Table,
) -> Result<(), DieseliteBuildError> {
    let relation = &catalog.relation;
    let same_key = |left: &ForeignKeyColumn, right: &ForeignKeyColumn| left.id == right.id;
    let mut unmatched =
        answer(relation, "foreign key count", table.foreign_keys(database))?.collect::<Vec<_>>();
    equal(
        relation,
        "foreign key count",
        catalog.foreign_keys.chunk_by(same_key).count(),
        unmatched.len(),
    )?;
    for group in catalog.foreign_keys.chunk_by(same_key) {
        let mut found = None;
        for (position, key) in unmatched.iter().enumerate() {
            let mut expected = group.iter();
            let agrees = answer(relation, "foreign key columns", key.host_columns(database))?.all(
                |column| {
                    expected.next().is_some_and(|expected| {
                        expected.from.eq_ignore_ascii_case(column.column_name())
                    })
                },
            ) && expected.next().is_none();
            if agrees {
                found.get_or_insert(position);
                if matches_foreign_key(database, group, key) {
                    found = Some(position);
                    break;
                }
            }
        }
        let position =
            found.ok_or_else(|| mismatch(relation, "foreign key columns", group, "absent"))?;
        let key = unmatched.swap_remove(position);
        check_foreign_key(database, relation, group, key)?;
    }
    Ok(())
}

fn action_name(action: sqlparser::ast::ReferentialAction) -> &'static str {
    use sqlparser::ast::ReferentialAction;
    match action {
        ReferentialAction::NoAction => "NO ACTION",
        ReferentialAction::Restrict => "RESTRICT",
        ReferentialAction::Cascade => "CASCADE",
        ReferentialAction::SetNull => "SET NULL",
        ReferentialAction::SetDefault => "SET DEFAULT",
    }
}

fn matches_foreign_key(
    database: &ParserDB,
    group: &[ForeignKeyColumn],
    key: &<ParserDB as DatabaseLike>::ForeignKey,
) -> bool {
    if !group[0].referenced_table.eq_ignore_ascii_case(key.referenced_table_name().name())
        || group[0].on_update != action_name(key.on_update(database))
        || group[0].on_delete != action_name(key.on_delete(database))
    {
        return false;
    }
    written_columns_agree(group, key)
}

/// Returns whether the referenced columns SQLite reports for `group` are the
/// ones `key` names as written. SQLite reports `to` for every column or for
/// none, the latter when the key names its target's primary key implicitly.
fn written_columns_agree(
    group: &[ForeignKeyColumn],
    key: &<ParserDB as DatabaseLike>::ForeignKey,
) -> bool {
    let mut expected = group.iter().filter_map(|column| column.to.as_deref());
    key.referenced_column_names().all(|(name, _)| {
        expected.next().is_some_and(|expected| expected.eq_ignore_ascii_case(name))
    }) && expected.next().is_none()
}

fn check_foreign_key(
    database: &ParserDB,
    relation: &TableListEntry,
    group: &[ForeignKeyColumn],
    key: &<ParserDB as DatabaseLike>::ForeignKey,
) -> Result<(), DieseliteBuildError> {
    names_equal(
        relation,
        "foreign key target",
        once(group[0].referenced_table.as_str()),
        once(key.referenced_table_name().name()),
    )?;
    equal(
        relation,
        "foreign key update",
        group[0].on_update.as_str(),
        action_name(key.on_update(database)),
    )?;
    equal(
        relation,
        "foreign key delete",
        group[0].on_delete.as_str(),
        action_name(key.on_delete(database)),
    )?;
    names_equal(
        relation,
        "foreign key referenced columns",
        group.iter().filter_map(|column| column.to.as_deref()),
        key.referenced_column_names().map(|(name, _)| name),
    )
}

/// One `pragma_index_xinfo` row as compared: the column name folded to lower
/// case, the `cid` SQLite gives the rowid (`-1`) and an expression (`-2`) or
/// `0` for a column, the direction, the collation, and whether it is a key.
type Entry = (Option<String>, i32, bool, String, bool);

fn catalog_entries(columns: &[IndexColumn]) -> Vec<Entry> {
    columns
        .iter()
        .map(|column| {
            (
                column.name.as_deref().map(str::to_lowercase),
                column.cid.min(0),
                column.desc,
                column.coll.clone(),
                column.key,
            )
        })
        .collect()
}

fn model_entries<I: TableIndexLike<DB = ParserDB>>(
    relation: &TableListEntry,
    index: &I,
    database: &ParserDB,
) -> Result<Vec<Entry>, DieseliteBuildError> {
    let entries = answer(relation, "index columns", index.sqlite_index_columns(database))?;
    Ok(entries
        .into_iter()
        .flatten()
        .map(|entry| {
            let (name, cid) = match entry.indexed() {
                SqliteIndexed::Column(column) => (Some(column.column_name().to_lowercase()), 0),
                SqliteIndexed::Rowid => (None, -1),
                SqliteIndexed::Expression(_) => (None, -2),
            };
            (name, cid, entry.is_descending(), entry.collation().to_owned(), entry.is_key())
        })
        .collect())
}

fn check_indexes(
    database: &ParserDB,
    catalog: &CatalogRelation,
    table: &<ParserDB as DatabaseLike>::Table,
) -> Result<(), DieseliteBuildError> {
    let relation = &catalog.relation;
    let indexes = answer(relation, "index names", table.indices(database))?.collect::<Vec<_>>();
    equal(
        relation,
        "index names",
        catalog.indexes.iter().filter(|(index, _)| index.origin == "c").count(),
        indexes.len(),
    )?;
    let constraints =
        answer(relation, "unique constraint", table.unique_indices(database))?.collect::<Vec<_>>();
    for (index, columns) in &catalog.indexes {
        let expected = catalog_entries(columns);
        if index.origin == "c" {
            let model = indexes
                .iter()
                .find(|model| {
                    model.name().is_some_and(|name| name.eq_ignore_ascii_case(&index.name))
                })
                .ok_or_else(|| mismatch(relation, "index names", &index.name, "absent"))?;
            equal(relation, "index uniqueness", index.unique, model.is_unique())?;
            equal(relation, "index partiality", index.partial, model.predicate().is_some())?;
            equal(relation, "index columns", expected, model_entries(relation, *model, database)?)?;
            continue;
        }
        let fact = if index.origin == "pk" { "primary key index" } else { "unique constraint" };
        let mut backed = false;
        for constraint in &constraints {
            let name = answer(relation, fact, constraint.backing_index_name(database))?;
            if name.is_some_and(|name| name.eq_ignore_ascii_case(&index.name)) {
                backed = true;
                equal(
                    relation,
                    "index columns",
                    &expected,
                    &model_entries(relation, *constraint, database)?,
                )?;
            }
        }
        equal(relation, fact, true, backed)?;
    }
    for constraint in &constraints {
        if let Some(name) =
            answer(relation, "unique constraint", constraint.backing_index_name(database))?
        {
            equal(
                relation,
                "unique constraint",
                true,
                catalog.indexes.iter().any(|(index, _)| {
                    index.origin != "c" && index.name.eq_ignore_ascii_case(&name)
                }),
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_relation_mismatches() -> Result<(), sql_traits::errors::Error> {
        let database = ParseOptions::default()
            .parse::<sqlparser::dialect::SQLiteDialect>("CREATE TABLE t(a INT)")?;
        let result = check(&database, &[]);
        assert!(matches!(
            result,
            Err(crate::DieseliteBuildError::Mismatch { fact: "relations", .. })
        ));
        Ok(())
    }

    fn catalog() -> CatalogRelation {
        CatalogRelation {
            relation: TableListEntry {
                schema: "main".to_owned(),
                name: "t".to_owned(),
                table_type: "table".to_owned(),
                ncol: 1,
                wr: false,
                strict: false,
            },
            columns: vec![TableColumn {
                cid: 0,
                name: "a".to_owned(),
                declared_type: "INT".to_owned(),
                notnull: false,
                dflt_value: None,
                pk: 0,
                hidden: 0,
            }],
            foreign_keys: vec![],
            indexes: vec![],
        }
    }

    fn index(origin: &str) -> (IndexListEntry, Vec<IndexColumn>) {
        (
            IndexListEntry {
                seq: 0,
                name: "idx".to_owned(),
                unique: false,
                origin: origin.to_owned(),
                partial: false,
            },
            vec![IndexColumn {
                seqno: 0,
                cid: 0,
                name: Some("a".to_owned()),
                desc: false,
                coll: "BINARY".to_owned(),
                key: true,
            }],
        )
    }

    fn foreign_key() -> ForeignKeyColumn {
        ForeignKeyColumn {
            id: 0,
            seq: 0,
            referenced_table: "t".to_owned(),
            from: "a".to_owned(),
            to: Some("a".to_owned()),
            on_update: "NO ACTION".to_owned(),
            on_delete: "NO ACTION".to_owned(),
            match_clause: "NONE".to_owned(),
        }
    }

    fn rejects(
        sql: &str,
        catalog: CatalogRelation,
        fact: &str,
    ) -> Result<(), sql_traits::errors::Error> {
        let database = ParseOptions::default()
            .with_identifier_case(IdentifierCase::Folded)
            .parse::<sqlparser::dialect::SQLiteDialect>(sql)?;
        let result = check(&database, &[catalog]);
        assert!(
            matches!(&result, Err(DieseliteBuildError::Mismatch { fact: found, .. }) if *found == fact),
            "{result:?}"
        );
        Ok(())
    }

    macro_rules! mismatch_test {
        ($name:ident, $sql:literal, $fact:literal, $catalog:ident, $change:block) => {
            #[test]
            fn $name() -> Result<(), sql_traits::errors::Error> {
                let mut $catalog = catalog();
                $change
                rejects($sql, $catalog, $fact)
            }
        };
    }

    mismatch_test!(relation_kind, "CREATE TABLE t(a INT)", "relation kind", rows, {
        rows.relation.table_type = "view".to_owned();
    });
    mismatch_test!(virtual_flag, "CREATE TABLE t(a INT)", "virtual", rows, {
        rows.relation.table_type = "virtual".to_owned();
    });
    mismatch_test!(shadow_owner, "CREATE TABLE t(a INT)", "shadow owner", rows, {
        rows.relation.table_type = "shadow".to_owned();
    });
    mismatch_test!(column_names, "CREATE TABLE t(a INT)", "column names", rows, {
        rows.columns[0].name = "other".to_owned();
    });
    mismatch_test!(primary_key, "CREATE TABLE t(a INT)", "primary key", rows, {
        rows.columns[0].pk = 1;
    });
    mismatch_test!(not_null, "CREATE TABLE t(a INT)", "not null", rows, {
        rows.columns[0].notnull = true;
    });
    mismatch_test!(hidden, "CREATE TABLE t(a INT)", "hidden", rows, {
        rows.columns[0].hidden = 1;
    });
    mismatch_test!(generated, "CREATE TABLE t(a INT)", "generated", rows, {
        rows.columns[0].hidden = 2;
    });
    mismatch_test!(strict, "CREATE TABLE t(a INT)", "strict", rows, {
        rows.relation.strict = true;
    });
    mismatch_test!(without_rowid, "CREATE TABLE t(a INT PRIMARY KEY)", "without rowid", rows, {
        rows.relation.wr = true;
        rows.columns[0].pk = 1;
    });
    mismatch_test!(generation_storage, "CREATE TABLE t(a INT AS (1) STORED)", "generated", rows, {
        rows.columns[0].hidden = 2;
    });
    mismatch_test!(foreign_key_count, "CREATE TABLE t(a INT)", "foreign key count", rows, {
        rows.foreign_keys.push(foreign_key());
    });
    mismatch_test!(
        foreign_key_columns,
        "CREATE TABLE t(a INT REFERENCES t(a))",
        "foreign key columns",
        rows,
        {
            let mut key = foreign_key();
            key.from = "other".to_owned();
            rows.foreign_keys.push(key);
        }
    );
    mismatch_test!(
        foreign_key_target,
        "CREATE TABLE t(a INT REFERENCES t(a))",
        "foreign key target",
        rows,
        {
            let mut key = foreign_key();
            key.referenced_table = "other".to_owned();
            rows.foreign_keys.push(key);
        }
    );
    mismatch_test!(
        foreign_key_update,
        "CREATE TABLE t(a INT REFERENCES t(a))",
        "foreign key update",
        rows,
        {
            let mut key = foreign_key();
            key.on_update = "CASCADE".to_owned();
            rows.foreign_keys.push(key);
        }
    );
    mismatch_test!(
        foreign_key_delete,
        "CREATE TABLE t(a INT REFERENCES t(a))",
        "foreign key delete",
        rows,
        {
            let mut key = foreign_key();
            key.on_delete = "CASCADE".to_owned();
            rows.foreign_keys.push(key);
        }
    );
    mismatch_test!(
        foreign_key_referenced_columns,
        "CREATE TABLE t(a INT UNIQUE REFERENCES t(a))",
        "foreign key referenced columns",
        rows,
        {
            let mut key = foreign_key();
            key.to = Some("other".to_owned());
            rows.foreign_keys.push(key);
        }
    );
    mismatch_test!(index_names, "CREATE TABLE t(a INT)", "index names", rows, {
        rows.indexes.push(index("c"));
    });
    mismatch_test!(
        index_uniqueness,
        "CREATE TABLE t(a INT); CREATE INDEX idx ON t(a)",
        "index uniqueness",
        rows,
        {
            let mut named = index("c");
            named.0.unique = true;
            rows.indexes.push(named);
        }
    );
    mismatch_test!(
        index_partiality,
        "CREATE TABLE t(a INT); CREATE INDEX idx ON t(a)",
        "index partiality",
        rows,
        {
            let mut named = index("c");
            named.0.partial = true;
            rows.indexes.push(named);
        }
    );
    mismatch_test!(
        index_columns,
        "CREATE TABLE t(a INT); CREATE INDEX idx ON t(a)",
        "index columns",
        rows,
        {
            let mut named = index("c");
            named.1[0].name = None;
            rows.indexes.push(named);
        }
    );
    mismatch_test!(primary_key_index, "CREATE TABLE t(a INT)", "primary key index", rows, {
        rows.indexes.push(index("pk"));
    });
    mismatch_test!(unique_constraint, "CREATE TABLE t(a INT)", "unique constraint", rows, {
        rows.indexes.push(index("u"));
    });
    mismatch_test!(
        index_auxiliary_entries,
        "CREATE TABLE t(a INT); CREATE INDEX idx ON t(a)",
        "index columns",
        rows,
        {
            rows.indexes.push(index("c"));
        }
    );
    mismatch_test!(
        backing_index_name,
        "CREATE TABLE t(a INT UNIQUE)",
        "unique constraint",
        rows,
        {
            rows.indexes.push(index("u"));
        }
    );
    mismatch_test!(backing_index_entries, "CREATE TABLE t(a INT UNIQUE)", "index columns", rows, {
        let mut backing = index("u");
        backing.0.name = "sqlite_autoindex_t_1".to_owned();
        rows.indexes.push(backing);
    });

    #[test]
    fn unlisted_backing_index() -> Result<(), sql_traits::errors::Error> {
        rejects("CREATE TABLE t(a INT UNIQUE)", catalog(), "unique constraint")
    }

    #[test]
    fn absent_table() -> Result<(), sql_traits::errors::Error> {
        let database = ParseOptions::default()
            .with_identifier_case(IdentifierCase::Folded)
            .parse::<sqlparser::dialect::SQLiteDialect>("CREATE TABLE t(a INT)")?;
        let mut missing = catalog();
        missing.relation.name = "missing".to_owned();
        let result = check(&database, &[catalog(), missing]);
        assert!(
            matches!(
                &result,
                Err(DieseliteBuildError::Mismatch { object, fact: "relations", .. })
                    if object == "missing"
            ),
            "{result:?}"
        );
        Ok(())
    }

    #[test]
    fn extra_model_column() -> Result<(), sql_traits::errors::Error> {
        rejects("CREATE TABLE t(a INT, b INT)", catalog(), "column names")
    }

    mismatch_test!(
        absent_named_index,
        "CREATE TABLE t(a INT); CREATE INDEX other ON t(a)",
        "index names",
        rows,
        {
            rows.indexes.push(index("c"));
        }
    );
}
