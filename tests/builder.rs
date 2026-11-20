//! Builder connection requirements.

#[test]
fn missing_connection_is_typed() {
    let result: Result<sql_traits::prelude::ParserDB, _> =
        dieselite::DieseliteDatabaseBuilder::default().try_into();
    assert!(matches!(result, Err(dieselite::DieseliteBuildError::MissingConnection)));
}

#[test]
fn sqlite_nullable_primary_key() -> Result<(), sql_traits::errors::Error> {
    use sql_traits::prelude::*;
    let database =
        ParseOptions::default()
            .with_identifier_case(IdentifierCase::Folded)
            .parse::<sqlparser::dialect::SQLiteDialect>("CREATE TABLE t(id TEXT PRIMARY KEY)")?;
    let table = database
        .table_by_target(TargetName::new("t", false), IdentifierCase::Folded)?
        .expect("table exists");
    let column = table.columns(&database)?.next().expect("column exists");
    assert!(column.is_nullable(&database)?);
    Ok(())
}

#[derive(Debug, thiserror::Error)]
enum TestError {
    #[error(transparent)]
    Connection(#[from] diesel::ConnectionError),
    #[error(transparent)]
    Query(#[from] diesel::result::Error),
    #[error(transparent)]
    Build(#[from] Box<dieselite::DieseliteBuildError>),
}

fn build(sql: &str) -> Result<sql_traits::prelude::ParserDB, TestError> {
    use diesel::{Connection, connection::SimpleConnection};
    let mut connection = diesel::SqliteConnection::establish(":memory:")?;
    connection.batch_execute(sql)?;
    Ok(dieselite::DieseliteDatabaseBuilder::default()
        .connection(&mut connection)
        .try_into()
        .map_err(Box::new)?)
}

#[test]
fn plain_table_options() {
    use sql_traits::prelude::*;
    let database = build(
        "CREATE TABLE t(id INTEGER PRIMARY KEY AUTOINCREMENT, value TEXT NOT NULL DEFAULT 'x' COLLATE NOCASE UNIQUE CHECK(length(value)>0), optional TEXT NULL, conflict INT NOT NULL ON CONFLICT FAIL DEFAULT 7)",
    ).expect("catalog agrees");
    let table = database.tables().next().expect("table");
    assert_eq!(table.columns(&database).expect("columns").count(), 4);
}

#[test]
fn generated_columns() {
    use sql_traits::prelude::*;
    let database = build("CREATE TABLE t(a INT, b INT AS (a+1) VIRTUAL, c INT AS (a+2) STORED)")
        .expect("catalog agrees");
    let table = database.tables().next().expect("table");
    assert_eq!(
        table
            .columns(&database)
            .expect("columns")
            .filter(|column| column.generation_expression().is_some())
            .count(),
        2
    );
}

#[test]
fn without_rowid_and_strict() {
    use sql_traits::prelude::*;
    let database = build("CREATE TABLE t(a INT,b TEXT,PRIMARY KEY(b,a)) WITHOUT ROWID, STRICT")
        .expect("catalog agrees");
    let table = database.tables().next().expect("table");
    assert_eq!(
        table
            .primary_key_columns(&database)
            .expect("key")
            .map(ColumnLike::column_name)
            .collect::<Vec<_>>(),
        ["b", "a"]
    );
}

#[test]
fn forward_and_cyclic_keys() {
    use sql_traits::prelude::*;
    let database = build(
        "CREATE TABLE a(id INT PRIMARY KEY,b INT REFERENCES b(id)); CREATE TABLE b(id INT PRIMARY KEY,a INT REFERENCES a(id))",
    ).expect("catalog agrees");
    assert_eq!(database.tables().count(), 2);
}

#[test]
fn missing_foreign_key_target() {
    use sql_traits::prelude::*;
    let database = build("CREATE TABLE h(a INT REFERENCES absent(id), b INT REFERENCES absent2)")
        .expect("catalog agrees");
    assert_eq!(
        database.tables().next().expect("table").foreign_keys(&database).expect("keys").count(),
        2
    );
}

#[test]
fn composite_and_implicit_keys() {
    use sql_traits::prelude::*;
    let database = build(
        "CREATE TABLE p(a INT,b INT,PRIMARY KEY(b,a)); CREATE TABLE c(a INT,b INT,FOREIGN KEY(a,b) REFERENCES p ON DELETE CASCADE ON UPDATE SET NULL)",
    ).expect("catalog agrees");
    assert_eq!(database.tables().count(), 2);
}

#[test]
fn unique_partial_expression_indexes() {
    use sql_traits::prelude::*;
    let database = build(
        "CREATE TABLE t(a INT,b TEXT UNIQUE); CREATE UNIQUE INDEX named ON t(a); CREATE INDEX partial ON t(b) WHERE a>0; CREATE INDEX expression ON t((a+1))",
    ).expect("catalog agrees");
    assert_eq!(database.indexes().count(), 3);
}

#[test]
fn views_and_triggers() {
    use sql_traits::prelude::*;
    let database = build(
        "CREATE TABLE t(a INT); CREATE VIEW v AS SELECT a FROM t; CREATE TRIGGER tr AFTER INSERT ON t BEGIN SELECT new.a; END",
    ).expect("catalog agrees");
    assert_eq!(database.views().count(), 1);
}

#[test]
fn fts5_virtual_table() {
    use sql_traits::prelude::*;
    let database = build("CREATE VIRTUAL TABLE notes USING fts5(body)").expect("catalog agrees");
    assert_eq!(
        database.tables().filter(|table| table.is_virtual(&database).expect("virtual")).count(),
        1
    );
}

#[test]
fn dbstat_virtual_table() {
    use sql_traits::prelude::*;
    let database = build("CREATE VIRTUAL TABLE stats USING dbstat").expect("catalog agrees");
    assert_eq!(
        database.tables().next().expect("table").columns(&database).expect("columns").count(),
        12
    );
}

#[test]
fn attached_and_temp_databases() {
    use sql_traits::prelude::*;
    let database = build(
        "CREATE TABLE t(a INT); ATTACH ':memory:' AS other; CREATE TABLE other.p(id INT PRIMARY KEY); CREATE TABLE other.c(id INT REFERENCES p(id)); CREATE TEMP TABLE temp_table(a INT); CREATE TEMP VIEW temp_view AS SELECT * FROM main.t",
    ).expect("catalog agrees");
    assert_eq!(database.tables().count(), 4);
    assert_eq!(database.views().count(), 1);
}

#[test]
fn mixed_case_names() {
    use sql_traits::prelude::*;
    let database = build("CREATE TABLE \"Users\"(\"ID\" INT)").expect("catalog agrees");
    assert!(
        database
            .table_by_target(
                TargetName::new("users", false).with_schema("main", false),
                IdentifierCase::Folded
            )
            .expect("lookup")
            .is_some()
    );
}

#[test]
fn foreign_keys_sharing_host_columns() {
    let result = build(
        "CREATE TABLE t(a INT UNIQUE REFERENCES t(a) ON DELETE CASCADE REFERENCES t(a) ON DELETE SET NULL)",
    );
    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn rowid_alias_table_primary_key_descending() {
    let result = build("CREATE TABLE t(id INTEGER, PRIMARY KEY(id DESC))");
    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn descending_inline_primary_key_is_nullable() {
    let result = build("CREATE TABLE t(id INTEGER PRIMARY KEY DESC)");
    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn triggers() {
    use sql_traits::prelude::*;
    let database =
        build("CREATE TABLE t(a INT); CREATE TRIGGER tr AFTER INSERT ON t BEGIN SELECT new.a; END")
            .expect("catalog agrees");
    assert_eq!(database.triggers().count(), 1);
}

#[test]
fn temp_view_reads_main() {
    use sql_traits::prelude::*;
    let database = build("CREATE TABLE t(a INT); CREATE TEMP TABLE scratch(a INT); CREATE TEMP VIEW v AS SELECT * FROM main.t").expect("catalog agrees");
    assert!(database.tables().any(|table| table.table_schema() == Some("temp")));
    assert!(database.views().any(|view| view.view_schema() == Some("temp")));
}
