//! Atomic portable-configuration snapshots, replacement, and one-step undo.

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use super::{Database, PersistedSettings, keyboard, placements, settings};
use crate::{CONFIGURATION_BACKUP_FORMAT_VERSION, ConfigurationBackup, Error, Result};

const PREVIOUS_CONFIGURATION_KEY: &str = "previous_configuration_backup";

impl Database {
    /// Read all portable data under a single SQLite snapshot. An incomplete
    /// table is an error rather than a deceptively successful partial backup.
    pub fn configuration_backup(&self) -> Result<ConfigurationBackup> {
        self.with_conn(|conn| {
            let tx = Transaction::new_unchecked(conn, TransactionBehavior::Deferred)?;
            let backup = read_configuration(&tx)?;
            tx.commit()?;
            Ok(backup)
        })
    }

    /// Read the local undo snapshot, never included in exported documents.
    pub fn read_previous_configuration(&self) -> Result<Option<ConfigurationBackup>> {
        self.with_conn(read_previous_configuration)
    }

    /// Replace every portable table atomically, only while the saved state
    /// still matches the preview. The caller validates keyboard semantics
    /// before entering this storage boundary.
    ///
    /// An import remembers the complete previous configuration in the same
    /// transaction. Undo consumes that record and refuses a stale replacement
    /// if another import has changed the undo target since it was previewed.
    /// Every other metadata record remains untouched.
    pub fn replace_configuration(
        &self,
        expected: &ConfigurationBackup,
        replacement: &ConfigurationBackup,
        remember_previous: bool,
    ) -> Result<()> {
        expected.validate()?;
        // Serialization also enforces the document byte bound for callers
        // that constructed a value directly instead of parsing an import.
        let _ = replacement.to_json()?;
        self.with_conn(|conn| {
            let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
            let current = read_configuration(&tx)?;
            if &current != expected {
                return Err(Error::ConfigurationChanged);
            }
            if !remember_previous && read_previous_configuration(&tx)?.as_ref() != Some(replacement)
            {
                return Err(Error::ConfigurationChanged);
            }

            tx.execute("DELETE FROM settings", [])?;
            tx.execute("DELETE FROM hotkeys", [])?;
            tx.execute("DELETE FROM modifier_rules", [])?;
            tx.execute("DELETE FROM window_placements", [])?;
            settings::write_settings(&tx, &replacement.settings)?;
            for hotkey in &replacement.hotkeys {
                keyboard::write_hotkey(&tx, hotkey)?;
            }
            for rule in &replacement.modifier_rules {
                keyboard::write_modifier_rule(&tx, rule)?;
            }
            for placement in &replacement.window_placements {
                placements::write_window_placement(&tx, placement)?;
            }

            if remember_previous {
                tx.execute(
                    "INSERT INTO meta (key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![PREVIOUS_CONFIGURATION_KEY, current.to_json()?],
                )?;
            } else {
                tx.execute(
                    "DELETE FROM meta WHERE key = ?1",
                    [PREVIOUS_CONFIGURATION_KEY],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
    }
}

fn read_previous_configuration(conn: &Connection) -> Result<Option<ConfigurationBackup>> {
    let json: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = ?1",
            [PREVIOUS_CONFIGURATION_KEY],
            |row| row.get(0),
        )
        .optional()?;
    json.map(|json| ConfigurationBackup::from_json(&json))
        .transpose()
}

fn read_configuration(conn: &Connection) -> Result<ConfigurationBackup> {
    let (settings, settings_counts) = settings::preflight_settings(conn)?;
    let PersistedSettings::Ready(settings) = settings else {
        return Err(Error::invalid(
            "configuration backup",
            "settings are absent or unreadable",
        ));
    };
    let hotkeys = keyboard::read_hotkeys(conn)?;
    let modifier_rules = keyboard::read_modifier_rules(conn)?;
    let window_placements = placements::preflight_window_placements(conn)?;
    if settings_counts.stored != 1
        || settings_counts.skipped != 0
        || hotkeys.counts.skipped != 0
        || modifier_rules.counts.skipped != 0
        || window_placements.counts.skipped != 0
    {
        return Err(Error::invalid(
            "configuration backup",
            "contains unreadable stored rows; repair them before exporting or importing",
        ));
    }
    let backup = ConfigurationBackup {
        format_version: CONFIGURATION_BACKUP_FORMAT_VERSION,
        settings,
        hotkeys: hotkeys.values,
        modifier_rules: modifier_rules.values,
        window_placements: window_placements.values,
    };
    backup.validate()?;
    Ok(backup)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AppSettings, Language, NormalizedRect, PlacementSlot, WindowApplication, WindowPlacement,
    };

    fn database() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.seed_defaults(
            &crate::defaults::default_hotkeys(),
            &crate::defaults::default_modifier_rules(),
            &AppSettings::default(),
        )
        .unwrap();
        db.save_window_placement(&WindowPlacement {
            application: WindowApplication {
                bundle_id: "com.example.Editor".into(),
                name: "Editor".into(),
            },
            slot: PlacementSlot::Primary,
            frame: NormalizedRect::new(0.0, 0.0, 0.5, 1.0),
        })
        .unwrap();
        db.set_meta("permission_snapshot", "machine-local").unwrap();
        db
    }

    fn changed(mut backup: ConfigurationBackup) -> ConfigurationBackup {
        backup.settings.language = Language::Ja;
        backup.hotkeys.truncate(1);
        backup.hotkeys[0].label = "Imported shortcut".into();
        backup.modifier_rules.clear();
        backup.window_placements[0].frame.width = 1.0;
        backup
    }

    #[test]
    fn replacement_updates_every_table_and_undo_preserves_machine_metadata() {
        let db = database();
        let before = db.configuration_backup().unwrap();
        let replacement = changed(before.clone());
        assert!(db.read_previous_configuration().unwrap().is_none());

        db.replace_configuration(&before, &replacement, true)
            .unwrap();
        assert_eq!(db.configuration_backup().unwrap(), replacement);
        assert_eq!(
            db.read_previous_configuration().unwrap(),
            Some(before.clone())
        );
        assert_eq!(
            db.get_meta("permission_snapshot").unwrap().as_deref(),
            Some("machine-local")
        );
        assert!(
            !db.configuration_backup()
                .unwrap()
                .to_json()
                .unwrap()
                .contains("machine-local")
        );

        db.replace_configuration(&replacement, &before, false)
            .unwrap();
        assert_eq!(db.configuration_backup().unwrap(), before);
        assert!(db.read_previous_configuration().unwrap().is_none());
        assert_eq!(
            db.get_meta("permission_snapshot").unwrap().as_deref(),
            Some("machine-local")
        );
    }

    #[test]
    fn a_changed_current_configuration_refuses_the_preview_without_mutation() {
        let db = database();
        let before = db.configuration_backup().unwrap();
        let replacement = changed(before.clone());
        let mut settings = before.settings.clone();
        settings.show_in_menu_bar = false;
        db.save_settings(&settings).unwrap();
        let current = db.configuration_backup().unwrap();

        assert!(matches!(
            db.replace_configuration(&before, &replacement, true),
            Err(Error::ConfigurationChanged)
        ));
        assert_eq!(db.configuration_backup().unwrap(), current);
        assert!(db.read_previous_configuration().unwrap().is_none());
    }

    #[test]
    fn a_changed_undo_target_refuses_the_old_preview_even_when_current_is_unchanged() {
        let db = database();
        let before = db.configuration_backup().unwrap();
        let replacement = changed(before.clone());
        db.replace_configuration(&before, &replacement, true)
            .unwrap();
        db.replace_configuration(&replacement, &replacement, true)
            .unwrap();

        assert!(matches!(
            db.replace_configuration(&replacement, &before, false),
            Err(Error::ConfigurationChanged)
        ));
        assert_eq!(db.configuration_backup().unwrap(), replacement);
        assert_eq!(db.read_previous_configuration().unwrap(), Some(replacement));
    }

    #[test]
    fn a_late_write_failure_rolls_back_all_tables_and_preserves_previous_undo() {
        let db = database();
        let before = db.configuration_backup().unwrap();
        let replacement = changed(before.clone());
        db.replace_configuration(&before, &replacement, true)
            .unwrap();
        db.with_conn(|conn| {
            conn.execute_batch("CREATE TRIGGER fail_backup_meta BEFORE UPDATE ON meta BEGIN SELECT RAISE(ABORT, 'injected failure'); END")?;
            Ok(())
        }).unwrap();

        assert!(
            db.replace_configuration(&replacement, &before, true)
                .is_err()
        );
        assert_eq!(db.configuration_backup().unwrap(), replacement);
        assert_eq!(db.read_previous_configuration().unwrap(), Some(before));
    }

    #[test]
    fn a_failed_undo_preserves_all_data_and_the_undo_record() {
        let db = database();
        let before = db.configuration_backup().unwrap();
        let replacement = changed(before.clone());
        db.replace_configuration(&before, &replacement, true)
            .unwrap();
        db.with_conn(|conn| {
            conn.execute_batch("CREATE TRIGGER fail_backup_delete BEFORE DELETE ON meta BEGIN SELECT RAISE(ABORT, 'injected failure'); END")?;
            Ok(())
        }).unwrap();

        assert!(
            db.replace_configuration(&replacement, &before, false)
                .is_err()
        );
        assert_eq!(db.configuration_backup().unwrap(), replacement);
        assert_eq!(db.read_previous_configuration().unwrap(), Some(before));
    }

    #[test]
    fn export_refuses_each_kind_of_skipped_or_missing_row() {
        for sql in [
            "DELETE FROM settings",
            "UPDATE settings SET data = 'broken'",
            "UPDATE hotkeys SET action = 'broken'",
            "UPDATE modifier_rules SET tap = 'broken'",
            "UPDATE window_placements SET frame = 'broken'",
        ] {
            let db = database();
            db.with_conn(|conn| {
                conn.execute_batch(sql)?;
                Ok(())
            })
            .unwrap();
            assert!(db.configuration_backup().is_err(), "{sql}");
        }
    }

    #[test]
    fn undo_survives_reopening_and_never_becomes_part_of_the_export() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.db");
        let db = Database::open(&path).unwrap();
        db.seed_defaults(
            &crate::defaults::default_hotkeys(),
            &[],
            &AppSettings::default(),
        )
        .unwrap();
        let before = db.configuration_backup().unwrap();
        let mut replacement = before.clone();
        replacement.settings.language = Language::Ja;
        db.replace_configuration(&before, &replacement, true)
            .unwrap();
        drop(db);

        let reopened = Database::open(&path).unwrap();
        assert_eq!(
            reopened.read_previous_configuration().unwrap(),
            Some(before.clone())
        );
        let export = reopened.configuration_backup().unwrap().to_json().unwrap();
        assert!(!export.contains(PREVIOUS_CONFIGURATION_KEY));
        reopened
            .replace_configuration(&replacement, &before, false)
            .unwrap();
        assert_eq!(reopened.configuration_backup().unwrap(), before);
    }
}
