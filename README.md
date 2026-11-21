# dieselite

[![CI](https://github.com/connetto-rs/dieselite/actions/workflows/ci.yml/badge.svg)](https://github.com/connetto-rs/dieselite/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/connetto-rs/dieselite/graph/badge.svg)](https://codecov.io/gh/connetto-rs/dieselite)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/connetto-rs/dieselite/blob/main/LICENSE)
[![crates.io](https://img.shields.io/crates/v/dieselite.svg)](https://crates.io/crates/dieselite)
[![docs.rs](https://docs.rs/dieselite/badge.svg)](https://docs.rs/dieselite)

Diesel tables and models for the SQLite catalog, so a program reads a live schema the way it reads its own tables. `sqlite_schema` holds the stored DDL, and the pragma table-valued functions such as `pragma_table_xinfo` hold what SQLite derived from it. Not published on crates.io yet, so the last two badges do not resolve.

A pragma function's argument is its hidden `arg` column, so joining on it lists the columns of every table.

```rust
use diesel::{connection::SimpleConnection, prelude::*};
use dieselite::{
    models::TableColumn,
    schema::{pragma_table_xinfo, sqlite_schema},
};

let mut connection = SqliteConnection::establish(":memory:")?;
connection.batch_execute("CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);")?;

let columns: Vec<(String, TableColumn)> = sqlite_schema::table
    .inner_join(pragma_table_xinfo::table.on(pragma_table_xinfo::arg.eq(sqlite_schema::name)))
    .filter(sqlite_schema::object_type.eq("table"))
    .select((sqlite_schema::name, TableColumn::as_select()))
    .load(&mut connection)?;

assert_eq!(columns.len(), 2);
assert_eq!(columns[1].0, "users");
assert_eq!(columns[1].1.name, "email");
assert!(columns[1].1.notnull);
# Ok::<(), Box<dyn std::error::Error>>(())
```

Build a checked `ParserDB` from a connection's stored definitions.

```rust
use diesel::{Connection, SqliteConnection};
use dieselite::{DieseliteDatabaseBuilder, sql_traits::prelude::ParserDB};

let mut connection = SqliteConnection::establish(":memory:").expect("connection");
let database: ParserDB = DieseliteDatabaseBuilder::default()
    .connection(&mut connection)
    .try_into()?;
# Ok::<(), dieselite::DieseliteBuildError>(())
```

The `serde` feature, on by default, derives `Serialize` and `Deserialize` on the models. The pragma tables need SQLite 3.37 or later.
