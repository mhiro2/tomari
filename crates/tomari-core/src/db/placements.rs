//! Repository methods for per-application remembered window positions.

use rusqlite::{OptionalExtension, params};

use super::{Database, DecodedRows, PersistedRowCounts, collect_valid_rows};
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

    /// Fetch one remembered position, if configured and decodable. A malformed
    /// stored value is treated as absent so callers can overwrite or remove it.
    pub fn get_window_placement(
        &self,
        bundle_id: &str,
        slot: PlacementSlot,
    ) -> Result<Option<WindowPlacement>> {
        self.with_conn(|conn| {
            let result = conn
                .query_row(
                    "SELECT bundle_id, app_name, slot, frame
                 FROM window_placements
                 WHERE bundle_id = ?1 AND slot = ?2",
                    params![bundle_id, slot.as_str()],
                    decode_placement,
                )
                .optional();
            match result {
                Ok(placement) => Ok(placement),
                Err(rusqlite::Error::FromSqlConversionFailure(_, _, error)) => {
                    tracing::warn!(
                        entity = "window placement",
                        %error,
                        "treating a stored row that does not deserialize as absent"
                    );
                    Ok(None)
                }
                Err(error) => Err(error.into()),
            }
        })
    }

    /// The slots for which a row is stored but cannot be used — its frame does
    /// not parse, or parses to something invalid. [`Self::list_window_placements`]
    /// skips such rows silently so the rest stay usable; this names them so the
    /// UI can offer to replace or forget them instead of showing an empty slot
    /// that a later save then "mysteriously" fills.
    pub fn damaged_window_placement_slots(&self, bundle_id: &str) -> Result<Vec<PlacementSlot>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT slot, frame FROM window_placements WHERE bundle_id = ?1
                 ORDER BY CASE slot WHEN 'primary' THEN 0 ELSE 1 END",
            )?;
            let rows = stmt.query_map([bundle_id], |row| {
                let slot: String = row.get(0)?;
                let raw: String = row.get(1)?;
                Ok((slot, raw))
            })?;
            let mut damaged = Vec::new();
            for row in rows {
                let (slot, raw) = row?;
                let slot = match slot.as_str() {
                    "primary" => PlacementSlot::Primary,
                    "secondary" => PlacementSlot::Secondary,
                    // A row whose slot is unknown cannot be addressed by slot
                    // at all; it is skipped by every read and left alone.
                    _ => continue,
                };
                let usable = serde_json::from_str::<crate::domain::NormalizedRect>(&raw)
                    .is_ok_and(|frame| frame.is_valid());
                if !usable {
                    damaged.push(slot);
                }
            }
            Ok(damaged)
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

    /// Forget one named position, returning whether a row existed. Deleting an
    /// absent position is idempotent.
    pub fn delete_window_placement(&self, bundle_id: &str, slot: PlacementSlot) -> Result<bool> {
        self.with_conn(|conn| {
            let deleted = conn.execute(
                "DELETE FROM window_placements WHERE bundle_id = ?1 AND slot = ?2",
                params![bundle_id, slot.as_str()],
            )?;
            Ok(deleted != 0)
        })
    }
}

/// Read every persisted placement column and classify only malformed slot/frame
/// payloads as damaged rows.
pub(super) fn preflight_window_placements(
    conn: &rusqlite::Connection,
) -> Result<DecodedRows<WindowPlacement>> {
    let mut statement = conn.prepare(
        "SELECT bundle_id, app_name, slot, frame
         FROM window_placements
         ORDER BY bundle_id, slot",
    )?;
    let mut rows = statement.query([])?;
    let mut values = Vec::new();
    let mut counts = PersistedRowCounts::default();

    while let Some(row) = rows.next()? {
        let bundle_id: String = row.get(0)?;
        let app_name: String = row.get(1)?;
        let slot: String = row.get(2)?;
        let frame: String = row.get(3)?;
        counts.stored += 1;

        match decode_placement_values(bundle_id, app_name, slot, frame) {
            Ok(placement) => values.push(placement),
            Err(error) => {
                counts.skipped += 1;
                tracing::warn!(
                    entity = "window placement",
                    %error,
                    "skipping a stored row with a damaged slot or frame"
                );
            }
        }
    }

    Ok(DecodedRows { values, counts })
}

