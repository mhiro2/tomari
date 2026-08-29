//! Repository methods for hotkeys and modifier rules.

use rusqlite::{Connection, params};

use super::{Database, DecodedRows, PersistedRowCounts};
use crate::domain::action::AppAction;
use crate::domain::keyboard::{Hotkey, KeySide, ModifierKey, ModifierRule};
use crate::error::{Error, Result};

impl Database {
    pub fn list_hotkeys(&self) -> Result<Vec<Hotkey>> {
        self.with_conn(|conn| Ok(read_hotkeys(conn)?.values))
    }

    /// Total stored hotkey rows, whether or not they still decode. Paired with
    /// [`Database::list_hotkeys`] (which silently skips undecodable rows) to tell
    /// whether any rows were dropped.
    pub fn count_hotkeys(&self) -> Result<usize> {
        self.count_rows("hotkeys")
    }

    /// Total stored modifier-rule rows, whether or not they still decode — the
    /// counterpart to [`Database::count_hotkeys`] for [`Database::list_modifier_rules`].
    pub fn count_modifier_rules(&self) -> Result<usize> {
        self.count_rows("modifier_rules")
    }

    pub fn upsert_hotkey(&self, hk: &Hotkey) -> Result<()> {
        self.with_conn(|conn| write_hotkey(conn, hk))
    }

    /// Replace one existing hotkey, allowing validation to canonicalize its
    /// primary key without leaving the raw-ID row behind.
    pub fn replace_hotkey(&self, previous_id: &str, hk: &Hotkey) -> Result<()> {
        self.with_conn(|conn| replace_hotkey(conn, previous_id, hk))
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
        self.with_conn(|conn| Ok(read_modifier_rules(conn)?.values))
    }

    pub fn upsert_modifier_rule(&self, rule: &ModifierRule) -> Result<()> {
        self.with_conn(|conn| write_modifier_rule(conn, rule))
    }

