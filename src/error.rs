//! Failures while reading and checking a SQLite schema.

/// A failure while building a database from SQLite's catalog.
#[derive(Debug, thiserror::Error)]
pub enum DieseliteBuildError {
    /// No connection was supplied.
    #[error("a SQLite connection is required")]
    MissingConnection,
    /// The connection predates the supported catalog functions.
    #[error("SQLite {found} is older than 3.37.0")]
    UnsupportedSqliteVersion {
        /// Version reported by SQLite.
        found: String,
    },
    /// A catalog query failed.
    #[error(transparent)]
    Query(#[from] diesel::result::Error),
    /// A stored definition could not be parsed as one statement.
    #[error("cannot parse {database}.{name}")]
    UnparsableDefinition {
        /// Database holding the definition.
        database: String,
        /// Object being parsed.
        name: String,
        /// Stored definition.
        sql: String,
        /// Parser failure.
        #[source]
        source: sqlparser::parser::ParserError,
    },
    /// The parsed definition could not enter the model.
    #[error("cannot ingest {database}.{name}")]
    Ingestion {
        /// Database holding the definition.
        database: String,
        /// Object being ingested.
        name: String,
        /// Model failure.
        #[source]
        source: Box<sql_traits::errors::Error>,
    },
    /// A foreign key's target could not be read from the model.
    #[error("cannot resolve foreign key {id} on {database}.{table}")]
    ForeignKeyLookup {
        /// Database holding the referencing table.
        database: String,
        /// Referencing table.
        table: String,
        /// SQLite's foreign key identifier.
        id: i32,
        /// Target lookup failure.
        #[source]
        source: sql_traits::errors::LookupError,
    },
    /// SQLite's catalog disagrees with the model.
    #[error("{database}.{object} disagrees on {fact} ({catalog} versus {model})")]
    Mismatch {
        /// Database holding the object.
        database: String,
        /// Object being checked.
        object: String,
        /// Stable name of the checked fact.
        fact: &'static str,
        /// SQLite's answer.
        catalog: String,
        /// Model's answer.
        model: String,
    },
}
