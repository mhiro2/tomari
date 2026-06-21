//! Registration of global hotkeys with the system. The plugin's handler (set
//! up in `main`) looks the triggered shortcut up in [`AppState::shortcuts`] and
//! dispatches the associated action.

use std::str::FromStr;

use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

use crate::state::AppState;

/// A hotkey that could not be registered (invalid accelerator, or a conflict
/// with another app), identified so callers can tell whether a particular
/// hotkey was affected.
pub struct RegistrationFailure {
    pub id: String,
    pub accelerator: String,
    pub error: String,
}

/// Re-register every enabled hotkey from the database, replacing the previous
/// set. Hotkeys that fail to register (invalid or conflicting) are returned
/// individually rather than failing the whole pass: one stale conflict — e.g.
/// an accelerator another app grabbed since — must not block saving or
/// toggling every other hotkey. `Err` is reserved for not being able to read
/// the hotkey list at all.
pub fn register_all(app: &AppHandle, state: &AppState) -> Result<Vec<RegistrationFailure>, String> {
    let gs = app.global_shortcut();
    // If the previous set cannot be cleared, re-registering would fail with
    // "already registered" for every hotkey while the dispatch map is gone —
    // keep the current, working registrations instead and report it.
    if let Err(e) = gs.unregister_all() {
        return Err(format!(
            "could not clear previously registered shortcuts: {e}"
        ));
    }

    let mut map = state.shortcuts.lock().unwrap();
    map.clear();

    let hotkeys = state.db.list_hotkeys().map_err(|e| e.to_string())?;
    let mut failures = Vec::new();
    for hk in hotkeys.into_iter().filter(|h| h.enabled) {
        match Shortcut::from_str(&hk.accelerator) {
            Ok(shortcut) => match gs.register(shortcut) {
                Ok(()) => {
                    map.insert(shortcut, hk.action);
                }
                Err(e) => {
                    tracing::warn!(accelerator = %hk.accelerator, error = %e, "failed to register shortcut");
                    failures.push(RegistrationFailure {
                        id: hk.id,
                        accelerator: hk.accelerator,
                        error: e.to_string(),
                    });
                }
            },
            Err(e) => {
                tracing::warn!(accelerator = %hk.accelerator, error = %e, "invalid accelerator");
                failures.push(RegistrationFailure {
                    id: hk.id,
                    accelerator: hk.accelerator,
                    error: e.to_string(),
                });
            }
        }
    }

    Ok(failures)
}
