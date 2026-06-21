//! Domain types for the keyboard-customization feature: per-modifier tap
//! commands, modifier remapping (e.g. Caps Lock → Control) and left/right side
//! awareness for the IME-switching trick (tap left/right ⌘ to flip 英数 / かな).

use serde::{Deserialize, Serialize};

use super::action::AppAction;

/// A physical modifier key that can be remapped or given a tap command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModifierKey {
    CapsLock,
    Control,
    Option,
    Command,
    Shift,
    Function,
}

impl ModifierKey {
    pub fn label(&self) -> &'static str {
        match self {
            Self::CapsLock => "Caps Lock",
            Self::Control => "Control",
            Self::Option => "Option",
            Self::Command => "Command",
            Self::Shift => "Shift",
            Self::Function => "Fn",
        }
    }

    /// The glyph macOS uses for this modifier.
    pub fn glyph(&self) -> &'static str {
        match self {
            Self::CapsLock => "⇪",
            Self::Control => "⌃",
            Self::Option => "⌥",
            Self::Command => "⌘",
            Self::Shift => "⇧",
            Self::Function => "fn",
        }
    }
}

/// Which physical side of a paired modifier a rule applies to. `Either` matches
/// both; `Left`/`Right` enable the "tap left ⌘ for 英数, right ⌘ for かな" trick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeySide {
    Left,
    Right,
    Either,
}

impl KeySide {
    /// Does a rule for `self` apply to an event coming from `event_side`?
    pub fn matches(self, event_side: KeySide) -> bool {
        matches!(
            (self, event_side),
            (KeySide::Either, _)
                | (KeySide::Left, KeySide::Left)
                | (KeySide::Right, KeySide::Right)
        )
    }
}

/// A global hotkey: an accelerator string bound to an [`AppAction`].
///
/// The `accelerator` uses the Tauri/Electron syntax, e.g. `"Cmd+Shift+R"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hotkey {
    pub id: String,
    pub label: String,
    pub accelerator: String,
    pub action: AppAction,
    pub enabled: bool,
}

/// Repurposes a modifier key:
/// * `remap_to` changes the role it plays when held/chorded (e.g. Caps → Ctrl).
/// * `tap` fires when the key is pressed and released *alone* and quickly.
///
/// Holding the key (or using it in a chord) leaves system shortcuts working,
/// exactly as the Tomari keyboard page describes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModifierRule {
    pub id: String,
    pub label: String,
    pub modifier: ModifierKey,
    pub side: KeySide,
    /// Role the key plays as a modifier; `None` keeps its native behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remap_to: Option<ModifierKey>,
    /// When held (or chorded), act as the *hyper* combo — ⌃⌥⇧⌘ at once.
    /// Takes precedence over `remap_to` for the held role.
    #[serde(default)]
    pub hyper: bool,
    /// Command fired on a solo tap. Use [`AppAction::NoOp`] to leave unbound.
    pub tap: AppAction,
    pub enabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifier_rule_omits_defaults_when_deserializing_legacy_json() {
        // Rows persisted before `hyper` existed must still load.
        let json = r#"{
            "id": "m1", "label": "Caps", "modifier": "capsLock", "side": "either",
            "remapTo": "control", "tap": {"type":"noOp"}, "enabled": true
        }"#;
        let rule: ModifierRule = serde_json::from_str(json).unwrap();
        assert!(!rule.hyper);
    }
}
