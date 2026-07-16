//! SQLite persistence layer.
//!
//! A single [`Database`] owns the connection behind a mutex so it can be stored
//! in shared application state and used from multiple threads. Repository
//! methods are implemented across the submodules of this module.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::Connection;

use crate::domain::AppSettings;
use crate::domain::keyboard::{Hotkey, ModifierRule};
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
        // Wait for a lock held by another connection (e.g. a concurrent writer
        // under WAL) instead of failing immediately with `SQLITE_BUSY`.
        conn.busy_timeout(Duration::from_secs(5))?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.migrate()?;
        Ok(db)
    }

    /// Run lock-guarded work against the connection.
    ///
    /// Recovers the guard from a poisoned mutex rather than panicking: a panic
    /// while a query was running poisons the lock, and under the release
    /// profile's `panic = "abort"` propagating that would silently terminate a
    /// resident app. The connection itself stays usable (a panicking statement
    /// does not corrupt it), so taking the guard back lets later queries proceed.
    fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let guard = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        f(&guard)
    }

    /// Total row count of `table`. `table` is only ever an in-crate string
    /// literal (never user input), so interpolating it carries no injection
    /// risk. Used to compare against a decoded list and spot silently-skipped
    /// (undecodable) rows.
    fn count_rows(&self, table: &str) -> Result<usize> {
        self.with_conn(|conn| {
            let n: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })?;
            Ok(n as usize)
        })
    }

    /// Seed the first-run defaults — hotkeys, modifier rules and the settings
    /// row — in a single transaction, so a failure part-way through rolls back
    /// and never leaves a half-populated database.
    ///
    /// Only call this once a real first run has been confirmed (i.e.
    /// [`settings_exist`](Self::settings_exist) returned `Ok(false)`). A *read
    /// failure* while checking must not be treated as a first run: the settings
    /// row may exist but be momentarily unreadable, and seeding then would
    /// overwrite a real user's configuration. Writing all rows atomically also
    /// guarantees the settings row (the first-run marker) only appears if the
    /// accompanying hotkeys and rules landed too.
    pub fn seed_defaults(
        &self,
        hotkeys: &[Hotkey],
        rules: &[ModifierRule],
        settings: &AppSettings,
    ) -> Result<()> {
        self.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            for hk in hotkeys {
                keyboard::write_hotkey(&tx, hk)?;
            }
            for rule in rules {
                keyboard::write_modifier_rule(&tx, rule)?;
            }
            settings::write_settings(&tx, settings)?;
            tx.commit()?;
            Ok(())
        })
    }

    fn migrate(&self) -> Result<()> {
        self.with_conn(|conn| {
            let version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
            if version > SCHEMA_VERSION {
                // The database was written by a newer version of the app and
                // uses a schema this binary does not understand. Opening it
                // anyway risks silent data loss or corruption, so refuse
                // outright rather than proceeding as if `version ==
                // SCHEMA_VERSION`.
                return Err(Error::Migration(format!(
                    "database schema version {version} is newer than this app supports \
                     (expected at most {SCHEMA_VERSION}); please update the app"
                )));
            }
            if version == SCHEMA_VERSION {
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

    #[test]
    fn refuses_to_open_a_database_from_a_newer_schema_version() {
        // A database stamped with a `user_version` ahead of what this binary
        // knows about was written by a newer app version. Opening it as if it
        // matched the current schema could silently corrupt or drop data, so
        // it must be rejected instead.
        let db = Database::open_in_memory().expect("open");
        db.with_conn(|conn| {
            conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)?;
            Ok(())
        })
        .expect("bump user_version");

        let err = db.migrate().expect_err("newer schema must be rejected");
        assert!(matches!(err, Error::Migration(_)));
    }

    #[test]
    fn seed_defaults_writes_every_row_and_marks_first_run() {
        use crate::domain::action::AppAction;
        use crate::domain::keyboard::{KeySide, ModifierKey};
        use crate::domain::window::WindowPreset;

        let db = Database::open_in_memory().expect("open");
        assert!(!db.settings_exist().unwrap(), "starts uninitialized");

        let hotkeys = vec![Hotkey {
            id: "h1".into(),
            label: "Snap left".into(),
            accelerator: "Cmd+Alt+Left".into(),
            action: AppAction::SnapWindow(WindowPreset::LeftHalf),
            enabled: true,
        }];
        let rules = vec![ModifierRule {
            id: "m1".into(),
            label: "Caps → Ctrl".into(),
            modifier: ModifierKey::CapsLock,
            side: KeySide::Either,
            remap_to: Some(ModifierKey::Control),
            hyper: false,
            tap: AppAction::SendKeystroke("Escape".into()),
            enabled: true,
        }];
        let settings = AppSettings::default();

        db.seed_defaults(&hotkeys, &rules, &settings).expect("seed");

        assert!(db.settings_exist().unwrap(), "settings row now present");
        assert_eq!(db.list_hotkeys().unwrap(), hotkeys);
        assert_eq!(db.list_modifier_rules().unwrap(), rules);
        assert_eq!(db.get_settings().unwrap(), settings);
    }

    #[test]
    fn seed_defaults_is_atomic_and_idempotent() {
        use crate::domain::action::AppAction;
        use crate::domain::window::WindowPreset;

        let db = Database::open_in_memory().expect("open");
        // An empty batch still writes the settings row (the first-run marker),
        // so a later launch correctly sees the database as initialized.
        db.seed_defaults(&[], &[], &AppSettings::default())
            .expect("seed empty");
        assert!(db.settings_exist().unwrap());
        assert!(db.list_hotkeys().unwrap().is_empty());

        // Re-seeding re-runs the same upserts by primary key, so it never
        // duplicates rows — the writes are keyed, not blindly appended.
        let hotkeys = vec![Hotkey {
            id: "h1".into(),
            label: "Snap left".into(),
            accelerator: "Cmd+Alt+Left".into(),
            action: AppAction::SnapWindow(WindowPreset::LeftHalf),
            enabled: true,
        }];
        db.seed_defaults(&hotkeys, &[], &AppSettings::default())
            .expect("re-seed");
        db.seed_defaults(&hotkeys, &[], &AppSettings::default())
            .expect("re-seed again");
        assert_eq!(db.list_hotkeys().unwrap(), hotkeys);
    }

    #[test]
    fn on_disk_database_survives_close_and_reopen() {
        // In-memory tests never exercise the real file-backed path (WAL
        // checkpointing, the file actually persisting to disk). Open a file
        // under a temp directory, write through one connection, drop it, then
        // reopen the same path and confirm the data is still there.
        use crate::domain::action::AppAction;
        use crate::domain::keyboard::Hotkey;
        use crate::domain::window::WindowPreset;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tomari.sqlite3");

        let hk = Hotkey {
            id: "h1".into(),
            label: "Snap left".into(),
            accelerator: "Cmd+Alt+Left".into(),
            action: AppAction::SnapWindow(WindowPreset::LeftHalf),
            enabled: true,
        };

        {
            let db = Database::open(&path).expect("open on-disk db");
            db.upsert_hotkey(&hk).expect("write hotkey");
        } // `db` and its connection are dropped here.

        let db = Database::open(&path).expect("reopen on-disk db");
        assert_eq!(db.list_hotkeys().expect("hotkeys"), vec![hk]);
    }
}