    /// Replace one existing modifier rule, including an ID canonicalization.
    pub fn replace_modifier_rule(&self, previous_id: &str, rule: &ModifierRule) -> Result<()> {
        self.with_conn(|conn| replace_modifier_rule(conn, previous_id, rule))
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

/// Insert-or-replace a single hotkey on the given connection.
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

fn replace_hotkey(conn: &Connection, previous_id: &str, hk: &Hotkey) -> Result<()> {
    let action = serde_json::to_string(&hk.action)?;
    let changed = conn.execute(
        "UPDATE hotkeys
         SET id = ?1, label = ?2, accelerator = ?3, action = ?4, enabled = ?5
         WHERE id = ?6",
        params![
            hk.id,
            hk.label,
            hk.accelerator,
            action,
            hk.enabled as i64,
            previous_id
        ],
    )?;
    if changed == 0 {
        return Err(Error::not_found("hotkey", previous_id));
    }
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

fn replace_modifier_rule(conn: &Connection, previous_id: &str, rule: &ModifierRule) -> Result<()> {
    let modifier = serde_json::to_string(&rule.modifier)?;
    let side = serde_json::to_string(&rule.side)?;
    let remap_to = rule
        .remap_to
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    let tap = serde_json::to_string(&rule.tap)?;
    let changed = conn.execute(
        "UPDATE modifier_rules
         SET id = ?1, label = ?2, modifier = ?3, side = ?4,
             remap_to = ?5, hyper = ?6, tap = ?7, enabled = ?8
         WHERE id = ?9",
        params![
            rule.id,
            rule.label,
            modifier,
            side,
            remap_to,
            rule.hyper as i64,
            tap,
            rule.enabled as i64,
            previous_id
        ],
    )?;
    if changed == 0 {
        return Err(Error::not_found("modifier_rule", previous_id));
    }
    Ok(())
}

/// Read every persisted hotkey column from an existing connection.
pub(super) fn read_hotkeys(conn: &Connection) -> Result<DecodedRows<Hotkey>> {
    let mut statement = conn.prepare(
        "SELECT id, label, accelerator, action, enabled
         FROM hotkeys
         ORDER BY label, id",
    )?;
    let mut rows = statement.query([])?;
    let mut values = Vec::new();
    let mut counts = PersistedRowCounts::default();

    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let label: String = row.get(1)?;
        let accelerator: String = row.get(2)?;
        let action: String = row.get(3)?;
        let enabled: i64 = row.get(4)?;
        counts.stored += 1;

        match serde_json::from_str(&action) {
            Ok(action) => values.push(Hotkey {
                id,
                label,
                accelerator,
                action,
                enabled: enabled != 0,
            }),
            Err(error) => {
                counts.skipped += 1;
                tracing::warn!(
                    entity = "hotkey",
                    row_id = id,
                    %error,
                    "skipping a stored row whose JSON does not deserialize"
                );
            }
        }
    }

    Ok(DecodedRows { values, counts })
}

/// Read every persisted modifier-rule column from an existing connection.
pub(super) fn read_modifier_rules(conn: &Connection) -> Result<DecodedRows<ModifierRule>> {
    let mut statement = conn.prepare(
        "SELECT id, label, modifier, side, remap_to, hyper, tap, enabled
         FROM modifier_rules
         ORDER BY label, id",
    )?;
    let mut rows = statement.query([])?;
    let mut values = Vec::new();
    let mut counts = PersistedRowCounts::default();

    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let label: String = row.get(1)?;
        let modifier: String = row.get(2)?;
        let side: String = row.get(3)?;
        let remap_to: Option<String> = row.get(4)?;
        let hyper: i64 = row.get(5)?;
        let tap: String = row.get(6)?;
        let enabled: i64 = row.get(7)?;
        counts.stored += 1;

        let decoded = (|| -> serde_json::Result<ModifierRule> {
            Ok(ModifierRule {
                id: id.clone(),
                label,
                modifier: serde_json::from_str::<ModifierKey>(&modifier)?,
                side: serde_json::from_str::<KeySide>(&side)?,
                remap_to: remap_to.as_deref().map(serde_json::from_str).transpose()?,
                hyper: hyper != 0,
                tap: serde_json::from_str::<AppAction>(&tap)?,
                enabled: enabled != 0,
            })
        })();
        match decoded {
            Ok(rule) => values.push(rule),
            Err(error) => {
                counts.skipped += 1;
                tracing::warn!(
                    entity = "modifier rule",
                    row_id = id,
                    %error,
                    "skipping a stored row whose JSON does not deserialize"
                );
            }
        }
    }

    Ok(DecodedRows { values, counts })
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
    fn hotkey_replacement_canonicalizes_the_primary_key_without_leaving_a_copy() {
        let db = Database::open_in_memory().unwrap();
        let raw = Hotkey {
            id: " row ".into(),
            label: "Raw".into(),
            accelerator: "Cmd+1".into(),
            action: AppAction::NoOp,
            enabled: false,
        };
        db.upsert_hotkey(&raw).unwrap();
        let canonical = Hotkey {
            id: "row".into(),
            label: "Canonical".into(),
            accelerator: "Cmd+2".into(),
            action: AppAction::TogglePanel,
            enabled: true,
        };

        db.replace_hotkey(&raw.id, &canonical).unwrap();

        assert_eq!(db.list_hotkeys().unwrap(), vec![canonical.clone()]);
        assert_eq!(db.count_hotkeys().unwrap(), 1);

        db.replace_hotkey(&canonical.id, &raw).unwrap();
        assert_eq!(db.list_hotkeys().unwrap(), vec![raw]);
        assert_eq!(db.count_hotkeys().unwrap(), 1);
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
    fn modifier_replacement_canonicalizes_the_primary_key_without_leaving_a_copy() {
        let db = Database::open_in_memory().unwrap();
        let raw = ModifierRule {
            id: " rule ".into(),
            label: "Raw".into(),
            modifier: ModifierKey::Control,
            side: KeySide::Left,
            remap_to: None,
            hyper: false,
            tap: AppAction::NoOp,
            enabled: false,
        };
        db.upsert_modifier_rule(&raw).unwrap();
        let canonical = ModifierRule {
            id: "rule".into(),
            label: "Canonical".into(),
            modifier: ModifierKey::Option,
            side: KeySide::Right,
            remap_to: Some(ModifierKey::Control),
            hyper: false,
            tap: AppAction::TogglePanel,
            enabled: true,
        };

        db.replace_modifier_rule(&raw.id, &canonical).unwrap();

        assert_eq!(db.list_modifier_rules().unwrap(), vec![canonical.clone()]);
        assert_eq!(db.count_modifier_rules().unwrap(), 1);

        db.replace_modifier_rule(&canonical.id, &raw).unwrap();
        assert_eq!(db.list_modifier_rules().unwrap(), vec![raw]);
        assert_eq!(db.count_modifier_rules().unwrap(), 1);
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

    #[test]
    fn count_exceeds_decoded_count_when_rows_are_corrupt() {
        // The raw row count must stay ahead of what the list decodes, so a
        // caller can tell that rows were silently skipped and surface the loss.
        let db = Database::open_in_memory().unwrap();
        db.upsert_hotkey(&Hotkey {
            id: "good".into(),
            label: "Snap left".into(),
            accelerator: "Cmd+Alt+Left".into(),
            action: AppAction::SnapWindow(WindowPreset::LeftHalf),
            enabled: true,
        })
        .unwrap();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO hotkeys (id, label, accelerator, action, enabled)
                 VALUES ('corrupt', 'Corrupt', 'Cmd+1', 'not json', 1)",
                [],
            )?;
            Ok(())
        })
        .unwrap();

        assert_eq!(
            db.list_hotkeys().unwrap().len(),
            1,
            "the corrupt row is skipped"
        );
        assert_eq!(db.count_hotkeys().unwrap(), 2, "but it is still counted");
    }
}
