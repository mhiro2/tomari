//! Error type shared across Tomari crates.

/// Result alias used throughout the core crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can originate from the core layer.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    /// SQLite's integrity pragmas can report damage as a successful query row
    /// rather than a `SQLITE_CORRUPT` error. Keep that diagnostic distinct from
    /// ordinary database failures so startup can quarantine the damaged file.
    #[error("database integrity check failed: {0}")]
    DatabaseIntegrity(String),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("schema migration failed: {0}")]
    Migration(String),

    /// One migration step's SQL failed. Keeps the SQLite error — code and
    /// extended code included — rather than flattening it into text, so a
    /// database that turns out corrupt *during* migration is recognised as
    /// such (and quarantined at launch) instead of becoming a fatal error the
    /// next launch hits again.
    #[error("schema migration to version {step} failed: {source}")]
    MigrationStep {
        step: i32,
        #[source]
        source: rusqlite::Error,
    },

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
        match self {
            Error::DatabaseIntegrity(_) => true,
            Error::Database(e) | Error::MigrationStep { source: e, .. } => sqlite_corruption(e),
            _ => false,
        }
    }

    /// The SQLite error underneath, if there is one — a migration step's
    /// failure as much as a plain database error.
    pub fn sqlite_error(&self) -> Option<&rusqlite::Error> {
        match self {
            Error::Database(e) | Error::MigrationStep { source: e, .. } => Some(e),
            _ => None,
        }
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

/// Whether a SQLite error says the file itself is unreadable.
fn sqlite_corruption(e: &rusqlite::Error) -> bool {
    use rusqlite::ErrorCode;
    matches!(
        e,
        rusqlite::Error::SqliteFailure(e, _)
            if matches!(e.code, ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sqlite(code: rusqlite::ErrorCode, extended: i32) -> rusqlite::Error {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code,
                extended_code: extended,
            },
            None,
        )
    }

    #[test]
    fn corruption_is_recognised_inside_a_migration_step_too() {
        let during_migration = Error::MigrationStep {
            step: 3,
            source: sqlite(rusqlite::ErrorCode::DatabaseCorrupt, 11),
        };
        assert!(during_migration.is_database_corruption());
        assert_eq!(
            during_migration
                .sqlite_error()
                .and_then(|e| e.sqlite_error_code()),
            Some(rusqlite::ErrorCode::DatabaseCorrupt)
        );
        let plain = Error::Database(sqlite(rusqlite::ErrorCode::NotADatabase, 26));
        assert!(plain.is_database_corruption());
        let integrity = Error::DatabaseIntegrity("page 2 is malformed".into());
        assert!(integrity.is_database_corruption());
        assert!(integrity.sqlite_error().is_none());
        // A step that failed for any other reason is a real migration failure,
        // not a damaged file.
        let bad_sql = Error::MigrationStep {
            step: 1,
            source: sqlite(rusqlite::ErrorCode::Unknown, 1),
        };
        assert!(!bad_sql.is_database_corruption());
        assert!(!Error::Migration("version".into()).is_database_corruption());
    }
}
