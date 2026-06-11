//! Repository methods for hotkeys and modifier rules.

use rusqlite::{Connection, Row, params};

use super::Database;
use crate::domain::action::AppAction;
use crate::domain::keyboard::{Hotkey, KeySide, ModifierKey, ModifierRule};
use crate::error::{Error, Result};

/// Parse a JSON column into a domain value, mapping serde errors into the
/// `rusqlite` error type so they flow through `query_map`.
fn from_json<T: serde::de::DeserializeOwned>(json: &str) -> rusqlite::Result<T> {
    serde_json::from_str(json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}

fn map_hotkey(row: &Row<'_>) -> rusqlite::Result<Hotkey> {
    let action: AppAction = from_json(&row.get::<_, String>("action")?)?;
    Ok(Hotkey {
        id: row.get("id")?,
        label: row.get("label")?,
        accelerator: row.get("accelerator")?,
        action,
        enabled: row.get::<_, i64>("enabled")? != 0,
    })
}

impl Database {
    pub fn list_hotkeys(&self) -> Result<Vec<Hotkey>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT * FROM hotkeys ORDER BY label")?;
            let rows = stmt.query_map([], map_hotkey)?;
            super::collect_valid_rows(rows, "hotkey")
        })
    }

    pub fn upsert_hotkey(&self, hk: &Hotkey) -> Result<()> {
        self.with_conn(|conn| write_hotkey(conn, hk))
    }

    pub fn delete_hotkey(&self, id: &str) -> Result<()> {
        self.with_conn(|conn| {
            let n = conn.execute("DELETE FROM hotkeys WHERE id = ?1", params![id])?;
            if n == 0 {
                return Err(Error::not_found("hotkey", id));
            }
            Ok(())
        })
    }

    pub fn list_modifier_rules(&self) -> Result<Vec<ModifierRule>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT * FROM modifier_rules ORDER BY label")?;
            let rows = stmt.query_map([], map_modifier_rule)?;
            super::collect_valid_rows(rows, "modifier_rule")
        })
    }

    pub fn upsert_modifier_rule(&self, rule: &ModifierRule) -> Result<()> {
        self.with_conn(|conn| write_modifier_rule(conn, rule))
    }

    pub fn delete_modifier_rule(&self, id: &str) -> Result<()> {
        self.with_conn(|conn| {
            let n = conn.execute("DELETE FROM modifier_rules WHERE id = ?1", params![id])?;
            if n == 0 {
                return Err(Error::not_found("modifier_rule", id));
            }
            Ok(())
        })
    }
}

/// Insert-or-replace a single hotkey on the given connection. Shared by
/// [`Database::upsert_hotkey`] and the bulk transactional import so both write
/// rows identically.
pub(super) fn write_hotkey(conn: &Connection, hk: &Hotkey) -> Result<()> {
    let action = serde_json::to_string(&hk.action)?;
    conn.execute(
        "INSERT INTO hotkeys (id, label, accelerator, action, enabled)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET
            label = excluded.label,
            accelerator = excluded.accelerator,
            action = excluded.action,
            enabled = excluded.enabled",
        params![hk.id, hk.label, hk.accelerator, action, hk.enabled as i64],
    )?;
    Ok(())
}

pub(super) fn write_modifier_rule(conn: &Connection, rule: &ModifierRule) -> Result<()> {
    let modifier = serde_json::to_string(&rule.modifier)?;
    let side = serde_json::to_string(&rule.side)?;
    let remap_to = rule
        .remap_to
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    let tap = serde_json::to_string(&rule.tap)?;
    conn.execute(
        "INSERT INTO modifier_rules
            (id, label, modifier, side, remap_to, hyper, tap, enabled)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(id) DO UPDATE SET
            label = excluded.label,
            modifier = excluded.modifier,
            side = excluded.side,
            remap_to = excluded.remap_to,
            hyper = excluded.hyper,
            tap = excluded.tap,
            enabled = excluded.enabled",
        params![
            rule.id,
            rule.label,
            modifier,
            side,
            remap_to,
            rule.hyper as i64,
            tap,
            rule.enabled as i64
        ],
    )?;
    Ok(())
}

