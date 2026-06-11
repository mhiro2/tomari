//! The unified action vocabulary that hotkeys, modifier taps and the tray menu
//! all map onto. Keeping a single enum means every input path — a global
//! shortcut, a modifier tap, or a menu click — resolves to the same command.

use serde::{Deserialize, Serialize};

use super::window::{DisplayDirection, WindowPreset};

/// The two macOS input modes Tomari flips between when a ⌘ key is tapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImeMode {
    /// Roman / alphanumeric input (英数).
    Alphanumeric,
    /// Kana input (かな).
    Kana,
}

/// An application to launch or summon, optionally with "Quick Peek" behavior
/// (it hides itself again when the triggering key is released).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchTarget {
    /// macOS bundle identifier, e.g. `"com.apple.Safari"`.
    pub bundle_id: String,
    /// When true, summon the app while held and hide it on release.
    pub quick_peek: bool,
}

/// A high-level command the application can perform in response to user input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
pub enum AppAction {
    /// Show or hide the menu-bar panel window.
    TogglePanel,
    /// Move/resize the focused window to a preset position. Repeating the same
    /// request on an unmoved window cycles 1/2 → 1/3 → 2/3.
    SnapWindow(WindowPreset),
    /// Like [`SnapWindow`] but always applies exactly the requested preset,
    /// never cycling. Used by deterministic callers — the URL scheme — where a
    /// second identical invocation must be idempotent rather than advance a
    /// cycle.
    SnapWindowExact(WindowPreset),
    /// Move the focused window to a neighboring display, keeping its
    /// proportional position and size.
    MoveWindowToDisplay(DisplayDirection),
    /// Restore the focused window to its frame before the last window action.
    UndoWindow,
    /// Launch or summon an application.
    LaunchApp(LaunchTarget),
    /// Switch the active input method (英数 / かな).
    SwitchIme(ImeMode),
    /// Emit a keystroke described by an accelerator string, e.g. `"Escape"`.
    SendKeystroke(String),
    /// Turn sleep prevention on or off (toggle). Keeps the system awake while
    /// long-running work continues, including with the lid closed.
    ToggleKeepAwake,
    /// Explicitly do nothing (used to leave a tap unbound).
    NoOp,
}

impl AppAction {
    /// A short, human-readable label for menus and the UI.
    pub fn label(&self) -> String {
        match self {
            Self::TogglePanel => "Toggle Panel".into(),
            Self::SnapWindow(preset) | Self::SnapWindowExact(preset) => {
                format!("Snap: {}", preset.label())
            }
            Self::MoveWindowToDisplay(direction) => format!("Move to {}", direction.label()),
            Self::UndoWindow => "Undo Window Move".into(),
            Self::LaunchApp(t) => {
                let verb = if t.quick_peek { "Quick Peek" } else { "Launch" };
                format!("{verb}: {}", t.bundle_id)
            }
            Self::SwitchIme(ImeMode::Alphanumeric) => "IME: 英数".into(),
            Self::SwitchIme(ImeMode::Kana) => "IME: かな".into(),
            Self::SendKeystroke(k) => format!("Send: {k}"),
            Self::ToggleKeepAwake => "Keep Awake".into(),
            Self::NoOp => "Do Nothing".into(),
        }
    }

    /// Whether this action is a no-op.
    pub fn is_noop(&self) -> bool {
        matches!(self, Self::NoOp)
    }
}