fn decode_placement(row: &rusqlite::Row<'_>) -> rusqlite::Result<WindowPlacement> {
    let bundle_id: String = row.get(0)?;
    let app_name: String = row.get(1)?;
    let slot: String = row.get(2)?;
    let frame: String = row.get(3)?;
    decode_placement_values(bundle_id, app_name, slot, frame).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(error))
    })
}

fn decode_placement_values(
    bundle_id: String,
    app_name: String,
    slot: String,
    frame: String,
) -> Result<WindowPlacement> {
    let slot = match slot.as_str() {
        "primary" => PlacementSlot::Primary,
        "secondary" => PlacementSlot::Secondary,
        other => {
            return Err(Error::invalid("slot", format!("unknown value {other}")));
        }
    };
    let frame: crate::domain::NormalizedRect = serde_json::from_str(&frame)?;
    // Meaning, not just shape: a frame that parses but is non-finite, empty or
    // outside the work area would be refused on write (`save_window_placement`),
    // so reading it back as a placement would recall a window to nowhere. Treat
    // it as a row that does not deserialize — skipped by the list, absent to a
    // point read — so a valid Secondary is not blocked by a damaged Primary and
    // the slot can be overwritten or forgotten.
    if !frame.is_valid() {
        return Err(Error::invalid(
            "frame",
            "stored value is not a valid normalized rectangle",
        ));
    }
    Ok(WindowPlacement {
        application: WindowApplication {
            bundle_id,
            name: app_name,
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

    #[test]
    fn malformed_positions_can_be_replaced_or_deleted() {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO window_placements (bundle_id, app_name, slot, frame)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    "com.example.Editor",
                    "Editor",
                    PlacementSlot::Primary.as_str(),
                    "not-json",
                ],
            )?;
            Ok(())
        })
        .unwrap();

        assert!(
            db.get_window_placement("com.example.Editor", PlacementSlot::Primary)
                .unwrap()
                .is_none()
        );

        let replacement = placement(PlacementSlot::Primary);
        db.save_window_placement(&replacement).unwrap();
        assert_eq!(
            db.get_window_placement("com.example.Editor", PlacementSlot::Primary)
                .unwrap(),
            Some(replacement)
        );

        db.with_conn(|conn| {
            conn.execute(
                "UPDATE window_placements SET frame = ?1
                 WHERE bundle_id = ?2 AND slot = ?3",
                params![
                    "still-not-json",
                    "com.example.Editor",
                    PlacementSlot::Primary.as_str(),
                ],
            )?;
            Ok(())
        })
        .unwrap();
        assert!(
            db.delete_window_placement("com.example.Editor", PlacementSlot::Primary)
                .unwrap()
        );
        assert!(
            db.get_window_placement("com.example.Editor", PlacementSlot::Primary)
                .unwrap()
                .is_none()
        );
        assert!(
            !db.delete_window_placement("com.example.Editor", PlacementSlot::Primary)
                .unwrap()
        );
    }

    #[test]
    fn a_stored_frame_that_parses_but_is_invalid_is_skipped_and_reported() {
        let db = Database::open_in_memory().unwrap();
        db.save_window_placement(&placement(PlacementSlot::Secondary))
            .unwrap();
        // Bypass the write-side validation the way a hand edit or an older
        // build would: a Primary whose frame is outside the work area.
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO window_placements (bundle_id, app_name, slot, frame)
                 VALUES ('com.example.Editor', 'Editor', 'primary', ?1)",
                [r#"{"x":0.9,"y":0.0,"width":0.5,"height":0.5}"#],
            )?;
            Ok(())
        })
        .unwrap();

        // The damaged Primary does not block the valid Secondary …
        let listed = db.list_window_placements("com.example.Editor").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].slot, PlacementSlot::Secondary);
        assert_eq!(
            db.get_window_placement("com.example.Editor", PlacementSlot::Primary)
                .unwrap(),
            None
        );
        // … and is named as damaged, so the UI can offer to replace or forget it.
        assert_eq!(
            db.damaged_window_placement_slots("com.example.Editor")
                .unwrap(),
            vec![PlacementSlot::Primary]
        );
        // Overwriting and forgetting both work on the damaged slot.
        db.save_window_placement(&placement(PlacementSlot::Primary))
            .unwrap();
        assert!(
            db.damaged_window_placement_slots("com.example.Editor")
                .unwrap()
                .is_empty()
        );
        assert!(
            db.delete_window_placement("com.example.Editor", PlacementSlot::Primary)
                .unwrap()
        );
    }
}
