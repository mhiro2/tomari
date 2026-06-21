//! Factory data used to seed a fresh install with sensible default bindings:
//! Caps Lock → Control, and tapping the left/right ⌘ to flip between 英数 and
//! かな (the latter pair gated by `command_ime_switch_enabled`).

use crate::domain::action::{AppAction, ImeMode};
use crate::domain::keyboard::{Hotkey, KeySide, ModifierKey, ModifierRule};
use crate::domain::window::WindowPreset;

/// Default global hotkeys shipped on first launch.
pub fn default_hotkeys() -> Vec<Hotkey> {
    vec![
        Hotkey {
            id: "hk-toggle-panel".into(),
            label: "Show/hide Tomari".into(),
            accelerator: "Cmd+Shift+K".into(),
            action: AppAction::TogglePanel,
            enabled: true,
        },
        Hotkey {
            id: "hk-snap-left".into(),
            label: "Snap window left".into(),
            accelerator: "Ctrl+Alt+Left".into(),
            action: AppAction::SnapWindow(WindowPreset::LeftHalf),
            enabled: true,
        },
        Hotkey {
            id: "hk-snap-right".into(),
            label: "Snap window right".into(),
            accelerator: "Ctrl+Alt+Right".into(),
            action: AppAction::SnapWindow(WindowPreset::RightHalf),
            enabled: true,
        },
        Hotkey {
            id: "hk-maximize".into(),
            label: "Maximize window".into(),
            accelerator: "Ctrl+Alt+Up".into(),
            action: AppAction::SnapWindow(WindowPreset::Maximize),
            enabled: true,
        },
    ]
}

/// Default modifier rules seeded on first run. Caps Lock → Control is the one
/// stored rule (built in for JIS users); the left/right ⌘ IME toggle is not a
/// stored rule — it is assembled on demand from [`command_ime_rules`] when the
/// `command_ime_switch_enabled` setting is on.
pub fn default_modifier_rules() -> Vec<ModifierRule> {
    vec![ModifierRule {
        id: "mr-capslock".into(),
        label: "Caps Lock → Control".into(),
        modifier: ModifierKey::CapsLock,
        side: KeySide::Either,
        remap_to: Some(ModifierKey::Control),
        hyper: false,
        // Caps Lock is a pure remap: it acts as Control whether tapped or held,
        // with no tap action of its own.
        tap: AppAction::NoOp,
        enabled: true,
    }]
}

/// The left/right ⌘ IME-toggle rules, assembled when `command_ime_switch_enabled`
/// is on rather than persisted as editable rows: the two sides are the halves of
/// one habit and are toggled together by a single setting.
pub fn command_ime_rules() -> Vec<ModifierRule> {
    vec![
        ModifierRule {
            id: "mr-left-cmd-eisu".into(),
            label: "Tap left ⌘ → 英数".into(),
            modifier: ModifierKey::Command,
            side: KeySide::Left,
            remap_to: None,
            hyper: false,
            tap: AppAction::SwitchIme(ImeMode::Alphanumeric),
            enabled: true,
        },
        ModifierRule {
            id: "mr-right-cmd-kana".into(),
            label: "Tap right ⌘ → かな".into(),
            modifier: ModifierKey::Command,
            side: KeySide::Right,
            remap_to: None,
            hyper: false,
            tap: AppAction::SwitchIme(ImeMode::Kana),
            enabled: true,
        },
    ]
}
