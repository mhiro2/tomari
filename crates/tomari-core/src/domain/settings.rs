//! Application-wide user settings.

use serde::{Deserialize, Serialize};

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
    /// UI language for the panel and tray menu.
    #[serde(default)]
    pub language: Language,
    /// Master switches for each feature area.
    pub keyboard_enabled: bool,
    pub window_management_enabled: bool,
    /// Allow external processes — launchers like Raycast or Alfred, via the
    /// `tomari://` URL scheme — to place the focused window (snap, move to
    /// display, undo). A `tomari://` URL can be opened by any local process or
    /// web page, so external window control is opt-in: this defaults *off*.
    /// Toggling the panel (`tomari://v1/toggle-panel`) is *not* gated here — it
    /// only shows/hides Tomari's own window and is the recovery route for a
    /// hidden menu bar. Input-synthesis actions (keystrokes, IME) are likewise
    /// not covered and will live behind their own switch when added.
    #[serde(default = "default_external_window_actions_enabled")]
    pub external_window_actions_enabled: bool,
    /// Tap the left ⌘ for 英数 and the right ⌘ for かな. A single switch for the
    /// pair, since they are the two halves of one JIS-style IME-toggle habit.
    #[serde(default = "default_command_ime_switch_enabled")]
    pub command_ime_switch_enabled: bool,
    /// Show the icon in the macOS menu bar.
    pub show_in_menu_bar: bool,
    /// Snap the focused window by dragging it (no modifier) to a screen edge or
    /// corner: a preview appears at the edge and the window snaps on release.
    #[serde(default)]
    pub drag_to_snap_enabled: bool,
    /// Move (⌃⌥ + drag) or resize (⌃⌥⌘ + drag) the window under the pointer by
    /// dragging anywhere inside it — no need to grab the title bar or click
    /// first. A single switch covers both gestures, like the snap toggle above.
    #[serde(default)]
    pub drag_to_move_enabled: bool,
}

fn default_external_window_actions_enabled() -> bool {
    false
}

fn default_command_ime_switch_enabled() -> bool {
    true
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            launch_at_login: false,
            language: Language::System,
            keyboard_enabled: true,
            window_management_enabled: true,
            external_window_actions_enabled: default_external_window_actions_enabled(),
            command_ime_switch_enabled: default_command_ime_switch_enabled(),
            show_in_menu_bar: true,
            drag_to_snap_enabled: false,
            drag_to_move_enabled: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_window_actions_default_off() {
        // External window control is opt-in: a `tomari://` URL can be opened by
        // any local process, so it must not move windows until the user asks.
        assert!(!AppSettings::default().external_window_actions_enabled);
    }

    #[test]
    fn command_ime_switch_default_on() {
        // The left/right ⌘ IME toggle is on out of the box, matching the
        // bindings a fresh install ships with.
        assert!(AppSettings::default().command_ime_switch_enabled);
    }

    #[test]
    fn legacy_json_without_optional_fields_fills_defaults() {
        // A settings row written by an older build. It still carries the now
        // dropped `theme` and `holdThresholdMs` fields, which must be ignored
        // rather than failing the read; every field added since must come back
        // as its default rather than wiping the user's settings.
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
        // should reconstruct to the default — proving each missing field filled
        // and each dropped field ignored.
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
            "keyboardEnabled",
            "windowManagementEnabled",
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
            language: Language::Ja,
            keyboard_enabled: false,
            window_management_enabled: false,
            external_window_actions_enabled: false,
            command_ime_switch_enabled: false,
            show_in_menu_bar: false,
            drag_to_snap_enabled: true,
            drag_to_move_enabled: true,
        };
        let json = serde_json::to_string(&original).unwrap();
        let restored: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, original);
    }
}
