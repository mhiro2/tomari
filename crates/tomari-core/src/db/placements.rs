//! Repository methods for per-application remembered window positions.

use rusqlite::{OptionalExtension, params};

use super::{Database, collect_valid_rows};
use crate::domain::{PlacementSlot, WindowApplication, WindowPlacement};
use crate::error::{Error, Result};

impl Database {
    /// Fetch both remembered positions for one application in slot order.
    pub fn list_window_placements(&self, bundle_id: &str) -> Result<Vec<WindowPlacement>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT bundle_id, app_name, slot, frame
                 FROM window_placements
                 WHERE bundle_id = ?1
                 ORDER BY CASE slot WHEN 'primary' THEN 0 ELSE 1 END",
            )?;
            let rows = stmt.query_map([bundle_id], decode_placement)?;
            collect_valid_rows(rows, "window placement")
        })
    }

    /// Fetch one remembered position, if configured.
    pub fn get_window_placement(
        &self,
        bundle_id: &str,
        slot: PlacementSlot,
    ) -> Result<Option<WindowPlacement>> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT bundle_id, app_name, slot, frame
                 FROM window_placements
                 WHERE bundle_id = ?1 AND slot = ?2",
                params![bundle_id, slot.as_str()],
                decode_placement,
            )
            .optional()
            .map_err(Into::into)
        })
    }

    /// Insert or replace one application's named position.
    pub fn save_window_placement(&self, placement: &WindowPlacement) -> Result<()> {
        if placement.application.bundle_id.trim().is_empty() {
            return Err(Error::invalid("bundle_id", "must not be empty"));
        }
        if !placement.frame.is_valid() {
            return Err(Error::invalid(
                "frame",
                "must be finite, non-empty, and inside the work area",
            ));
        }
        let frame = serde_json::to_string(&placement.frame)?;
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO window_placements (bundle_id, app_name, slot, frame)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(bundle_id, slot) DO UPDATE SET
                   app_name = excluded.app_name,
                   frame = excluded.frame",
                params![
                    placement.application.bundle_id,
                    placement.application.name,
                    placement.slot.as_str(),
                    frame,
                ],
            )?;
            Ok(())
        })
    }

    /// Forget one named position. Deleting an absent position is idempotent.
    pub fn delete_window_placement(&self, bundle_id: &str, slot: PlacementSlot) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "DELETE FROM window_placements WHERE bundle_id = ?1 AND slot = ?2",
                params![bundle_id, slot.as_str()],
            )?;
            Ok(())
        })
    }
}

fn decode_placement(row: &rusqlite::Row<'_>) -> rusqlite::Result<WindowPlacement> {
    let slot: String = row.get(2)?;
    let slot = match slot.as_str() {
        "primary" => PlacementSlot::Primary,
        "secondary" => PlacementSlot::Secondary,
        other => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                Box::new(Error::invalid("slot", format!("unknown value {other}"))),
            ));
        }
    };
    let raw: String = row.get(3)?;
    let frame = serde_json::from_str(&raw).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(WindowPlacement {
        application: WindowApplication {
            bundle_id: row.get(0)?,
            name: row.get(1)?,
        },
        slot,
        frame,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::NormalizedRect;

    fn placement(slot: PlacementSlot) -> WindowPlacement {
        WindowPlacement {
            application: WindowApplication {
                bundle_id: "com.example.Editor".into(),
                name: "Editor".into(),
            },
            slot,
            frame: NormalizedRect::new(0.1, 0.2, 0.6, 0.7),
        }
    }

    #[test]
    fn saves_lists_replaces_and_deletes_positions() {
        let db = Database::open_in_memory().unwrap();
        db.save_window_placement(&placement(PlacementSlot::Secondary))
            .unwrap();
        db.save_window_placement(&placement(PlacementSlot::Primary))
            .unwrap();

        let listed = db.list_window_placements("com.example.Editor").unwrap();
        assert_eq!(
            listed.iter().map(|p| p.slot).collect::<Vec<_>>(),
            PlacementSlot::ALL
        );

        let mut replacement = placement(PlacementSlot::Primary);
        replacement.frame = NormalizedRect::new(0.0, 0.0, 1.0, 1.0);
        db.save_window_placement(&replacement).unwrap();
        assert_eq!(
            db.get_window_placement("com.example.Editor", PlacementSlot::Primary)
                .unwrap(),
            Some(replacement)
        );

        db.delete_window_placement("com.example.Editor", PlacementSlot::Primary)
            .unwrap();
        assert!(
            db.get_window_placement("com.example.Editor", PlacementSlot::Primary)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn refuses_invalid_positions() {
        let db = Database::open_in_memory().unwrap();
        let mut bad = placement(PlacementSlot::Primary);
        bad.frame = NormalizedRect::new(0.8, 0.0, 0.5, 1.0);
        assert!(matches!(
            db.save_window_placement(&bad),
            Err(Error::Invalid { .. })
        ));
    }
}
