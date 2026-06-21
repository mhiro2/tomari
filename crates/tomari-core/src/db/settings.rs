//! Repository methods for the single-row application settings.

use rusqlite::{Connection, OptionalExtension, params};

use super::Database;
use crate::domain::AppSettings;
use crate::error::Result;

impl Database {
    /// Fetch settings, falling back to [`AppSettings::default`] if none stored.
    pub fn get_settings(&self) -> Result<AppSettings> {
        self.with_conn(|conn| {
            let raw: Option<String> = conn
                .query_row("SELECT data FROM settings WHERE id = 1", [], |row| {
                    row.get(0)
                })
                .optional()?;
            match raw {
                Some(json) => Ok(serde_json::from_str(&json)?),
                None => Ok(AppSettings::default()),
            }
        })
    }

    /// Whether the single settings row has been written yet. Used to tell a
    /// first run (no row) apart from a user who has deliberately cleared all of
    /// their hotkeys and sequences (row present), so defaults are not re-seeded.
    pub fn settings_exist(&self) -> Result<bool> {
        self.with_conn(|conn| {
            let exists = conn
                .query_row("SELECT 1 FROM settings WHERE id = 1", [], |_| Ok(()))
                .optional()?
                .is_some();
            Ok(exists)
        })
    }

    /// Persist settings into the single settings row.
    pub fn save_settings(&self, settings: &AppSettings) -> Result<()> {
        self.with_conn(|conn| write_settings(conn, settings))
    }
}

/// Write the single settings row on the given connection.
pub(super) fn write_settings(conn: &Connection, settings: &AppSettings) -> Result<()> {
    let json = serde_json::to_string(settings)?;
    conn.execute(
        "INSERT INTO settings (id, data) VALUES (1, ?1)
         ON CONFLICT(id) DO UPDATE SET data = excluded.data",
        params![json],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_empty() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.get_settings().unwrap(), AppSettings::default());
    }

    #[test]
    fn settings_row_presence_marks_first_run() {
        let db = Database::open_in_memory().unwrap();
        assert!(!db.settings_exist().unwrap());
        db.save_settings(&AppSettings::default()).unwrap();
        assert!(db.settings_exist().unwrap());
    }

    #[test]
    fn save_and_reload() {
        let db = Database::open_in_memory().unwrap();
        let s = AppSettings {
            command_ime_switch_enabled: false,
            launch_at_login: true,
            ..Default::default()
        };
        db.save_settings(&s).unwrap();
        assert_eq!(db.get_settings().unwrap(), s);
    }
}
