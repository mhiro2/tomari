//! Repository methods for the single-row application settings.

use rusqlite::{Connection, OptionalExtension, params};

use super::{Database, PersistedRowCounts, PersistedSettings};
use crate::domain::AppSettings;
use crate::error::{Error, Result};

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

    /// Whether the single settings row has been written yet.
    ///
    /// Presence marks an initialized store even when the user deliberately
    /// cleared every hotkey and rule. Absence alone does not prove a first run;
    /// callers must also verify [`Database::has_persisted_data`] is false.
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

/// Read every settings column without acquiring the database mutex.
pub(super) fn preflight_settings(
    conn: &Connection,
) -> Result<(PersistedSettings, PersistedRowCounts)> {
    let mut statement = conn.prepare("SELECT id, data FROM settings ORDER BY id")?;
    let mut rows = statement.query([])?;
    let mut state = PersistedSettings::Missing;
    let mut counts = PersistedRowCounts::default();

    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        let raw: String = row.get(1)?;
        counts.stored += 1;
        if id != 1 {
            return Err(Error::invalid(
                "settings.id",
                format!("expected canonical id 1, found {id}"),
            ));
        }
        match serde_json::from_str(&raw) {
            Ok(settings) => state = PersistedSettings::Ready(settings),
            Err(error) => {
                counts.skipped += 1;
                state = PersistedSettings::UnreadableJson {
                    message: error.to_string(),
                };
            }
        }
    }

    Ok((state, counts))
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
