//! Pure comparisons between pragma rows and the parsed model.

use diesel::prelude::*;
use sql_traits::{errors::LookupError, prelude::*};
use sqlparser::ast::Expr;

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
            let indexes = pragma_index_list::table
                .filter(pragma_index_list::arg.eq(&relation.name))
                .filter(pragma_index_list::schema.eq(&relation.schema))
                .select(IndexListEntry::as_select())
                .load::<IndexListEntry>(connection)?
                .into_iter()
                .map(|index| {
                    let columns = pragma_index_xinfo::table
                        .filter(pragma_index_xinfo::arg.eq(&index.name))
                        .filter(pragma_index_xinfo::schema.eq(&relation.schema))
                        .filter(pragma_index_xinfo::key.eq(true))
                        .order(pragma_index_xinfo::seqno)
                        .select(IndexColumn::as_select())
                        .load(connection)?;
                    Ok((index, columns))
                })
                .collect::<Result<_, diesel::result::Error>>()?;
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
        if relation.table_type == "shadow" {
            names_equal(
                relation,
                "shadow owner",
                std::iter::once(relation.name.rsplit_once('_').map(|(prefix, _)| prefix)),
                std::iter::once(owner.map(TableLike::table_name)),
            )?;
        }
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
        catalog.columns.iter().map(|column| Some(column.name.as_str())),
        columns.map(|column| Some(column.column_name())),
    )?;
    let mut primary = catalog.columns.iter().filter(|column| column.pk > 0).collect::<Vec<_>>();
    primary.sort_by_key(|column| column.pk);
    names_equal(
        relation,
        "primary key",
        primary.iter().map(|column| Some(column.name.as_str())),
        answer(relation, "primary key", table.primary_key_columns(database))?
            .map(|column| Some(column.column_name())),
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
            matches!(catalog_column.hidden, 2 | 3),
            column.generation_expression().is_some(),
        )?;
    }
    Ok(())
}

fn names_equal<'catalog, 'model>(
    relation: &TableListEntry,
    fact: &'static str,
    catalog: impl Iterator<Item = Option<&'catalog str>>,
    mut model: impl Iterator<Item = Option<&'model str>>,
) -> Result<(), DieseliteBuildError> {
    for expected in catalog {
        let actual = model.next();
        let agrees = match (expected, actual) {
            (Some(expected), Some(Some(actual))) => expected.eq_ignore_ascii_case(actual),
            (None, Some(None)) => true,
            _ => false,
        };
        if !agrees {
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
    let groups = catalog.foreign_keys.chunk_by(|left, right| left.id == right.id);
    let mut unmatched =
        answer(relation, "foreign key count", table.foreign_keys(database))?.collect::<Vec<_>>();
    equal(relation, "foreign key count", groups.clone().count(), unmatched.len())?;
    for group in groups {
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
    if group.iter().all(|column| column.to.is_none()) {
        return true;
    }
    let Ok(mut columns) = key.referenced_columns(database) else { return true };
    let mut expected = group.iter();
    columns.all(|column| {
        expected
            .next()
            .and_then(|expected| expected.to.as_deref())
            .is_some_and(|expected| expected.eq_ignore_ascii_case(column.column_name()))
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
        std::iter::once(Some(group[0].referenced_table.as_str())),
        std::iter::once(Some(key.referenced_table_name().name())),
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
    let referenced = key
        .referenced_columns(database)
        .map_err(|source| {
            DieseliteBuildError::ForeignKeyLookup {
                database: relation.schema.clone(),
                table: relation.name.clone(),
                id: group[0].id,
                source,
            }
        })?
        .map(|column| Some(column.column_name()));
    if group.iter().all(|column| column.to.is_none()) {
        let parent = key.referenced_table(database).map_err(|source| {
            DieseliteBuildError::ForeignKeyLookup {
                database: relation.schema.clone(),
                table: relation.name.clone(),
                id: group[0].id,
                source,
            }
        })?;
        names_equal(
            relation,
            "foreign key referenced columns",
            answer(
                relation,
                "foreign key referenced columns",
                parent.primary_key_columns(database),
            )?
            .map(|column| Some(column.column_name())),
            referenced,
        )
    } else {
        names_equal(
            relation,
            "foreign key referenced columns",
            group.iter().map(|column| column.to.as_deref()),
            referenced,
        )
    }
}

fn column_expression(expression: &Expr) -> Option<&str> {
    match expression {
        Expr::Identifier(identifier) => Some(identifier.value.as_str()),
        Expr::Nested(inner) | Expr::Collate { expr: inner, .. } => column_expression(inner),
        _ => None,
    }
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
    for (index, columns) in &catalog.indexes {
        let expected = columns.iter().map(|column| column.name.as_deref());
        if index.origin == "c" {
            let model = indexes
                .iter()
                .find(|model| {
                    model.name().is_some_and(|name| name.eq_ignore_ascii_case(&index.name))
                })
                .ok_or_else(|| mismatch(relation, "index names", &index.name, "absent"))?;
            equal(relation, "index uniqueness", index.unique, model.is_unique())?;
            equal(relation, "index partiality", index.partial, model.predicate().is_some())?;
            names_equal(
                relation,
                "index columns",
                expected,
                answer(relation, "index columns", model.keys(database))?
                    .map(|key| column_expression(key.expression())),
            )?;
        } else if index.origin == "pk" {
            names_equal(
                relation,
                "primary key index",
                expected,
                answer(relation, "primary key index", table.primary_key_columns(database))?
                    .map(|column| Some(column.column_name())),
            )?;
        } else {
            let mut found = false;
            for unique in answer(relation, "unique constraint", table.unique_indices(database))? {
                let mut expected = expected.clone();
                let agrees = answer(relation, "unique constraint", unique.columns(database))?.all(
                    |column| {
                        expected
                            .next()
                            .flatten()
                            .is_some_and(|name| name.eq_ignore_ascii_case(column.column_name()))
                    },
                ) && expected.next().is_none();
                if agrees {
                    found = true;
                    break;
                }
            }
            equal(relation, "unique constraint", true, found)?;
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
}
