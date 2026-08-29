//! Repository methods for the `meta` table — small app-internal key/value
//! records (e.g. the permission snapshot) that must not live in
//! [`AppSettings`](crate::domain::AppSettings), where they would leak into the
//! settings object the frontend reads and writes.

use rusqlite::{OptionalExtension, params};

use super::{Database, PersistedRowCounts};
use crate::error::Result;

impl Database {
    /// The stored value for `key`, or `None` when it has never been written.
    pub fn get_meta(&self, key: &str) -> Result<Option<String>> {
        self.with_conn(|conn| {
            Ok(conn
                .query_row(
                    "SELECT value FROM meta WHERE key = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()?)
        })
    }

    /// Insert or overwrite the value stored under `key`.
    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
            Ok(())
        })
    }
}

/// Read every metadata column without retaining internal values in the public
/// startup report.
pub(super) fn preflight_meta(conn: &rusqlite::Connection) -> Result<PersistedRowCounts> {
    let mut statement = conn.prepare("SELECT key, value FROM meta ORDER BY key")?;
    let mut rows = statement.query([])?;
    let mut counts = PersistedRowCounts::default();
    while let Some(row) = rows.next()? {
        let _: String = row.get(0)?;
        let _: String = row.get(1)?;
        counts.stored += 1;
    }
    Ok(counts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_key_reads_as_none() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.get_meta("nope").unwrap(), None);
    }

    #[test]
    fn set_then_get_round_trips() {
        let db = Database::open_in_memory().unwrap();
        db.set_meta("k", "v1").unwrap();
        assert_eq!(db.get_meta("k").unwrap().as_deref(), Some("v1"));
    }

    #[test]
    fn set_overwrites_an_existing_value() {
        let db = Database::open_in_memory().unwrap();
        db.set_meta("k", "v1").unwrap();
        db.set_meta("k", "v2").unwrap();
        assert_eq!(db.get_meta("k").unwrap().as_deref(), Some("v2"));
    }

    #[test]
    fn keys_are_independent() {
        let db = Database::open_in_memory().unwrap();
        db.set_meta("a", "1").unwrap();
        db.set_meta("b", "2").unwrap();
        assert_eq!(db.get_meta("a").unwrap().as_deref(), Some("1"));
        assert_eq!(db.get_meta("b").unwrap().as_deref(), Some("2"));
    }
}
