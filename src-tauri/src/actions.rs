//! Dispatch an [`AppAction`] coming from a hotkey, modifier tap, tray menu or
//! the UI to its concrete effect.

use tauri::{AppHandle, Manager};
use tomari_core::{AppAction, ImeMode, LaunchTarget};

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
        AppAction::LaunchApp(target) => {
            // Quick Peek from a path with no key-release moment (tray menu, the
            // UI's run button) toggles instead: first trigger summons, the next
            // one dismisses. The hold-capable paths (hotkey handler, event tap)
            // call peek::begin/end directly.
            #[cfg(target_os = "macos")]
            if target.quick_peek {
                crate::peek::toggle(app, &target.bundle_id);
                return Ok(());
            }
            launch_app(target)
        }
        AppAction::SwitchIme(mode) => switch_ime(state, *mode),
        AppAction::SendKeystroke(accel) => send_keystroke(state, accel),
        AppAction::ToggleKeepAwake => {
            crate::keepawake::toggle(app);
            Ok(())
        }
        AppAction::NoOp => Ok(()),
    }
}

/// Show the panel if hidden, hide it if visible.
pub fn toggle_panel(app: &AppHandle) -> Result<(), CmdError> {
    let Some(window) = app.get_webview_window("main") else {
        return Ok(());
    };
    if window.is_visible().unwrap_or(false) {
        window.hide().map_err(|e| CmdError::other(e.to_string()))
    } else {
        window.show().map_err(|e| CmdError::other(e.to_string()))?;
        let _ = window.set_focus();
        Ok(())
    }
}

/// Show and focus the panel without toggling, so a menu entry that opens it
/// brings it forward even when it is already visible.
pub fn show_panel(app: &AppHandle) -> Result<(), CmdError> {
    let Some(window) = app.get_webview_window("main") else {
        return Ok(());
    };
    window.show().map_err(|e| CmdError::other(e.to_string()))?;
    let _ = window.set_focus();
    Ok(())
}

fn launch_app(target: &LaunchTarget) -> Result<(), CmdError> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("-b")
            .arg(&target.bundle_id)
            .spawn()
            .map(|_| ())
            .map_err(|e| CmdError::other(e.to_string()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = target;
        Err(CmdError::other("launching apps is only supported on macOS"))
    }
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
