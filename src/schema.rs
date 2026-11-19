//! Diesel tables for the SQLite catalog.
//!
//! [`sqlite_schema`] is the catalog table of the `main` database. Every other
//! table here is a pragma table-valued function. Its argument is the hidden
//! `arg` column and the database it reads is the hidden `schema` column, so a
//! filter or a join condition on them takes the place of the call arguments.
//! A pragma that names a table or an index returns no rows until `arg` is
//! constrained.
//!
//! See <https://www.sqlite.org/schematab.html> and
//! <https://www.sqlite.org/pragma.html#pragfunc>.

diesel::table! {
    /// One row per table, index, view and trigger of the `main` database,
    /// holding the DDL as SQLite stores it.
    sqlite_schema (name) {
        /// `table`, `index`, `view` or `trigger`.
        #[sql_name = "type"]
        object_type -> Text,
        /// Name of the object.
        name -> Text,
        /// Table or view the object belongs to, and the object's own name for
        /// a table or a view.
        tbl_name -> Text,
        /// Root b-tree page, or `0` for views, triggers and virtual tables.
        rootpage -> BigInt,
        /// The `CREATE` statement as written, normalised by SQLite. `NULL`
        /// for the indexes SQLite creates for `UNIQUE` and `PRIMARY KEY`
        /// constraints.
        sql -> Nullable<Text>,
    }
}

diesel::table! {
    /// `pragma_table_list`: one row per table and view of every database,
    /// since SQLite 3.37.
    pragma_table_list (schema, name) {
        /// Database holding the table, such as `main`, `temp` or an attached
        /// name.
        schema -> Text,
        /// Name of the table or view.
        name -> Text,
        /// `table`, `view`, `virtual` or `shadow` (a table that stores a
        /// virtual table's data).
        #[sql_name = "type"]
        table_type -> Text,
        /// Number of columns, generated and hidden ones included.
        ncol -> Integer,
        /// Whether the table is `WITHOUT ROWID`.
        wr -> Bool,
        /// Whether the table is `STRICT`.
        strict -> Bool,
        /// Hidden argument restricting the list to one table.
        arg -> Text,
    }
}

diesel::table! {
    /// `pragma_table_xinfo`: one row per column of the table or view named
    /// by `arg`, generated and hidden columns included.
    pragma_table_xinfo (schema, arg, cid) {
        /// Position of the column in the table, from `0`.
        cid -> Integer,
        /// Name of the column.
        name -> Text,
        /// Declared type as written, or an empty string when there is none.
        #[sql_name = "type"]
        declared_type -> Text,
        /// Whether the column is `NOT NULL`.
        notnull -> Bool,
        /// Text of the `DEFAULT` expression.
        dflt_value -> Nullable<Text>,
        /// Position of the column in the primary key, from `1`, or `0` when it
        /// is not part of it.
        pk -> Integer,
        /// `0` for an ordinary column, `1` for a hidden column of a virtual
        /// table, `2` for a `VIRTUAL` generated column and `3` for a `STORED`
        /// one.
        hidden -> Integer,
        /// Hidden argument naming the table or view.
        arg -> Text,
        /// Hidden argument naming the database.
        schema -> Text,
    }
}

diesel::table! {
    /// `pragma_foreign_key_list`: one row per column pair of each foreign
    /// key of the table named by `arg`.
    pragma_foreign_key_list (schema, arg, id, seq) {
        /// Foreign key the row belongs to, from `0`.
        id -> Integer,
        /// Position of the column pair within the foreign key, from `0`.
        seq -> Integer,
        /// Referenced table.
        #[sql_name = "table"]
        referenced_table -> Text,
        /// Referencing column.
        from -> Text,
        /// Referenced column, or `NULL` when the key references the primary
        /// key without naming its columns.
        to -> Nullable<Text>,
        /// `ON UPDATE` action, such as `NO ACTION` or `CASCADE`.
        on_update -> Text,
        /// `ON DELETE` action, such as `NO ACTION` or `CASCADE`.
        on_delete -> Text,
        /// `MATCH` clause, `NONE` when absent.
        #[sql_name = "match"]
        match_clause -> Text,
        /// Hidden argument naming the referencing table.
        arg -> Text,
        /// Hidden argument naming the database.
        schema -> Text,
    }
}