fn map_modifier_rule(row: &Row<'_>) -> rusqlite::Result<ModifierRule> {
    let modifier: ModifierKey = from_json(&row.get::<_, String>("modifier")?)?;
    let side: KeySide = from_json(&row.get::<_, String>("side")?)?;
    let remap_to: Option<ModifierKey> = match row.get::<_, Option<String>>("remap_to")? {
        Some(j) => Some(from_json(&j)?),
        None => None,
    };
    let tap: AppAction = from_json(&row.get::<_, String>("tap")?)?;
    Ok(ModifierRule {
        id: row.get("id")?,
        label: row.get("label")?,
        modifier,
        side,
        remap_to,
        hyper: row.get::<_, i64>("hyper")? != 0,
        tap,
        enabled: row.get::<_, i64>("enabled")? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::window::WindowPreset;

    #[test]
    fn hotkey_round_trip() {
        let db = Database::open_in_memory().unwrap();
        let hk = Hotkey {
            id: "h1".into(),
            label: "Snap left".into(),
            accelerator: "Cmd+Alt+Left".into(),
            action: AppAction::SnapWindow(WindowPreset::LeftHalf),
            enabled: true,
        };
        db.upsert_hotkey(&hk).unwrap();
        assert_eq!(db.list_hotkeys().unwrap(), vec![hk]);
        db.delete_hotkey("h1").unwrap();
        assert!(db.list_hotkeys().unwrap().is_empty());
    }

    #[test]
    fn modifier_rule_round_trip() {
        let db = Database::open_in_memory().unwrap();
        let rule = ModifierRule {
            id: "m1".into(),
            label: "Caps → Ctrl, tap Esc".into(),
            modifier: ModifierKey::CapsLock,
            side: KeySide::Either,
            remap_to: Some(ModifierKey::Control),
            hyper: false,
            tap: AppAction::SendKeystroke("Escape".into()),
            enabled: true,
        };
        db.upsert_modifier_rule(&rule).unwrap();
        assert_eq!(db.list_modifier_rules().unwrap(), vec![rule]);
        db.delete_modifier_rule("m1").unwrap();
        assert!(db.list_modifier_rules().unwrap().is_empty());
    }

    #[test]
    fn side_aware_rule_round_trip() {
        let db = Database::open_in_memory().unwrap();
        let rule = ModifierRule {
            id: "m2".into(),
            label: "Right ⌘ → かな".into(),
            modifier: ModifierKey::Command,
            side: KeySide::Right,
            remap_to: None,
            hyper: false,
            tap: AppAction::SwitchIme(crate::domain::ImeMode::Kana),
            enabled: true,
        };
        db.upsert_modifier_rule(&rule).unwrap();
        assert_eq!(db.list_modifier_rules().unwrap(), vec![rule]);
    }

    #[test]
    fn hyper_rule_round_trip() {
        let db = Database::open_in_memory().unwrap();
        let rule = ModifierRule {
            id: "m3".into(),
            label: "Caps → Hyper".into(),
            modifier: ModifierKey::CapsLock,
            side: KeySide::Either,
            remap_to: None,
            hyper: true,
            tap: AppAction::TogglePanel,
            enabled: true,
        };
        db.upsert_modifier_rule(&rule).unwrap();
        assert_eq!(db.list_modifier_rules().unwrap(), vec![rule]);
    }

    #[test]
    fn a_malformed_row_is_skipped_not_fatal() {
        // One row with broken JSON and one written by a hypothetical newer
        // version (an unknown action variant) must not take down the rows
        // that still deserialize.
        let db = Database::open_in_memory().unwrap();
        let good = Hotkey {
            id: "good".into(),
            label: "Snap left".into(),
            accelerator: "Cmd+Alt+Left".into(),
            action: AppAction::SnapWindow(WindowPreset::LeftHalf),
            enabled: true,
        };
        db.upsert_hotkey(&good).unwrap();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO hotkeys (id, label, accelerator, action, enabled)
                 VALUES ('corrupt', 'Corrupt', 'Cmd+1', 'not json', 1),
                        ('future', 'Future', 'Cmd+2', '{\"type\":\"notYetInvented\"}', 1)",
                [],
            )?;
            Ok(())
        })
        .unwrap();

        assert_eq!(db.list_hotkeys().unwrap(), vec![good]);
    }
}
