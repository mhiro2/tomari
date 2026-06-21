//! Dispatch an [`AppAction`] coming from a hotkey, modifier tap, tray menu or
//! the UI to its concrete effect.

use tauri::{AppHandle, Manager};
use tomari_core::{AppAction, ImeMode};

use crate::error::CmdError;
use crate::state::AppState;

/// Perform `action`. Errors are returned as a [`CmdError`] so they can flow
/// back to the frontend (localized by code) or be logged from the shortcut
/// handler.
pub fn dispatch(action: &AppAction, app: &AppHandle, state: &AppState) -> Result<(), CmdError> {
    match action {
        AppAction::TogglePanel => toggle_panel(app),
        // Window ops funnel through `window_ops`, which honors the
        // window-management master switch and records undo history.
        AppAction::SnapWindow(preset) => {
            crate::window_ops::snap(state, *preset, crate::window_ops::SnapBehavior::Cycle)
                .map(|_| ())
        }
        AppAction::SnapWindowExact(preset) => {
            crate::window_ops::snap(state, *preset, crate::window_ops::SnapBehavior::Exact)
                .map(|_| ())
        }
        AppAction::MoveWindowToDisplay(direction) => {
            crate::window_ops::move_to_display(state, *direction)
        }
        AppAction::UndoWindow => crate::window_ops::undo(state),
        AppAction::SwitchIme(mode) => switch_ime(state, *mode),
        AppAction::SendKeystroke(accel) => send_keystroke(state, accel),
        AppAction::ToggleKeepAwake => {
            crate::keepawake::toggle(app);
            Ok(())
        }
        AppAction::NoOp => Ok(()),
    }
}

/// Toggle the Tomari window: hide it only when it is the active (visible and
/// focused) window, otherwise show and bring it forward. So the global shortcut
/// raises the window when it is buried behind another app rather than hiding an
/// out-of-sight window the user is trying to summon.
pub fn toggle_panel(app: &AppHandle) -> Result<(), CmdError> {
    let Some(window) = app.get_webview_window("main") else {
        return Ok(());
    };
    let visible = window.is_visible().unwrap_or(false);
    let focused = window.is_focused().unwrap_or(false);
    if visible && focused {
        window.hide().map_err(|e| CmdError::other(e.to_string()))
    } else {
        window.show().map_err(|e| CmdError::other(e.to_string()))?;
        let _ = window.set_focus();
        Ok(())
    }
}

/// Show and focus the window without toggling, so a menu entry that opens it
/// brings it forward even when it is already visible.
pub fn show_panel(app: &AppHandle) -> Result<(), CmdError> {
    let Some(window) = app.get_webview_window("main") else {
        return Ok(());
    };
    window.show().map_err(|e| CmdError::other(e.to_string()))?;
    let _ = window.set_focus();
    Ok(())
}

fn switch_ime(state: &AppState, mode: ImeMode) -> Result<(), CmdError> {
    #[cfg(target_os = "macos")]
    {
        ensure_keystroke_permission(state)?;
        crate::keysend::switch_ime(mode).map_err(CmdError::other)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (state, mode);
        Ok(())
    }
}

fn send_keystroke(state: &AppState, accel: &str) -> Result<(), CmdError> {
    #[cfg(target_os = "macos")]
    {
        ensure_keystroke_permission(state)?;
        crate::keysend::send_accelerator(accel).map_err(CmdError::other)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (state, accel);
        Ok(())
    }
}

/// Posting CGEvents silently no-ops without the Accessibility permission, so
/// check first and surface an actionable error instead of a keystroke that
/// quietly does nothing (the IME does not switch, no error is reported).
#[cfg(target_os = "macos")]
fn ensure_keystroke_permission(state: &AppState) -> Result<(), CmdError> {
    if state.windows.permission_granted() {
        return Ok(());
    }
    tracing::warn!("keystroke synthesis skipped: Accessibility permission not granted");
    Err(CmdError::permission_required(
        "Accessibility permission is required to send keystrokes",
    ))
}
