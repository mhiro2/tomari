//! SQLite persistence layer.
//!
//! A single [`Database`] owns the connection behind a mutex so it can be stored
//! in shared application state and used from multiple threads. Repository
//! methods are implemented across the submodules of this module.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::Connection;

use crate::error::{Error, Result};

mod keyboard;
mod settings;

/// The current schema version. Bump this and add a branch in [`migrate`] when
/// the schema changes.
const SCHEMA_VERSION: i32 = 1;

/// A thread-safe handle to the on-disk SQLite database.
pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    /// Open (creating if needed) the database at `path` and run migrations.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::from_connection(conn)
    }

    /// Open an in-memory database — handy for tests.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::from_connection(conn)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.migrate()?;
        Ok(db)
    }

    /// Run lock-guarded work against the connection.
    fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let guard = self.conn.lock().expect("database mutex poisoned");
        f(&guard)
    }

    fn migrate(&self) -> Result<()> {
        self.with_conn(|conn| {
            let version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
            if version >= SCHEMA_VERSION {
                return Ok(());
            }

            // Create the schema and stamp the version in one transaction so a
            // failure (e.g. a crash mid-setup) rolls back cleanly instead of
            // leaving half-created tables that break the next launch. `PRAGMA
            // user_version` is part of the transaction and reverts on rollback.
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(SCHEMA)
                .map_err(|e| Error::Migration(e.to_string()))?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            tx.commit()?;
            Ok(())
        })
    }
}

/// Collect mapped rows, skipping any row whose stored JSON no longer
/// deserializes (corruption, or a value written by a newer app version).
/// One bad row must not take the whole list — and with it every hotkey or
/// rule — down. Real query errors still propagate.
fn collect_valid_rows<T>(
    rows: impl Iterator<Item = rusqlite::Result<T>>,
    entity: &str,
) -> Result<Vec<T>> {
    let mut out = Vec::new();
    for row in rows {
        match row {
            Ok(value) => out.push(value),
            Err(rusqlite::Error::FromSqlConversionFailure(_, _, e)) => {
                tracing::warn!(entity, error = %e, "skipping a stored row that does not deserialize");
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(out)
}

/// The full database schema. `hyper` defaults to `0` so callers can omit it and
/// get the "not a hyper key" behaviour.
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS hotkeys (
    id          TEXT    PRIMARY KEY,
    label       TEXT    NOT NULL,
    accelerator TEXT    NOT NULL,
    action      TEXT    NOT NULL,
    enabled     INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS modifier_rules (
    id        TEXT    PRIMARY KEY,
    label     TEXT    NOT NULL,
    modifier  TEXT    NOT NULL,
    side      TEXT    NOT NULL,
    remap_to  TEXT,
    tap       TEXT    NOT NULL,
    enabled   INTEGER NOT NULL,
    hyper     INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS settings (
    id   INTEGER PRIMARY KEY CHECK (id = 1),
    data TEXT    NOT NULL
);
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_and_migrates_in_memory() {
        let db = Database::open_in_memory().expect("open");
        // Re-running migrate is idempotent.
        db.migrate().expect("re-migrate");
    }

    #[test]
    fn fresh_database_has_the_full_schema() {
        // The consolidated migration must create every table along with the
        // `hyper` column. Each list query touches those columns, so a dropped
        // column or table would surface here.
        let db = Database::open_in_memory().expect("open");
        assert!(db.list_hotkeys().expect("hotkeys").is_empty());
        assert!(db.list_modifier_rules().expect("modifier rules").is_empty());
    }
}
