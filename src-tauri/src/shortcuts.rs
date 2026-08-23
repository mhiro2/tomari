//! Registration of global hotkeys with the system. The plugin's handler (set
//! up in `main`) looks the triggered shortcut up in [`AppState::shortcuts`] and
//! dispatches the associated action.

use std::str::FromStr;

use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};
use tomari_core::{AppAction, Hotkey};

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

/// Whether a registration pass left the live shortcut set out of step with
/// the keyboard master switch. Conflicts matter only while enabling shortcuts;
/// disabling them legitimately produces an empty registration set.
fn registration_incomplete(
    keyboard_enabled: bool,
    result: &Result<Vec<RegistrationFailure>, String>,
) -> bool {
    match result {
        Err(_) => true,
        Ok(failures) => keyboard_enabled && !failures.is_empty(),
    }
}

/// Resolve a registered shortcut only while keyboard customization is live.
/// The runtime check closes the small race where a shortcut event was already
/// queued while settings were turning the master switch off.
pub(crate) fn action_for_shortcut(state: &AppState, shortcut: &Shortcut) -> Option<AppAction> {
    if !state.keyboard_enabled() {
        return None;
    }
    state.shortcuts.lock_safe().get(shortcut).cloned()
}

fn hotkeys_for_registration(state: &AppState) -> Result<Vec<Hotkey>, String> {
    if !state.keyboard_enabled() {
        return Ok(Vec::new());
    }
    state
        .db
        .list_hotkeys()
        .map(|hotkeys| {
            hotkeys
                .into_iter()
                .filter(|hotkey| hotkey.enabled)
                .collect()
        })
        .map_err(|error| error.to_string())
}

/// Re-register every enabled hotkey from the database, replacing the previous
/// set. Hotkeys that fail to register (invalid or conflicting) are returned
/// individually rather than failing the whole pass: one stale conflict — e.g.
/// an accelerator another app grabbed since — must not block saving or
/// toggling every other hotkey. `Err` is reserved for not being able to read
/// the hotkey list at all.
pub fn register_all(app: &AppHandle, state: &AppState) -> Result<Vec<RegistrationFailure>, String> {
    let result = register_all_inner(app, state);
    record_registration_result(state, &result);
    result
}

fn record_registration_result(state: &AppState, result: &Result<Vec<RegistrationFailure>, String>) {
    state.set_shortcut_registration_incomplete(registration_incomplete(
        state.keyboard_enabled(),
        result,
    ));
}

fn register_all_inner(
    app: &AppHandle,
    state: &AppState,
) -> Result<Vec<RegistrationFailure>, String> {
    let gs = app.global_shortcut();

    // When enabled, read the hotkey list *before* unregistering anything: if
    // the DB cannot be read, bailing out leaves the current, working set
    // untouched. When the master switch is off, `hotkeys_for_registration`
    // deliberately skips the DB read; releasing live shortcuts must not be
    // blocked by an unrelated persistence failure.
    let hotkeys = hotkeys_for_registration(state)?;

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
    for hk in hotkeys {
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
    use tomari_core::{AppAction, AppSettings, Database, Hotkey, Rect};
    use tomari_keyboard::ModifierEngine;
    use tomari_keyboard::accelerator;
    use tomari_window::MockWindowManager;

    use super::*;

    fn state(keyboard_enabled: bool) -> AppState {
        let settings = AppSettings {
            keyboard_enabled,
            ..AppSettings::default()
        };
        AppState::new(
            Database::open_in_memory().unwrap(),
            ModifierEngine::new(Vec::new()),
            Box::new(MockWindowManager::new(Rect::new(0.0, 0.0, 100.0, 100.0))),
            settings,
            false,
        )
    }

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

    #[test]
    fn keyboard_master_switch_filters_registration_and_dispatch() {
        let disabled = state(false);
        disabled
            .db
            .upsert_hotkey(&Hotkey {
                id: "window-left".into(),
                label: "Window left".into(),
                accelerator: "Cmd+Left".into(),
                action: AppAction::NoOp,
                enabled: true,
            })
            .unwrap();
        let shortcut = Shortcut::from_str("Cmd+Left").unwrap();
        disabled
            .shortcuts
            .lock_safe()
            .insert(shortcut, AppAction::NoOp);

        assert!(hotkeys_for_registration(&disabled).unwrap().is_empty());
        assert!(action_for_shortcut(&disabled, &shortcut).is_none());

        let enabled = state(true);
        enabled
            .db
            .upsert_hotkey(&Hotkey {
                id: "window-left".into(),
                label: "Window left".into(),
                accelerator: "Cmd+Left".into(),
                action: AppAction::NoOp,
                enabled: true,
            })
            .unwrap();
        enabled
            .shortcuts
            .lock_safe()
            .insert(shortcut, AppAction::NoOp);

        assert_eq!(hotkeys_for_registration(&enabled).unwrap().len(), 1);
        assert_eq!(
            action_for_shortcut(&enabled, &shortcut),
            Some(AppAction::NoOp)
        );
    }

    #[test]
    fn registration_warning_tracks_hard_failures_and_enable_conflicts() {
        let conflict = RegistrationFailure {
            id: "window-left".into(),
            accelerator: "Cmd+Left".into(),
            error: "already registered".into(),
        };

        assert!(registration_incomplete(
            true,
            &Err("unregister failed".into())
        ));
        assert!(registration_incomplete(
            false,
            &Err("unregister failed".into())
        ));
        assert!(registration_incomplete(true, &Ok(vec![conflict])));
        assert!(!registration_incomplete(false, &Ok(Vec::new())));
        assert!(!registration_incomplete(true, &Ok(Vec::new())));
    }

    #[test]
    fn registration_warning_persists_until_a_successful_reconciliation() {
        let state = state(true);
        let conflict = RegistrationFailure {
            id: "window-left".into(),
            accelerator: "Cmd+Left".into(),
            error: "already registered".into(),
        };

        record_registration_result(&state, &Ok(vec![conflict]));
        assert!(state.shortcut_registration_incomplete());

        // An unrelated settings save performs no registration pass, so the
        // mismatch remains visible instead of being cleared optimistically.
        assert!(state.shortcut_registration_incomplete());

        record_registration_result(&state, &Ok(Vec::new()));
        assert!(!state.shortcut_registration_incomplete());
    }
}
