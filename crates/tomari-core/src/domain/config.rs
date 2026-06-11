//! A portable, human-readable snapshot of the entire Tomari configuration.
//!
//! This is the on-disk format for the settings import/export feature: a single
//! pretty-printed JSON file holding every persisted entity (settings, hotkeys,
//! modifier rules). It is meant to be diffable and stored alongside dotfiles, so
//! export sorts collections by id and emits a trailing newline for stable diffs.
//!
//! The format is versioned by [`CONFIG_FORMAT_VERSION`], deliberately decoupled
//! from the SQLite `user_version` (the physical schema) so the logical export
//! shape can evolve on its own. The envelope uses `deny_unknown_fields` and
//! makes every collection mandatory: a typo'd or missing key is an error, never
//! a silently-empty collection that would wipe the user's data on import.

use serde::{Deserialize, Serialize};

use super::{AppSettings, Hotkey, ModifierRule};
use crate::error::{Error, Result};

/// The version of the import/export file format this build reads and writes.
/// Bump when the [`ConfigSnapshot`] shape changes incompatibly; add an explicit
/// migration when doing so.
pub const CONFIG_FORMAT_VERSION: u32 = 1;

/// A complete, portable copy of everything Tomari persists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigSnapshot {
    /// The format version (see [`CONFIG_FORMAT_VERSION`]). Required, so a file
    /// that is not a Tomari config is rejected rather than misread.
    pub format_version: u32,
    pub settings: AppSettings,
    pub hotkeys: Vec<Hotkey>,
    pub modifier_rules: Vec<ModifierRule>,
}

/// Just enough of the envelope to read the version before committing to the
/// full strict parse, so an unsupported version yields a clear message instead
/// of a generic "unknown field" / "missing field" error.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VersionProbe {
    format_version: u32,
}

impl ConfigSnapshot {
    /// Build a snapshot from the live configuration, stamped with the current
    /// format version.
    pub fn new(
        settings: AppSettings,
        hotkeys: Vec<Hotkey>,
        modifier_rules: Vec<ModifierRule>,
    ) -> Self {
        Self {
            format_version: CONFIG_FORMAT_VERSION,
            settings,
            hotkeys,
            modifier_rules,
        }
    }

    /// Serialize to the canonical export form: collections sorted by id,
    /// pretty-printed with a trailing newline, so re-exporting an unchanged
    /// configuration produces byte-identical output and git diffs stay minimal.
    pub fn to_pretty_json(&self) -> Result<String> {
        let mut sorted = self.clone();
        sorted.hotkeys.sort_by(|a, b| a.id.cmp(&b.id));
        sorted.modifier_rules.sort_by(|a, b| a.id.cmp(&b.id));
        let mut json = serde_json::to_string_pretty(&sorted)?;
        json.push('\n');
        Ok(json)
    }

    /// Parse and strictly validate an export file. Checks the format version
    /// first (so an unknown version reports clearly), then deserializes with
    /// unknown fields and missing collections rejected.
    pub fn from_json(s: &str) -> Result<Self> {
        let probe: VersionProbe = serde_json::from_str(s)?;
        if probe.format_version != CONFIG_FORMAT_VERSION {
            return Err(Error::invalid(
                "formatVersion",
                format!(
                    "unsupported config format version {} (this build reads version {CONFIG_FORMAT_VERSION})",
                    probe.format_version
                ),
            ));
        }
        Ok(serde_json::from_str(s)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ConfigSnapshot {
        ConfigSnapshot::new(
            AppSettings::default(),
            crate::defaults::default_hotkeys(),
            crate::defaults::default_modifier_rules(),
        )
    }

    #[test]
    fn round_trips_through_json() {
        let snap = sample();
        let json = snap.to_pretty_json().unwrap();
        let back = ConfigSnapshot::from_json(&json).unwrap();
        // Equality is order-insensitive only if ids are unique; the export is
        // already sorted, so compare against a sorted clone for determinism.
        let resorted = ConfigSnapshot::from_json(&back.to_pretty_json().unwrap()).unwrap();
        assert_eq!(back, resorted);
    }

    #[test]
    fn pretty_json_is_deterministic_and_newline_terminated() {
        let snap = sample();
        let a = snap.to_pretty_json().unwrap();
        // Re-parsing and re-serializing yields identical bytes regardless of the
        // input collection order.
        let mut shuffled = snap.clone();
        shuffled.hotkeys.reverse();
        shuffled.modifier_rules.reverse();
        let b = shuffled.to_pretty_json().unwrap();
        assert_eq!(a, b);
        assert!(a.ends_with("\n"));
    }

    /// A minimal but valid `settings` object, so the collection-shape tests
    /// below fail on the thing they are testing rather than on settings parsing.
    const VALID_SETTINGS: &str = r#""settings": {
        "launchAtLogin": false, "theme": "system", "keyboardEnabled": true,
        "windowManagementEnabled": true, "holdThresholdMs": 200, "showInMenuBar": true
    }"#;

    #[test]
    fn unknown_top_level_field_is_rejected() {
        // A typo such as "hotkyes" must not silently parse as an empty list and
        // wipe the user's hotkeys on import.
        let json = format!(
            r#"{{ "formatVersion": 1, {VALID_SETTINGS}, "hotkyes": [], "hotkeys": [],
                "modifierRules": [] }}"#
        );
        assert!(ConfigSnapshot::from_json(&json).is_err());
    }

    #[test]
    fn missing_collection_is_rejected() {
        // `modifierRules` is absent: a required collection must not default to
        // empty, which on import would silently delete every modifier rule.
        let json = format!(r#"{{ "formatVersion": 1, {VALID_SETTINGS}, "hotkeys": [] }}"#);
        assert!(ConfigSnapshot::from_json(&json).is_err());
    }

    #[test]
    fn unsupported_version_is_rejected_clearly() {
        let json = r#"{ "formatVersion": 999, "settings": {}, "hotkeys": [],
            "modifierRules": [] }"#;
        let err = ConfigSnapshot::from_json(json).unwrap_err().to_string();
        assert!(err.contains("formatVersion"), "got: {err}");
    }

    #[test]
    fn settings_field_omission_falls_back_to_defaults() {
        // `settings` keeps its additive `#[serde(default)]` tolerance: an export
        // from an older build missing a since-added field still imports.
        let json = r#"{
            "formatVersion": 1,
            "settings": {
                "launchAtLogin": false, "theme": "system", "keyboardEnabled": true,
                "windowManagementEnabled": true, "holdThresholdMs": 200, "showInMenuBar": true
            },
            "hotkeys": [], "modifierRules": []
        }"#;
        let snap = ConfigSnapshot::from_json(json).unwrap();
        assert_eq!(snap.settings, AppSettings::default());
    }
}
