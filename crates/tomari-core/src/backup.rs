//! Portable, versioned user configuration. Runtime state, permissions, machine
//! ownership records and internal metadata never enter this document.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{AppSettings, Error, Hotkey, ModifierRule, Result, WindowPlacement};

pub const CONFIGURATION_BACKUP_FORMAT_VERSION: u32 = 1;
pub const MAX_CONFIGURATION_BACKUP_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_CONFIGURATION_BACKUP_RECORDS: usize = 1000;

/// The complete portable configuration. Keyboard semantics are validated by
/// `tomari-keyboard` at the application boundary; this crate owns the document
/// shape, resource limits, settings bounds and storage-key invariants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationBackup {
    pub format_version: u32,
    pub settings: AppSettings,
    pub hotkeys: Vec<Hotkey>,
    pub modifier_rules: Vec<ModifierRule>,
    pub window_placements: Vec<WindowPlacement>,
}

impl ConfigurationBackup {
    /// Parse only this format's complete known shape. Persisted domain types
    /// deliberately accept old database fields/defaults, but an imported file
    /// must not silently lose unknown fields or invent omitted preferences.
    pub fn from_json(json: &str) -> Result<Self> {
        check_size(json.len())?;
        // Deserialize directly first: serde rejects repeated known fields,
        // whereas parsing through Value alone would retain only the last one.
        let backup: Self = serde_json::from_str(json)?;
        let mut input: Value = serde_json::from_str(json)?;
        // `remapTo` is the domain's optional nullable field. None is normally
        // omitted on serialization; an explicit null has the same meaning.
        if let Some(rules) = input.get_mut("modifierRules").and_then(Value::as_array_mut) {
            for rule in rules {
                if let Some(rule) = rule.as_object_mut()
                    && rule.get("remapTo").is_some_and(Value::is_null)
                {
                    rule.remove("remapTo");
                }
            }
        }
        if !same_shape(&input, &serde_json::to_value(&backup)?) {
            return Err(Error::invalid(
                "configuration backup",
                "contains unknown or missing fields",
            ));
        }
        backup.validate()?;
        Ok(backup)
    }

    pub fn to_json(&self) -> Result<String> {
        self.validate()?;
        let json = serde_json::to_string_pretty(self)?;
        check_size(json.len())?;
        Ok(json)
    }

    /// Check the invariants shared by import, export and atomic replacement.
    /// No repair or canonicalization is performed here.
    pub fn validate(&self) -> Result<()> {
        if self.format_version != CONFIGURATION_BACKUP_FORMAT_VERSION {
            return Err(Error::invalid(
                "formatVersion",
                format!(
                    "unsupported configuration backup version {}",
                    self.format_version
                ),
            ));
        }
        self.settings.validate()?;
        for (field, count) in [
            ("hotkeys", self.hotkeys.len()),
            ("modifierRules", self.modifier_rules.len()),
            ("windowPlacements", self.window_placements.len()),
        ] {
            if count > MAX_CONFIGURATION_BACKUP_RECORDS {
                return Err(Error::invalid(
                    field,
                    format!("must contain at most {MAX_CONFIGURATION_BACKUP_RECORDS} records"),
                ));
            }
        }
        unique_ids(self.hotkeys.iter().map(|row| row.id.as_str()), "hotkeys.id")?;
        unique_ids(
            self.modifier_rules.iter().map(|row| row.id.as_str()),
            "modifierRules.id",
        )?;
        let mut slots = HashSet::new();
        for placement in &self.window_placements {
            if placement.application.bundle_id.trim().is_empty() {
                return Err(Error::invalid(
                    "windowPlacements.bundleId",
                    "must not be empty",
                ));
            }
            if !placement.frame.is_valid() {
                return Err(Error::invalid(
                    "windowPlacements.frame",
                    "must be finite, non-empty, and inside the work area",
                ));
            }
            if !slots.insert((&placement.application.bundle_id, placement.slot)) {
                return Err(Error::invalid(
                    "windowPlacements",
                    "duplicate application and slot",
                ));
            }
        }
        Ok(())
    }
}

fn check_size(bytes: usize) -> Result<()> {
    if bytes > MAX_CONFIGURATION_BACKUP_BYTES {
        return Err(Error::invalid(
            "configuration backup",
            "must not exceed 2 MiB",
        ));
    }
    Ok(())
}

fn unique_ids<'a>(ids: impl Iterator<Item = &'a str>, field: &'static str) -> Result<()> {
    let mut seen = HashSet::new();
    for id in ids {
        if !seen.insert(id) {
            return Err(Error::invalid(field, "duplicate identifier"));
        }
    }
    Ok(())
}

