//! Whole-configuration export and import against the database.
//!
//! These back the settings import/export feature. Callers serialize the layer
//! above to JSON (see [`crate::domain::config`]); here we only move rows in and
//! out of SQLite. The replace path is a single transaction so a partial write
//! can never leave the database with, say, the new hotkeys but the old rules.
//!
//! Consistency between the lists and the settings row is the caller's
//! responsibility: export and import run while the app holds its config-mutation
//! lock, so no interactive save can interleave between the individual reads or
//! between the delete and the inserts here.

use std::path::Path;

use super::Database;
use super::keyboard::{write_hotkey, write_modifier_rule};
use super::settings::write_settings;
use crate::domain::ConfigSnapshot;
use crate::error::{Error, Result};

/// The outcome of reading the full configuration out of the database.
pub struct ExportResult {
    pub snapshot: ConfigSnapshot,
    /// How many stored rows failed to deserialize and were therefore left out
    /// of the snapshot. The list APIs skip such rows to keep the app usable,
    /// but an export silently dropping them would be a lossy backup, so the
    /// caller surfaces this as a warning.
    pub omitted: usize,
}

impl Database {
    /// Read the entire configuration into a [`ConfigSnapshot`], reporting how
    /// many corrupt rows were skipped.
    pub fn export_snapshot(&self) -> Result<ExportResult> {
        let settings = self.get_settings()?;
        let hotkeys = self.list_hotkeys()?;
        let modifier_rules = self.list_modifier_rules()?;

        let stored = self.with_conn(|conn| {
            let count = |table: &str| -> Result<usize> {
                let n: i64 =
                    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?;
                Ok(n as usize)
            };
            Ok(count("hotkeys")? + count("modifier_rules")?)
        })?;
        let parsed = hotkeys.len() + modifier_rules.len();

        Ok(ExportResult {
            snapshot: ConfigSnapshot::new(settings, hotkeys, modifier_rules),
            omitted: stored.saturating_sub(parsed),
        })
    }

    /// Write a complete, standalone copy of the database to `path` via
    /// `VACUUM INTO`. Unlike [`Database::export_snapshot`], this copies every row
    /// verbatim at the SQL layer — including rows the app can no longer
    /// deserialize — so a pre-import backup loses nothing. `path` must not exist
    /// and must be on a filesystem SQLite can write to.
    pub fn backup_to(&self, path: &Path) -> Result<()> {
        let dest = path
            .to_str()
            .ok_or_else(|| Error::NonUtf8Path(path.to_path_buf()))?;
        self.with_conn(|conn| {
            conn.execute("VACUUM INTO ?1", rusqlite::params![dest])?;
            Ok(())
        })
    }

    /// Atomically replace the entire configuration with `snapshot`: clear the
    /// entity tables and the settings row, then write the snapshot's rows, all in
    /// one transaction. The snapshot is assumed already validated by the caller
    /// (unique ids).
    pub fn replace_with_snapshot(&self, snapshot: &ConfigSnapshot) -> Result<()> {
        self.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            conn.execute("DELETE FROM hotkeys", [])?;
            conn.execute("DELETE FROM modifier_rules", [])?;
            for hk in &snapshot.hotkeys {
                write_hotkey(conn, hk)?;
            }
            for rule in &snapshot.modifier_rules {
                write_modifier_rule(conn, rule)?;
            }
            write_settings(conn, &snapshot.settings)?;
            tx.commit()?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::action::AppAction;
    use crate::domain::keyboard::Hotkey;

    fn hotkey(id: &str) -> Hotkey {
        Hotkey {
            id: id.into(),
            label: format!("hk {id}"),
            accelerator: "Cmd+Shift+K".into(),
            action: AppAction::TogglePanel,
            enabled: true,
        }
    }

    #[test]
    fn export_then_replace_round_trips() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_hotkey(&hotkey("a")).unwrap();
        db.upsert_hotkey(&hotkey("b")).unwrap();

        let exported = db.export_snapshot().unwrap();
        assert_eq!(exported.omitted, 0);
        assert_eq!(exported.snapshot.hotkeys.len(), 2);

        // Replacing with a snapshot holding a single, different hotkey must drop
        // everything else.
        let replacement = ConfigSnapshot::new(
            exported.snapshot.settings.clone(),
            vec![hotkey("only")],
            vec![],
        );
        db.replace_with_snapshot(&replacement).unwrap();

        let after = db.export_snapshot().unwrap().snapshot;
        assert_eq!(after.hotkeys, vec![hotkey("only")]);
        assert!(after.modifier_rules.is_empty());
    }

    #[test]
    fn replace_clears_every_table_before_writing() {
        // Replacing with an empty snapshot must wipe the entity tables — this is
        // the DELETE-then-insert path running as one transaction (the same
        // `unchecked_transaction` + `commit` shape the migration uses).
        let db = Database::open_in_memory().unwrap();
        db.upsert_hotkey(&hotkey("a")).unwrap();

        let empty = ConfigSnapshot::new(db.get_settings().unwrap(), vec![], vec![]);
        db.replace_with_snapshot(&empty).unwrap();

        let after = db.export_snapshot().unwrap().snapshot;
        assert!(after.hotkeys.is_empty());
        assert!(after.modifier_rules.is_empty());
    }

    #[test]
    fn omitted_counts_corrupt_rows() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_hotkey(&hotkey("good")).unwrap();
        // Inject a row whose action JSON does not deserialize.
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO hotkeys (id, label, accelerator, action, enabled)
                 VALUES ('bad', 'Bad', 'Cmd+1', 'not json', 1)",
                [],
            )?;
            Ok(())
        })
        .unwrap();

        let exported = db.export_snapshot().unwrap();
        assert_eq!(exported.snapshot.hotkeys.len(), 1);
        assert_eq!(exported.omitted, 1);
    }

    #[test]
    fn backup_to_copies_every_row_including_corrupt_ones() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_hotkey(&hotkey("good")).unwrap();
        // A row the app cannot deserialize must still survive a raw backup.
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO hotkeys (id, label, accelerator, action, enabled)
                 VALUES ('bad', 'Bad', 'Cmd+1', 'not json', 1)",
                [],
            )?;
            Ok(())
        })
        .unwrap();

        let dir = std::env::temp_dir();
        let dest = dir.join(format!(
            "tomari-test-backup-{}.sqlite",
            crate::clock::now_millis()
        ));
        db.backup_to(&dest).unwrap();

        // The backup is a standalone database with both rows present at the SQL
        // layer (the corrupt one is just text the row-copy preserves verbatim).
        let restored = Database::open(&dest).unwrap();
        let count: i64 = restored
            .with_conn(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM hotkeys", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(count, 2);

        let _ = std::fs::remove_file(&dest);
    }
}
