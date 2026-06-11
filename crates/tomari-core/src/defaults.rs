//! Factory data used to seed a fresh install with sensible default bindings:
//! Caps Lock → Control (tap for Esc) and tapping the left/right ⌘ to flip
//! between 英数 and かな.

use crate::domain::action::{AppAction, ImeMode, LaunchTarget};
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
        // Shipped disabled: a global Cmd+Shift+F would shadow that shortcut in
        // every app (Finder's Recents, Chrome full screen, Slack search, …) on
        // first run. The user opts in — and can rebind — from the settings UI.
        Hotkey {
            id: "hk-peek-finder".into(),
            label: "Quick Peek Finder".into(),
            accelerator: "Cmd+Shift+F".into(),
            action: AppAction::LaunchApp(LaunchTarget {
                bundle_id: "com.apple.finder".into(),
                quick_peek: true,
            }),
            enabled: false,
        },
    ]
}

/// Default modifier rules, reproducing the bindings the Tomari keyboard page
/// highlights: Caps Lock → Control (built in for JIS users), and tapping the
/// left/right ⌘ to flip between 英数 and かな.
pub fn default_modifier_rules() -> Vec<ModifierRule> {
    vec![
        ModifierRule {
            id: "mr-capslock".into(),
            label: "Caps Lock → Control (tap for Esc)".into(),
            modifier: ModifierKey::CapsLock,
            side: KeySide::Either,
            remap_to: Some(ModifierKey::Control),
            hyper: false,
            tap: AppAction::SendKeystroke("Escape".into()),
            enabled: true,
        },
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