fn same_shape(input: &Value, serialized: &Value) -> bool {
    match (input, serialized) {
        (Value::Object(input), Value::Object(serialized)) => {
            input.len() == serialized.len()
                && input.iter().all(|(key, value)| {
                    serialized
                        .get(key)
                        .is_some_and(|other| same_shape(value, other))
                })
        }
        (Value::Array(input), Value::Array(serialized)) => {
            input.len() == serialized.len()
                && input
                    .iter()
                    .zip(serialized)
                    .all(|(left, right)| same_shape(left, right))
        }
        // Deserialization already checked scalar types and values. In
        // particular, 0 and 0.0 are equivalent valid rectangle coordinates.
        (Value::Null, Value::Null)
        | (Value::Bool(_), Value::Bool(_))
        | (Value::Number(_), Value::Number(_))
        | (Value::String(_), Value::String(_)) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NormalizedRect, PlacementSlot, WindowApplication};

    pub(crate) fn sample() -> ConfigurationBackup {
        ConfigurationBackup {
            format_version: CONFIGURATION_BACKUP_FORMAT_VERSION,
            settings: AppSettings::default(),
            hotkeys: crate::defaults::default_hotkeys(),
            modifier_rules: crate::defaults::default_modifier_rules(),
            window_placements: vec![WindowPlacement {
                application: WindowApplication {
                    bundle_id: "com.example.Editor".into(),
                    name: "Editor".into(),
                },
                slot: PlacementSlot::Primary,
                frame: NormalizedRect::new(0.0, 0.0, 0.5, 1.0),
            }],
        }
    }

    #[test]
    fn portable_document_round_trips_without_runtime_or_metadata_fields() {
        let backup = sample();
        assert_eq!(
            ConfigurationBackup::from_json(&backup.to_json().unwrap()).unwrap(),
            backup
        );
        let value = serde_json::to_value(&backup).unwrap();
        let keys: Vec<_> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "formatVersion",
                "hotkeys",
                "modifierRules",
                "settings",
                "windowPlacements"
            ]
        );
    }

    #[test]
    fn every_object_boundary_rejects_unknown_fields() {
        for path in [
            "",
            "/settings",
            "/hotkeys/0",
            "/hotkeys/0/action",
            "/modifierRules/0",
            "/modifierRules/0/tap",
            "/windowPlacements/0",
            "/windowPlacements/0/application",
            "/windowPlacements/0/frame",
        ] {
            let mut value = serde_json::to_value(sample()).unwrap();
            value
                .pointer_mut(path)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("unexpected".into(), true.into());
            assert!(
                ConfigurationBackup::from_json(&value.to_string()).is_err(),
                "{path}"
            );
        }
    }

    #[test]
    fn missing_defaulted_fields_are_not_invented_for_an_import() {
        for (path, field) in [
            ("/settings", "language"),
            ("/settings", "commandImeSwitchEnabled"),
            ("/modifierRules/0", "hyper"),
        ] {
            let mut value = serde_json::to_value(sample()).unwrap();
            value
                .pointer_mut(path)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(
                ConfigurationBackup::from_json(&value.to_string()).is_err(),
                "{field}"
            );
        }
    }

    #[test]
    fn duplicate_json_keys_and_unknown_enums_are_rejected() {
        let json = sample().to_json().unwrap();
        let repeated = json.replacen(
            "\"formatVersion\": 1",
            "\"formatVersion\": 1, \"formatVersion\": 1",
            1,
        );
        assert!(ConfigurationBackup::from_json(&repeated).is_err());
        let repeated = json.replacen(
            "\"enabled\": true",
            "\"enabled\": true, \"enabled\": false",
            1,
        );
        assert!(ConfigurationBackup::from_json(&repeated).is_err());
        let mut value = serde_json::to_value(sample()).unwrap();
        value["modifierRules"][0]["modifier"] = "unknownModifier".into();
        assert!(ConfigurationBackup::from_json(&value.to_string()).is_err());
    }

    #[test]
    fn numeric_coordinate_spelling_and_optional_null_are_accepted() {
        let mut backup = sample();
        backup.modifier_rules[0].remap_to = None;
        let mut value = serde_json::to_value(&backup).unwrap();
        value["windowPlacements"][0]["frame"]["x"] = 0.into();
        value["modifierRules"][0]["remapTo"] = Value::Null;
        assert_eq!(
            ConfigurationBackup::from_json(&value.to_string()).unwrap(),
            backup
        );
    }

    #[test]
    fn version_sizes_duplicate_keys_and_invalid_frames_are_rejected() {
        let mut backup = sample();
        backup.format_version = 2;
        assert!(backup.validate().is_err());
        assert!(
            ConfigurationBackup::from_json(&" ".repeat(MAX_CONFIGURATION_BACKUP_BYTES + 1))
                .is_err()
        );
        let mut backup = sample();
        backup.hotkeys = vec![backup.hotkeys[0].clone(); MAX_CONFIGURATION_BACKUP_RECORDS + 1];
        assert!(backup.validate().is_err());
        let mut backup = sample();
        backup.hotkeys.push(backup.hotkeys[0].clone());
        assert!(backup.validate().is_err());
        let mut backup = sample();
        backup.modifier_rules.push(backup.modifier_rules[0].clone());
        assert!(backup.validate().is_err());
        let mut backup = sample();
        backup
            .window_placements
            .push(backup.window_placements[0].clone());
        assert!(backup.validate().is_err());
        let mut backup = sample();
        backup.window_placements[0].frame.width = 2.0;
        assert!(backup.validate().is_err());
        backup.window_placements[0].frame.width = f64::NAN;
        assert!(backup.to_json().is_err());
        let mut backup = sample();
        backup.settings.menu_bar_auto_collapse_secs = crate::MAX_AUTO_COLLAPSE_SECS + 1;
        assert!(backup.validate().is_err());
    }
}
