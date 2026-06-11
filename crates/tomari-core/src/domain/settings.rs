//! Application-wide user settings.

use serde::{Deserialize, Serialize};

/// Preferred color scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Theme {
    System,
    Light,
    Dark,
}

/// Preferred UI language. `System` follows the OS locale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Language {
    #[default]
    System,
    En,
    Ja,
}

/// The set of user-configurable preferences, persisted as a single row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    /// Start Tomari automatically at login.
    pub launch_at_login: bool,
    pub theme: Theme,
    /// UI language for the panel and tray menu.
    #[serde(default)]
    pub language: Language,
    /// Master switches for each feature area.
    pub keyboard_enabled: bool,
    pub window_management_enabled: bool,
    /// Allow external processes — launchers like Raycast or Alfred, via the
    /// `tomari://` URL scheme — to invoke window-placement actions (snap, move
    /// to display, undo, toggle panel). These act on the focused window and are
    /// undoable, so it ships on during the canary. Input-synthesis actions
    /// (keystrokes, IME) are deliberately *not* covered here and will live
    /// behind a separate, default-off switch when added.
    #[serde(default = "default_external_window_actions_enabled")]
    pub external_window_actions_enabled: bool,
    /// How long (ms) a modifier must be held before it counts as a *hold*
    /// rather than a *tap*.
    pub hold_threshold_ms: u64,
    /// Show the icon in the macOS menu bar.
    pub show_in_menu_bar: bool,
    /// Snap the focused window by dragging it (no modifier) to a screen edge or
    /// corner: a preview appears at the edge and the window snaps on release.
    #[serde(default)]
    pub drag_to_snap_enabled: bool,
}

/// Range the UI slider exposes for the tap/hold threshold (ms); enforced on
/// the backend too so a hand-edited database or a future UI change cannot
/// store a value that breaks tap detection (`0` makes every tap a hold).
pub const HOLD_THRESHOLD_MS_RANGE: std::ops::RangeInclusive<u64> = 100..=500;

impl AppSettings {
    /// Clamp numeric settings into the ranges the UI offers. Run on every save
    /// *and* on load, so neither the frontend nor a hand-edited (or stale)
    /// database can drive the engines with out-of-range values.
    pub fn sanitize(&mut self) {
        self.hold_threshold_ms = self.hold_threshold_ms.clamp(
            *HOLD_THRESHOLD_MS_RANGE.start(),
            *HOLD_THRESHOLD_MS_RANGE.end(),
        );
    }
}

fn default_external_window_actions_enabled() -> bool {
    true
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            launch_at_login: false,
            theme: Theme::System,
            language: Language::System,
            keyboard_enabled: true,
            window_management_enabled: true,
            external_window_actions_enabled: default_external_window_actions_enabled(),
            hold_threshold_ms: 200,
            show_in_menu_bar: true,
            drag_to_snap_enabled: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_clamps_threshold_into_range() {
        let mut s = AppSettings {
            hold_threshold_ms: 0,
            ..AppSettings::default()
        };
        s.sanitize();
        assert_eq!(s.hold_threshold_ms, *HOLD_THRESHOLD_MS_RANGE.start());

        let mut s = AppSettings {
            hold_threshold_ms: 10_000,
            ..AppSettings::default()
        };
        s.sanitize();
        assert_eq!(s.hold_threshold_ms, *HOLD_THRESHOLD_MS_RANGE.end());
    }

    #[test]
    fn sanitize_leaves_in_range_values_untouched() {
        let mut s = AppSettings {
            hold_threshold_ms: 250,
            ..AppSettings::default()
        };
        s.sanitize();
        assert_eq!(s.hold_threshold_ms, 250);
    }

    #[test]
    fn legacy_json_without_optional_fields_fills_defaults() {
        // A settings row written by a build that predated the `#[serde(default)]`
        // fields. Only the always-present fields are stored; every field added
        // since must come back as its default rather than failing the read and
        // wiping the user's settings.
        let legacy = r#"{
            "launchAtLogin": false,
            "theme": "system",
            "keyboardEnabled": true,
            "windowManagementEnabled": true,
            "holdThresholdMs": 200,
            "showInMenuBar": true
        }"#;
        let settings: AppSettings = serde_json::from_str(legacy).unwrap();
        // The chosen required values match the defaults, so the whole struct
        // should reconstruct to the default — proving each missing field filled.
        assert_eq!(settings, AppSettings::default());
    }

    #[test]
    fn every_required_field_is_mandatory() {
        // Fields without `#[serde(default)]` are not optional: dropping one is a
        // corrupt row, which the caller handles, not a silently-defaulted value.
        // Table-driven so a field mistakenly left non-defaulted in the future is
        // caught here rather than silently zeroing a user's settings.
        let full = serde_json::to_value(AppSettings::default()).unwrap();
        let required = [
            "launchAtLogin",
            "theme",
            "keyboardEnabled",
            "windowManagementEnabled",
            "holdThresholdMs",
            "showInMenuBar",
        ];
        for key in required {
            let mut obj = full.clone();
            obj.as_object_mut()
                .unwrap()
                .remove(key)
                .expect("field present in the serialized form");
            assert!(
                serde_json::from_value::<AppSettings>(obj).is_err(),
                "removing required field `{key}` must fail to deserialize"
            );
        }
    }

    #[test]
    fn round_trips_through_json() {
        // A fully non-default value survives serialize → deserialize unchanged,
        // so persisting and reloading settings is lossless.
        let original = AppSettings {
            launch_at_login: true,
            theme: Theme::Dark,
            language: Language::Ja,
            keyboard_enabled: false,
            window_management_enabled: false,
            external_window_actions_enabled: false,
            hold_threshold_ms: 350,
            show_in_menu_bar: false,
            drag_to_snap_enabled: true,
        };
        let json = serde_json::to_string(&original).unwrap();
        let restored: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, original);
    }
}
