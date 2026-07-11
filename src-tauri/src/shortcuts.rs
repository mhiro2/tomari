//! Registration of global hotkeys with the system. The plugin's handler (set
//! up in `main`) looks the triggered shortcut up in [`AppState::shortcuts`] and
//! dispatches the associated action.

use std::str::FromStr;

use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

use crate::locks::MutexExt;
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

    // Read the hotkey list *before* unregistering anything: if the DB cannot
    // be read, bailing out here leaves the current, working registrations (and
    // `state.shortcuts`) completely untouched, rather than having already
    // cleared them and losing every live hotkey to a transient read failure.
    let hotkeys = state.db.list_hotkeys().map_err(|e| e.to_string())?;

    // If the previous set cannot be cleared, re-registering would fail with
    // "already registered" for every hotkey while the dispatch map is gone —
    // keep the current, working registrations instead and report it.
    if let Err(e) = gs.unregister_all() {
        return Err(format!(
            "could not clear previously registered shortcuts: {e}"
        ));
    }

    let mut map = state.shortcuts.lock_safe();
    map.clear();

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

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use tauri_plugin_global_shortcut::Shortcut;
    use tomari_keyboard::accelerator;

    /// `accelerator::normalize` accepts these symbol-key names (see
    /// `tomari_keyboard::accelerator`), and the whole point of matching
    /// global-hotkey's own spelling is that its accelerator parser
    /// (`Shortcut::from_str`, the same one `register_all` hands the
    /// canonical string to) accepts them too — otherwise a hotkey would pass
    /// `validate_accelerator` at save time yet fail to register.
    #[test]
    fn symbol_key_accelerators_are_accepted_by_global_hotkey() {
        for input in [
            "Cmd+Semicolon",
            "Cmd+Quote",
            "Cmd+BracketLeft",
            "Cmd+BracketRight",
            "Cmd+Backslash",
            "Cmd+Backquote",
        ] {
            let normalized = accelerator::normalize(input).unwrap_or_else(|e| {
                panic!("tomari accelerator parser rejected `{input}`: {e}");
            });
            Shortcut::from_str(&normalized).unwrap_or_else(|e| {
                panic!("global-hotkey rejected normalized accelerator `{normalized}`: {e}");
            });
        }
    }
}
