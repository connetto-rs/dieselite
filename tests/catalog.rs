//! Reads every catalog relation of a database built from a fixed DDL corpus.

use diesel::{
    ConnectionError, ConnectionResult, connection::SimpleConnection, prelude::*, sql_types::Text,
    sqlite::SqliteConnection,
};
use dieselite::{
    models::{
        AttachedDatabase, Collation, ForeignKeyColumn, FunctionSignature, IndexColumn,
        IndexListEntry, SchemaObject, TableColumn, TableListEntry, VirtualTableModule,
    },
    schema::{
        pragma_collation_list, pragma_database_list, pragma_foreign_key_list, pragma_function_list,
        pragma_index_list, pragma_index_xinfo, pragma_module_list, pragma_table_list,
        pragma_table_xinfo, sqlite_schema,
    },
};

const CORPUS: &str = "
    CREATE TABLE parent(id INTEGER PRIMARY KEY, code TEXT NOT NULL UNIQUE, CHECK (length(code) > 0));
    CREATE TABLE child(
        id INTEGER PRIMARY KEY,
        parent_id INTEGER REFERENCES parent ON DELETE CASCADE,
        parent_code TEXT,
        amount REAL DEFAULT 1.5,
        doubled AS (amount * 2) STORED,
        halved AS (amount / 2) VIRTUAL,
        FOREIGN KEY (parent_code) REFERENCES parent(code) ON UPDATE SET NULL DEFERRABLE INITIALLY DEFERRED
    );
    CREATE TABLE pair(a TEXT, b INTEGER, note ANY, PRIMARY KEY (b, a)) STRICT, WITHOUT ROWID;
    CREATE INDEX child_amount ON child(amount DESC) WHERE amount > 0;
    CREATE UNIQUE INDEX parent_lower_code ON parent(lower(code) COLLATE NOCASE);
    CREATE VIEW child_amounts AS SELECT id, amount FROM child;
    CREATE TRIGGER parent_audit AFTER DELETE ON parent BEGIN DELETE FROM child WHERE parent_code = old.code; END;
    CREATE VIRTUAL TABLE notes USING fts5(body);
    ATTACH ':memory:' AS aux;
    CREATE TABLE aux.other(x);
    CREATE TEMP TABLE scratch(y);
";

diesel::define_sql_function! {
    /// An application-defined function, which the function list reports as not built in.
    fn shout(text: Text) -> Text;
}

/// Opens an in-memory database holding [`CORPUS`].
fn corpus() -> ConnectionResult<SqliteConnection> {
    let mut connection = SqliteConnection::establish(":memory:")?;
    connection.batch_execute(CORPUS).map_err(ConnectionError::CouldntSetupConfiguration)?;
    Ok(connection)
}

fn table_column(
    cid: i32,
    name: &str,
    declared_type: &str,
    notnull: bool,
    dflt_value: Option<&str>,
    pk: i32,
    hidden: i32,
) -> TableColumn {
    TableColumn {
        cid,
        name: name.to_owned(),
        declared_type: declared_type.to_owned(),
        notnull,
        dflt_value: dflt_value.map(str::to_owned),
        pk,
        hidden,
    }
}

fn index_column(
    seqno: i32,
    cid: i32,
    name: Option<&str>,
    desc: bool,
    coll: &str,
    key: bool,
) -> IndexColumn {
    IndexColumn { seqno, cid, name: name.map(str::to_owned), desc, coll: coll.to_owned(), key }
}

#[test]
fn schema_objects_keep_the_stored_ddl() {
    let mut connection = corpus().expect("the corpus database builds");
    let objects: Vec<SchemaObject> = sqlite_schema::table
        .select(SchemaObject::as_select())
        .order_by(sqlite_schema::name)
        .load(&mut connection)
        .expect("sqlite_schema loads");

    let names: Vec<(&str, &str, &str)> = objects
        .iter()
        .map(|object| (object.object_type.as_str(), object.name.as_str(), object.tbl_name.as_str()))
        .collect();
    assert_eq!(
        names,
        [
            ("table", "child", "child"),
            ("index", "child_amount", "child"),
            ("view", "child_amounts", "child_amounts"),
            ("table", "notes", "notes"),
            ("table", "notes_config", "notes_config"),
            ("table", "notes_content", "notes_content"),
            ("table", "notes_data", "notes_data"),
            ("table", "notes_docsize", "notes_docsize"),
            ("table", "notes_idx", "notes_idx"),
            ("table", "pair", "pair"),
            ("table", "parent", "parent"),
            ("trigger", "parent_audit", "parent"),
            ("index", "parent_lower_code", "parent"),
            ("index", "sqlite_autoindex_parent_1", "parent"),
        ]
    );

    let view = &objects[2];
    assert_eq!(view.rootpage, 0);
    assert_eq!(
        view.sql.as_deref(),
        Some("CREATE VIEW child_amounts AS SELECT id, amount FROM child")
    );
    let autoindex = &objects[13];
    assert!(autoindex.rootpage > 0);
    assert_eq!(autoindex.sql, None);
}

