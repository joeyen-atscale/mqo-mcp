//! Error types for `mqo-from-sql`.

use thiserror::Error;

/// Errors that can occur while parsing AtScale SQL.
#[derive(Debug, Error)]
pub enum ParseError {
    /// The SQL text could not be parsed.
    #[error("SQL syntax error: {0}")]
    SqlSyntax(String),

    /// The SQL does not conform to the expected AtScale shape.
    #[error("unsupported SQL shape: {0}")]
    UnsupportedShape(String),
}

/// Errors that can occur while resolving parsed names against a catalog snapshot.
#[derive(Debug, Error)]
pub enum ResolveError {
    /// A column name from the SQL was not found in the catalog snapshot.
    #[error("unknown name '{0}' — not found in catalog snapshot")]
    UnknownName(String),

    /// A name matched more than one entry (ambiguous label).
    #[error("ambiguous name '{0}' — matched multiple catalog entries: {1:?}")]
    AmbiguousName(String, Vec<String>),
}

/// Top-level error for the mqo-from-sql pipeline.
#[derive(Debug, Error)]
pub enum MqoFromSqlError {
    /// Parse phase failed.
    #[error("parse error: {0}")]
    Parse(#[from] ParseError),

    /// Resolve phase failed.
    #[error("resolve error: {0}")]
    Resolve(#[from] ResolveError),

    /// MQO structural validation failed.
    #[error("MQO validation failed: {0:?}")]
    InvalidMqo(Vec<mqo_spec::MqoError>),

    /// I/O error (reading files).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}