diesel::table! {
    /// `pragma_index_list`: one row per index of the table named by `arg`.
    pragma_index_list (schema, arg, name) {
        /// Sequence number SQLite assigns to the index.
        seq -> Integer,
        /// Name of the index.
        name -> Text,
        /// Whether the index is `UNIQUE`.
        unique -> Bool,
        /// `c` for `CREATE INDEX`, `u` for a `UNIQUE` constraint and `pk` for
        /// a `PRIMARY KEY` constraint.
        origin -> Text,
        /// Whether the index has a `WHERE` clause.
        partial -> Bool,
        /// Hidden argument naming the table.
        arg -> Text,
        /// Hidden argument naming the database.
        schema -> Text,
    }
}

diesel::table! {
    /// `pragma_index_xinfo`: one row per column of the index named by `arg`,
    /// auxiliary columns included. For a `WITHOUT ROWID` table named by
    /// `arg`, the columns of the table's own b-tree.
    pragma_index_xinfo (schema, arg, seqno) {
        /// Position of the column in the index, from `0`. Key columns come
        /// before auxiliary ones.
        seqno -> Integer,
        /// Position of the column in the table, `-1` for the rowid and `-2`
        /// for an expression.
        cid -> Integer,
        /// Name of the column, `NULL` for the rowid and for an expression.
        name -> Nullable<Text>,
        /// Whether the column is sorted `DESC`.
        desc -> Bool,
        /// Collating sequence of the column.
        coll -> Text,
        /// Whether the column is a key column rather than an auxiliary one.
        key -> Bool,
        /// Hidden argument naming the index.
        arg -> Text,
        /// Hidden argument naming the database.
        schema -> Text,
    }
}

diesel::table! {
    /// `pragma_database_list`: one row per database of the connection.
    pragma_database_list (seq) {
        /// Sequence number, `0` for `main` and `1` for `temp`.
        seq -> Integer,
        /// `main`, `temp` or the name given to `ATTACH`.
        name -> Text,
        /// Path of the database file, or an empty string when there is none.
        file -> Text,
    }
}

diesel::table! {
    /// `pragma_function_list`: one row per calling signature of each SQL
    /// function known to the connection.
    pragma_function_list (name, function_type, enc, narg) {
        /// Name of the function.
        name -> Text,
        /// Whether SQLite provides the function rather than the application.
        builtin -> Bool,
        /// `s` for a scalar, `a` for an aggregate and `w` for a window
        /// function.
        #[sql_name = "type"]
        function_type -> Text,
        /// Text encoding the function accepts, such as `utf8`.
        enc -> Text,
        /// Number of arguments, `-1` for any number.
        narg -> Integer,
        /// `SQLITE_DETERMINISTIC` and the other flags the function was
        /// registered with.
        flags -> Integer,
    }
}

diesel::table! {
    /// `pragma_collation_list`: one row per collating sequence of the
    /// connection.
    pragma_collation_list (name) {
        /// Sequence number.
        seq -> Integer,
        /// Name of the collating sequence.
        name -> Text,
    }
}

diesel::table! {
    /// `pragma_module_list`: one row per virtual table module registered
    /// with the connection.
    pragma_module_list (name) {
        /// Name of the module.
        name -> Text,
    }
}

diesel::allow_tables_to_appear_in_same_query!(
    sqlite_schema,
    pragma_table_list,
    pragma_table_xinfo,
    pragma_foreign_key_list,
    pragma_index_list,
    pragma_index_xinfo,
    pragma_database_list,
    pragma_function_list,
    pragma_collation_list,
    pragma_module_list,
);