#[test]
fn table_list_classifies_every_schema_object() {
    let mut connection = corpus().expect("the corpus database builds");
    let tables: Vec<TableListEntry> = pragma_table_list::table
        .select(TableListEntry::as_select())
        .order_by((pragma_table_list::schema, pragma_table_list::name))
        .load(&mut connection)
        .expect("pragma_table_list loads");

    let rows: Vec<(&str, &str, &str, i32, bool, bool)> = tables
        .iter()
        .map(|table| {
            (
                table.schema.as_str(),
                table.name.as_str(),
                table.table_type.as_str(),
                table.ncol,
                table.wr,
                table.strict,
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("aux", "other", "table", 1, false, false),
            ("aux", "sqlite_schema", "table", 5, false, false),
            ("main", "child", "table", 6, false, false),
            ("main", "child_amounts", "view", 2, false, false),
            ("main", "notes", "virtual", 3, false, false),
            ("main", "notes_config", "shadow", 2, true, false),
            ("main", "notes_content", "shadow", 2, false, false),
            ("main", "notes_data", "shadow", 2, false, false),
            ("main", "notes_docsize", "shadow", 2, false, false),
            ("main", "notes_idx", "shadow", 3, true, false),
            ("main", "pair", "table", 3, true, true),
            ("main", "parent", "table", 2, false, false),
            ("main", "sqlite_schema", "table", 5, false, false),
            ("temp", "scratch", "table", 1, false, false),
            ("temp", "sqlite_temp_schema", "table", 5, false, false),
        ]
    );
}

#[test]
fn table_columns_join_through_the_hidden_argument() {
    let mut connection = corpus().expect("the corpus database builds");
    let columns: Vec<(String, TableColumn)> = sqlite_schema::table
        .inner_join(pragma_table_xinfo::table.on(pragma_table_xinfo::arg.eq(sqlite_schema::name)))
        .filter(sqlite_schema::name.eq_any(["child", "pair"]))
        .select((sqlite_schema::name, TableColumn::as_select()))
        .order_by((sqlite_schema::name, pragma_table_xinfo::cid))
        .load(&mut connection)
        .expect("sqlite_schema joins pragma_table_xinfo");

    let child = |column| ("child".to_owned(), column);
    let pair = |column| ("pair".to_owned(), column);
    assert_eq!(
        columns,
        [
            child(table_column(0, "id", "INTEGER", false, None, 1, 0)),
            child(table_column(1, "parent_id", "INTEGER", false, None, 0, 0)),
            child(table_column(2, "parent_code", "TEXT", false, None, 0, 0)),
            child(table_column(3, "amount", "REAL", false, Some("1.5"), 0, 0)),
            child(table_column(4, "doubled", "", false, None, 0, 3)),
            child(table_column(5, "halved", "", false, None, 0, 2)),
            pair(table_column(0, "a", "TEXT", true, None, 2, 0)),
            pair(table_column(1, "b", "INTEGER", true, None, 1, 0)),
            pair(table_column(2, "note", "ANY", false, None, 0, 0)),
        ]
    );
}

#[test]
fn table_columns_of_an_attached_database_are_read_through_the_hidden_schema() {
    let mut connection = corpus().expect("the corpus database builds");
    let columns: Vec<TableColumn> = pragma_table_xinfo::table
        .filter(pragma_table_xinfo::arg.eq("other"))
        .filter(pragma_table_xinfo::schema.eq("aux"))
        .select(TableColumn::as_select())
        .load(&mut connection)
        .expect("pragma_table_xinfo loads for an attached database");

    assert_eq!(columns, [table_column(0, "x", "", false, None, 0, 0)]);
}

#[test]
fn foreign_keys_report_actions_and_an_implicit_target_column() {
    let mut connection = corpus().expect("the corpus database builds");
    let foreign_keys: Vec<ForeignKeyColumn> = pragma_foreign_key_list::table
        .filter(pragma_foreign_key_list::arg.eq("child"))
        .select(ForeignKeyColumn::as_select())
        .order_by((pragma_foreign_key_list::id, pragma_foreign_key_list::seq))
        .load(&mut connection)
        .expect("pragma_foreign_key_list loads");

    assert_eq!(
        foreign_keys,
        [
            ForeignKeyColumn {
                id: 0,
                seq: 0,
                referenced_table: "parent".to_owned(),
                from: "parent_code".to_owned(),
                to: Some("code".to_owned()),
                on_update: "SET NULL".to_owned(),
                on_delete: "NO ACTION".to_owned(),
                match_clause: "NONE".to_owned(),
            },
            ForeignKeyColumn {
                id: 1,
                seq: 0,
                referenced_table: "parent".to_owned(),
                from: "parent_id".to_owned(),
                to: None,
                on_update: "NO ACTION".to_owned(),
                on_delete: "CASCADE".to_owned(),
                match_clause: "NONE".to_owned(),
            },
        ]
    );
}

#[test]
fn index_columns_include_expressions_and_auxiliary_rowids() {
    let mut connection = corpus().expect("the corpus database builds");
    let columns: Vec<(IndexListEntry, IndexColumn)> = pragma_index_list::table
        .inner_join(
            pragma_index_xinfo::table.on(pragma_index_xinfo::arg.eq(pragma_index_list::name)),
        )
        .filter(pragma_index_list::arg.eq("parent"))
        .select((IndexListEntry::as_select(), IndexColumn::as_select()))
        .order_by((pragma_index_list::seq, pragma_index_xinfo::seqno))
        .load(&mut connection)
        .expect("pragma_index_list joins pragma_index_xinfo");

    let lower_code = IndexListEntry {
        seq: 0,
        name: "parent_lower_code".to_owned(),
        unique: true,
        origin: "c".to_owned(),
        partial: false,
    };
    let autoindex = IndexListEntry {
        seq: 1,
        name: "sqlite_autoindex_parent_1".to_owned(),
        unique: true,
        origin: "u".to_owned(),
        partial: false,
    };
    assert_eq!(
        columns,
        [
            (lower_code.clone(), index_column(0, -2, None, false, "NOCASE", true)),
            (lower_code, index_column(1, -1, None, false, "BINARY", false)),
            (autoindex.clone(), index_column(0, 1, Some("code"), false, "BINARY", true)),
            (autoindex, index_column(1, -1, None, false, "BINARY", false)),
        ]
    );
}

#[test]
fn partial_descending_index_is_reported_as_such() {
    let mut connection = corpus().expect("the corpus database builds");
    let indexes: Vec<IndexListEntry> = pragma_index_list::table
        .filter(pragma_index_list::arg.eq("child"))
        .select(IndexListEntry::as_select())
        .load(&mut connection)
        .expect("pragma_index_list loads");
    let columns: Vec<IndexColumn> = pragma_index_xinfo::table
        .filter(pragma_index_xinfo::arg.eq("child_amount"))
        .select(IndexColumn::as_select())
        .order_by(pragma_index_xinfo::seqno)
        .load(&mut connection)
        .expect("pragma_index_xinfo loads");

    assert_eq!(
        indexes,
        [IndexListEntry {
            seq: 0,
            name: "child_amount".to_owned(),
            unique: false,
            origin: "c".to_owned(),
            partial: true,
        }]
    );
    assert_eq!(
        columns,
        [
            index_column(0, 3, Some("amount"), true, "BINARY", true),
            index_column(1, -1, None, false, "BINARY", false),
        ]
    );
}

#[test]
fn database_list_names_main_temp_and_attached_databases() {
    let mut connection = corpus().expect("the corpus database builds");
    let databases: Vec<AttachedDatabase> = pragma_database_list::table
        .select(AttachedDatabase::as_select())
        .order_by(pragma_database_list::seq)
        .load(&mut connection)
        .expect("pragma_database_list loads");

    let database =
        |seq, name: &str| AttachedDatabase { seq, name: name.to_owned(), file: String::new() };
    assert_eq!(databases, [database(0, "main"), database(1, "temp"), database(2, "aux")]);
}

#[test]
fn function_list_tells_application_functions_from_built_in_ones() {
    let mut connection = corpus().expect("the corpus database builds");
    shout_utils::register_impl(&mut connection, |text: String| text.to_uppercase())
        .expect("the application function registers");
    let functions: Vec<FunctionSignature> = pragma_function_list::table
        .filter(pragma_function_list::name.eq_any(["shout", "lower"]))
        .select(FunctionSignature::as_select())
        .order_by(pragma_function_list::name)
        .load(&mut connection)
        .expect("pragma_function_list loads");

    let summary: Vec<(&str, bool, &str, &str, i32)> = functions
        .iter()
        .map(|function| {
            (
                function.name.as_str(),
                function.builtin,
                function.function_type.as_str(),
                function.enc.as_str(),
                function.narg,
            )
        })
        .collect();
    assert_eq!(summary, [("lower", true, "s", "utf8", 1), ("shout", false, "s", "utf8", 1)]);
}

#[test]
fn collation_list_includes_registered_collations() {
    let mut connection = corpus().expect("the corpus database builds");
    connection
        .register_collation("reverse", |left, right| right.cmp(left))
        .expect("the collation registers");
    let names: Vec<String> = pragma_collation_list::table
        .select(Collation::as_select())
        .load(&mut connection)
        .expect("pragma_collation_list loads")
        .into_iter()
        .map(|collation| collation.name)
        .collect();

    for expected in ["BINARY", "NOCASE", "RTRIM", "reverse"] {
        assert!(names.iter().any(|name| name == expected), "{expected} missing from {names:?}");
    }
}

#[test]
fn module_list_includes_the_compiled_in_modules() {
    let mut connection = corpus().expect("the corpus database builds");
    let modules: Vec<VirtualTableModule> = pragma_module_list::table
        .filter(pragma_module_list::name.eq_any(["fts5", "rtree"]))
        .select(VirtualTableModule::as_select())
        .order_by(pragma_module_list::name)
        .load(&mut connection)
        .expect("pragma_module_list loads");

    assert_eq!(
        modules,
        [
            VirtualTableModule { name: "fts5".to_owned() },
            VirtualTableModule { name: "rtree".to_owned() },
        ]
    );
}
