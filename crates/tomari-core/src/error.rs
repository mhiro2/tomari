//! Error type shared across Tomari crates.

/// Result alias used throughout the core crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can originate from the core layer.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("schema migration failed: {0}")]
    Migration(String),

    #[error("could not resolve application data directory")]
    NoDataDir,

    #[error("{kind} not found: {id}")]
    NotFound { kind: &'static str, id: String },

    #[error("invalid value for {field}: {reason}")]
    Invalid { field: &'static str, reason: String },
}

impl Error {
    /// Whether this error means the database file itself is unreadable
    /// (corrupt, or not a SQLite database at all), as opposed to a transient
    /// condition such as a lock held by another process, a read-only mount or
    /// a full disk. Recovery may only move the file aside for the former —
    /// doing so on a transient error would discard a healthy database.
    pub fn is_database_corruption(&self) -> bool {
        use rusqlite::ErrorCode;
        matches!(
            self,
            Error::Database(rusqlite::Error::SqliteFailure(e, _))
                if matches!(e.code, ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase)
        )
    }

    /// Convenience constructor for [`Error::NotFound`].
    pub fn not_found(kind: &'static str, id: impl Into<String>) -> Self {
        Self::NotFound {
            kind,
            id: id.into(),
        }
    }

    /// Convenience constructor for [`Error::Invalid`].
    pub fn invalid(field: &'static str, reason: impl Into<String>) -> Self {
        Self::Invalid {
            field,
            reason: reason.into(),
        }
    }
}
