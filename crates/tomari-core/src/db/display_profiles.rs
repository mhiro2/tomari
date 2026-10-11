//! Bounded saved display configurations, independent of live display state.

use rusqlite::{Connection, params};

use super::{Database, DecodedRows, PersistedRowCounts};
use crate::{DisplayProfile, Error, MAX_DISPLAY_PROFILES, Result};

impl Database {
    pub fn list_display_profiles(&self) -> Result<Vec<DisplayProfile>> {
        self.with_conn(|conn| {
            let rows = read_display_profiles(conn)?;
            if rows.counts.skipped != 0 {
                return Err(Error::invalid(
                    "display profiles",
                    "contains unreadable saved profiles",
                ));
            }
            Ok(rows.values)
        })
    }

    pub fn save_display_profile(&self, profile: &DisplayProfile) -> Result<()> {
        self.with_conn(|conn| write_display_profile(conn, profile))
    }

    pub fn delete_display_profile(&self, id: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM display_profiles WHERE id = ?1", [id])?;
            Ok(())
        })
    }

    pub fn clear_display_profiles(&self) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM display_profiles", [])?;
            Ok(())
        })
    }
}

pub(super) fn write_display_profile(conn: &Connection, profile: &DisplayProfile) -> Result<()> {
    profile.validate()?;
    let other: i64 = conn.query_row(
        "SELECT count(*) FROM display_profiles WHERE id != ?1",
        [&profile.id],
        |row| row.get(0),
    )?;
    if other >= MAX_DISPLAY_PROFILES as i64 {
        return Err(Error::invalid(
            "display profiles",
            "too many saved profiles",
        ));
    }
    conn.execute(
        "INSERT INTO display_profiles (id, data) VALUES (?1, ?2)
         ON CONFLICT(id) DO UPDATE SET data = excluded.data",
        params![profile.id, serde_json::to_string(profile)?],
    )?;
    Ok(())
}

pub(super) fn read_display_profiles(conn: &Connection) -> Result<DecodedRows<DisplayProfile>> {
    let mut statement = conn.prepare("SELECT id, data FROM display_profiles ORDER BY id")?;
    let mut rows = statement.query([])?;
    let mut values = Vec::new();
    let mut counts = PersistedRowCounts::default();
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let data: String = row.get(1)?;
        counts.stored += 1;
        match serde_json::from_str::<DisplayProfile>(&data) {
            Ok(profile) if profile.id == id && profile.validate().is_ok() => values.push(profile),
            _ => counts.skipped += 1,
        }
    }
    values.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    Ok(DecodedRows { values, counts })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_are_bounded_and_replaceable_at_capacity() {
        let db = Database::open_in_memory().unwrap();
        let mut profile = crate::domain::display::sample();
        for index in 0..MAX_DISPLAY_PROFILES {
            profile.id = index.to_string();
            db.save_display_profile(&profile).unwrap();
        }
        profile.name = "Updated".into();
        db.save_display_profile(&profile).unwrap();
        profile.id = "one-too-many".into();
        assert!(db.save_display_profile(&profile).is_err());
        assert_eq!(
            db.list_display_profiles().unwrap().len(),
            MAX_DISPLAY_PROFILES
        );
        db.delete_display_profile("0").unwrap();
        db.save_display_profile(&profile).unwrap();
        db.clear_display_profiles().unwrap();
        assert!(db.list_display_profiles().unwrap().is_empty());
    }

    #[test]
    fn malformed_rows_are_not_silently_exported_or_mistaken_for_an_empty_store() {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(|conn| {
            conn.execute("INSERT INTO display_profiles VALUES ('bad', '{}')", [])?;
            let rows = read_display_profiles(conn)?;
            assert_eq!(rows.counts.stored, 1);
            assert_eq!(rows.counts.skipped, 1);
            Ok(())
        })
        .unwrap();
        assert!(db.list_display_profiles().is_err());
        assert!(db.has_persisted_data().unwrap());
        assert!(!db.preflight_persisted_state().unwrap().is_pristine());
        db.clear_display_profiles().unwrap();
        assert!(db.list_display_profiles().unwrap().is_empty());
    }
}
