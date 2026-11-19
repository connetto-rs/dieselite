//! Rows of the catalog tables in [`crate::schema`].
//!
//! Each model selects the visible columns of its table. The hidden `arg` and
//! `schema` columns of the pragma functions are arguments, so they are
//! filtered or joined on and never selected.

use diesel::{Queryable, Selectable};

use crate::schema::{
    pragma_collation_list, pragma_database_list, pragma_foreign_key_list, pragma_function_list,
    pragma_index_list, pragma_index_xinfo, pragma_module_list, pragma_table_list,
    pragma_table_xinfo, sqlite_schema,
};

/// A table, index, view or trigger with its stored DDL, from [`sqlite_schema`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Queryable, Selectable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[diesel(table_name = sqlite_schema, check_for_backend(diesel::sqlite::Sqlite))]
pub struct SchemaObject {
    /// `table`, `index`, `view` or `trigger`.
    pub object_type: String,
    /// Name of the object.
    pub name: String,
    /// Table or view the object belongs to.
    pub tbl_name: String,
    /// Root b-tree page, `0` for views, triggers and virtual tables.
    pub rootpage: i64,
    /// The stored `CREATE` statement, absent for constraint indexes.
    pub sql: Option<String>,
}

/// A table or view of any database, from [`pragma_table_list`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Queryable, Selectable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[diesel(table_name = pragma_table_list, check_for_backend(diesel::sqlite::Sqlite))]
pub struct TableListEntry {
    /// Database holding the table.
    pub schema: String,
    /// Name of the table or view.
    pub name: String,
    /// `table`, `view`, `virtual` or `shadow`.
    pub table_type: String,
    /// Number of columns, generated and hidden ones included.
    pub ncol: i32,
    /// Whether the table is `WITHOUT ROWID`.
    pub wr: bool,
    /// Whether the table is `STRICT`.
    pub strict: bool,
}

/// A column of a table or view, from [`pragma_table_xinfo`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Queryable, Selectable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[diesel(table_name = pragma_table_xinfo, check_for_backend(diesel::sqlite::Sqlite))]
pub struct TableColumn {
    /// Position in the table, from `0`.
    pub cid: i32,
    /// Name of the column.
    pub name: String,
    /// Declared type as written, empty when there is none.
    pub declared_type: String,
    /// Whether the column is `NOT NULL`.
    pub notnull: bool,
    /// Text of the `DEFAULT` expression.
    pub dflt_value: Option<String>,
    /// Position in the primary key from `1`, or `0`.
    pub pk: i32,
    /// `0` ordinary, `1` hidden in a virtual table, `2` `VIRTUAL` generated,
    /// `3` `STORED` generated.
    pub hidden: i32,
}

/// One column pair of a foreign key, from [`pragma_foreign_key_list`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Queryable, Selectable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[diesel(table_name = pragma_foreign_key_list, check_for_backend(diesel::sqlite::Sqlite))]
pub struct ForeignKeyColumn {
    /// Foreign key the pair belongs to.
    pub id: i32,
    /// Position of the pair within the foreign key.
    pub seq: i32,
    /// Referenced table.
    pub referenced_table: String,
    /// Referencing column.
    pub from: String,
    /// Referenced column, absent when the key references the primary key
    /// implicitly.
    pub to: Option<String>,
    /// `ON UPDATE` action.
    pub on_update: String,
    /// `ON DELETE` action.
    pub on_delete: String,
    /// `MATCH` clause, `NONE` when absent.
    pub match_clause: String,
}

/// An index of a table, from [`pragma_index_list`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Queryable, Selectable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[diesel(table_name = pragma_index_list, check_for_backend(diesel::sqlite::Sqlite))]
pub struct IndexListEntry {
    /// Sequence number SQLite assigns to the index.
    pub seq: i32,
    /// Name of the index.
    pub name: String,
    /// Whether the index is `UNIQUE`.
    pub unique: bool,
    /// `c` for `CREATE INDEX`, `u` for `UNIQUE` and `pk` for `PRIMARY KEY`.
    pub origin: String,
    /// Whether the index has a `WHERE` clause.
    pub partial: bool,
}

/// A key or auxiliary column of an index, from [`pragma_index_xinfo`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Queryable, Selectable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[diesel(table_name = pragma_index_xinfo, check_for_backend(diesel::sqlite::Sqlite))]
pub struct IndexColumn {
    /// Position in the index, key columns first.
    pub seqno: i32,
    /// Position in the table, `-1` for the rowid and `-2` for an expression.
    pub cid: i32,
    /// Name of the column, absent for the rowid and for an expression.
    pub name: Option<String>,
    /// Whether the column is sorted `DESC`.
    pub desc: bool,
    /// Collating sequence.
    pub coll: String,
    /// Whether the column is a key column.
    pub key: bool,
}

/// A database of the connection, from [`pragma_database_list`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Queryable, Selectable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[diesel(table_name = pragma_database_list, check_for_backend(diesel::sqlite::Sqlite))]
pub struct AttachedDatabase {
    /// Sequence number, `0` for `main` and `1` for `temp`.
    pub seq: i32,
    /// `main`, `temp` or the name given to `ATTACH`.
    pub name: String,
    /// Path of the file, empty when there is none.
    pub file: String,
}

/// One calling signature of an SQL function, from [`pragma_function_list`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Queryable, Selectable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[diesel(table_name = pragma_function_list, check_for_backend(diesel::sqlite::Sqlite))]
pub struct FunctionSignature {
    /// Name of the function.
    pub name: String,
    /// Whether SQLite provides the function rather than the application.
    pub builtin: bool,
    /// `s` scalar, `a` aggregate, `w` window.
    pub function_type: String,
    /// Text encoding, such as `utf8`.
    pub enc: String,
    /// Number of arguments, `-1` for any number.
    pub narg: i32,
    /// Flags the function was registered with.
    pub flags: i32,
}

/// A collating sequence, from [`pragma_collation_list`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Queryable, Selectable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[diesel(table_name = pragma_collation_list, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Collation {
    /// Sequence number.
    pub seq: i32,
    /// Name of the collating sequence.
    pub name: String,
}

/// A virtual table module, from [`pragma_module_list`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Queryable, Selectable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[diesel(table_name = pragma_module_list, check_for_backend(diesel::sqlite::Sqlite))]
pub struct VirtualTableModule {
    /// Name of the module.
    pub name: String,
}
