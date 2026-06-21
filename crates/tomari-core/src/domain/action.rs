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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::window::{DisplayDirection, WindowPreset};

    /// Pins the wire contract with the hand-written TypeScript mirror in
    /// `src/lib/types.ts`: each variant's adjacently-tagged `type` discriminant
    /// must match what the frontend expects. The `match` below is exhaustive, so
    /// adding a variant fails to compile here — a prompt to mirror it in
    /// `types.ts` (and extend this list) rather than let the two drift.
    #[test]
    fn variant_tags_match_typescript_mirror() {
        fn expected_tag(action: &AppAction) -> &'static str {
            match action {
                AppAction::TogglePanel => "togglePanel",
                AppAction::SnapWindow(_) => "snapWindow",
                AppAction::SnapWindowExact(_) => "snapWindowExact",
                AppAction::MoveWindowToDisplay(_) => "moveWindowToDisplay",
                AppAction::UndoWindow => "undoWindow",
                AppAction::SwitchIme(_) => "switchIme",
                AppAction::SendKeystroke(_) => "sendKeystroke",
                AppAction::ToggleKeepAwake => "toggleKeepAwake",
                AppAction::NoOp => "noOp",
            }
        }

        let samples = [
            AppAction::TogglePanel,
            AppAction::SnapWindow(WindowPreset::LeftHalf),
            AppAction::SnapWindowExact(WindowPreset::LeftHalf),
            AppAction::MoveWindowToDisplay(DisplayDirection::Next),
            AppAction::UndoWindow,
            AppAction::SwitchIme(ImeMode::Kana),
            AppAction::SendKeystroke("Escape".into()),
            AppAction::ToggleKeepAwake,
            AppAction::NoOp,
        ];

        for action in &samples {
            let json = serde_json::to_value(action).unwrap();
            assert_eq!(
                json.get("type").and_then(|t| t.as_str()),
                Some(expected_tag(action)),
                "unexpected serde tag for {action:?}"
            );
        }
    }
}
