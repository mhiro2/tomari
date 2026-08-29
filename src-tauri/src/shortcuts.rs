//! Registration of global hotkeys with the system. The plugin's handler (set
//! up in `main`) looks the triggered shortcut up in [`AppState::shortcuts`] and
//! dispatches the associated action.

use std::str::FromStr;

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};
use tomari_core::{AppAction, Hotkey};
use tomari_keyboard::validation::{self, InvalidRecord};

use crate::configuration_warnings::publish_hotkey_issues;
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

struct PreparedHotkeys {
    live: Vec<Hotkey>,
    invalid: Vec<InvalidRecord>,
}

/// Validate a complete persisted collection before selecting enabled rows.
/// Disabled rows remain part of collision detection and the warning report,
/// but no disabled or invalid row can reach the OS registration boundary.
fn prepare_hotkeys_for_registration(hotkeys: Vec<Hotkey>) -> PreparedHotkeys {
    let report = validation::validate_hotkeys(hotkeys);
    PreparedHotkeys {
        live: report
            .valid
            .into_iter()
            .filter(|hotkey| hotkey.enabled)
            .collect(),
        invalid: report.invalid,
    }
}

/// Re-register every enabled hotkey from the database, replacing the previous
/// set. Hotkeys that fail to register (invalid or conflicting) are returned
/// individually rather than failing the whole pass: one stale conflict — e.g.
/// an accelerator another app grabbed since — must not block saving or
/// toggling every other hotkey. `Err` is reserved for not being able to read
/// the hotkey list at all, or for the main thread not answering.
///
/// Persisted rows are validated as a complete collection before anything is
/// unregistered. Semantically invalid rows remain stored for repair but are
/// quarantined from the live runtime and surfaced through configuration
/// warnings. The database read happens on the caller's thread; everything that
/// touches the global-shortcut plugin, and the dispatch map with it, runs on
/// the main thread (see [`on_main_thread`]). Callable from any thread.
pub fn register_all(app: &AppHandle, state: &AppState) -> Result<Vec<RegistrationFailure>, String> {
    let result = if state.keyboard_enabled() {
        // Read and validate *before* unregistering anything: a hard read error
        // leaves the current, working set untouched. Warning event delivery is
        // advisory and cannot prevent the accepted rows from becoming live.
        match state.db.list_hotkeys() {
            Ok(hotkeys) => {
                let prepared = prepare_hotkeys_for_registration(hotkeys);
                publish_hotkey_issues(app, &state.configuration_warnings, prepared.invalid);
                on_main_thread(app, move |app, state| {
                    apply_registrations(app, state, prepared.live)
                })
            }
            Err(error) => Err(error.to_string()),
        }
    } else {
        // Release first. A damaged or temporarily unreadable database must
        // never keep a shortcut live after the master switch is turned off.
        let result = on_main_thread(app, |app, state| {
            apply_registrations(app, state, Vec::new())
        });

        // Refresh warnings only after release, and only on a successful read.
        // A failure leaves the last coherent, pullable snapshot intact and is
        // deliberately not promoted to the runtime-apply result above.
        match state.db.list_hotkeys() {
            Ok(hotkeys) => {
                let prepared = prepare_hotkeys_for_registration(hotkeys);
                publish_hotkey_issues(app, &state.configuration_warnings, prepared.invalid);
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    "could not refresh configuration warnings after disabling shortcuts"
                );
            }
        }

        result
    };
    record_registration_result(state, &result);
    result
}

/// Release every registered shortcut and clear the dispatch map, e.g. while
/// the settings panel records a new chord. Callable from any thread; the
/// plugin work runs on the main thread like [`register_all`]'s.
pub fn suspend_all(app: &AppHandle) -> Result<(), String> {
    on_main_thread(app, |app, state| {
        app.global_shortcut()
            .unregister_all()
            .map_err(|e| e.to_string())?;
        state.shortcuts.lock_safe().clear();
        Ok(())
    })
}

/// Run `task` on the main thread and wait for its result — inline when already
/// there.
///
/// The global-shortcut plugin performs each register/unregister on the main
/// thread and, called from any other thread, waits for it *while holding its
/// own shortcut table lock*. The hotkey event handler, which runs on the main
/// thread, takes that same lock: a hotkey event queued ahead of the plugin's
/// task would then wait on a lock held by a thread waiting on the main thread —
/// a deadlock. The same shape applies to `AppState::shortcuts`, which the
/// dispatch path locks on the main thread. So the plugin calls and the dispatch
/// map update are never made from off the main thread piecemeal: the whole
/// sequence is handed to the main thread as one closure, and the caller waits
/// for it holding nothing the main thread needs (`config_mutation` is never
/// awaited on the main thread — see `AppState::lock_config_mutation`).
fn on_main_thread<T, F>(app: &AppHandle, task: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&AppHandle, &AppState) -> Result<T, String> + Send + 'static,
{
    if objc2_foundation::MainThreadMarker::new().is_some() {
        let state = app.state::<AppState>();
        return task(app, state.inner());
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let state = handle.state::<AppState>();
        let _ = tx.send(task(&handle, state.inner()));
    })
    .map_err(|e| format!("could not reach the main thread to update shortcuts: {e}"))?;
    rx.recv()
        .map_err(|_| "the main thread dropped the shortcut update without answering".to_string())?
}

fn record_registration_result(state: &AppState, result: &Result<Vec<RegistrationFailure>, String>) {
    state.set_shortcut_registration_incomplete(registration_incomplete(
        state.keyboard_enabled(),
        result,
    ));
}

/// The main-thread half of [`register_all`]: replace the live registrations
/// and the dispatch map with `hotkeys`.
fn apply_registrations(
    app: &AppHandle,
    state: &AppState,
    hotkeys: Vec<Hotkey>,
) -> Result<Vec<RegistrationFailure>, String> {
    let gs = app.global_shortcut();

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

    /// Every canonical global hotkey accepted by Tomari must also be accepted
    /// by the parser used by `register_all`. Enumerating the shared key
    /// vocabulary here turns additions — including semantic translations such
    /// as `Plus` to `Shift+Equal` — into an explicit cross-crate contract.
    #[test]
    fn validated_hotkey_keys_are_accepted_by_global_shortcut() {
        for key in accelerator::all_canonical_keys() {
            let input = hotkey("parity", &format!("Cmd+{key}"), true);
            let validated = tomari_keyboard::validation::validate_hotkey(input)
                .unwrap_or_else(|issue| panic!("Tomari rejected `Cmd+{key}`: {issue}"));
            Shortcut::from_str(&validated.accelerator).unwrap_or_else(|error| {
                panic!(
                    "global-shortcut rejected `{}` canonicalized from `Cmd+{key}`: {error}",
                    validated.accelerator
                );
            });
        }
    }

    #[test]
    fn command_plus_is_canonicalized_to_a_registrable_shortcut() {
        let validated =
            tomari_keyboard::validation::validate_hotkey(hotkey("zoom", "Cmd+Plus", true)).unwrap();

        assert_eq!(validated.accelerator, "Shift+Cmd+Equal");
        Shortcut::from_str(&validated.accelerator).unwrap();
    }

    #[test]
    fn keyboard_master_switch_blocks_dispatch() {
        let disabled = state(false);
        let shortcut = Shortcut::from_str("Cmd+Left").unwrap();
        disabled
            .shortcuts
            .lock_safe()
            .insert(shortcut, AppAction::NoOp);

        assert!(action_for_shortcut(&disabled, &shortcut).is_none());

        let enabled = state(true);
        enabled
            .shortcuts
            .lock_safe()
            .insert(shortcut, AppAction::NoOp);

        assert_eq!(
            action_for_shortcut(&enabled, &shortcut),
            Some(AppAction::NoOp)
        );
    }

    fn hotkey(id: &str, accelerator: &str, enabled: bool) -> Hotkey {
        Hotkey {
            id: id.into(),
            label: format!("Hotkey {id}"),
            accelerator: accelerator.into(),
            action: AppAction::NoOp,
            enabled,
        }
    }

    #[test]
    fn preparation_quarantines_unsafe_rows_but_keeps_valid_enabled_rows() {
        let prepared = prepare_hotkeys_for_registration(vec![
            hotkey("unsafe-enabled", "A", true),
            hotkey("unsafe-disabled", "B", false),
            hotkey("valid-enabled", " command + shift + 1 ", true),
            hotkey("valid-disabled", "Cmd+Shift+2", false),
        ]);

        assert_eq!(prepared.live.len(), 1);
        assert_eq!(prepared.live[0].id, "valid-enabled");
        assert_eq!(prepared.live[0].accelerator, "Shift+Cmd+1");
        assert_eq!(prepared.invalid.len(), 2);
        assert!(prepared.invalid.iter().any(|issue| {
            issue.id == "unsafe-enabled" && issue.reason.code() == "unsafeGlobalShortcut"
        }));
        assert!(prepared.invalid.iter().any(|issue| {
            issue.id == "unsafe-disabled" && issue.reason.code() == "unsafeGlobalShortcut"
        }));
    }

    #[test]
    fn preparation_quarantines_every_duplicate_even_when_one_is_disabled() {
        let prepared = prepare_hotkeys_for_registration(vec![
            hotkey("duplicate-enabled", "Cmd+1", true),
            hotkey("duplicate-disabled", " command + 1 ", false),
            hotkey("valid", "Cmd+2", true),
        ]);

        assert_eq!(prepared.live.len(), 1);
        assert_eq!(prepared.live[0].id, "valid");
        assert_eq!(prepared.invalid.len(), 2);
        assert!(
            prepared
                .invalid
                .iter()
                .all(|issue| issue.reason.code() == "duplicateAccelerator")
        );
        let mut ids: Vec<_> = prepared.invalid.into_iter().map(|issue| issue.id).collect();
        ids.sort();
        assert_eq!(ids, ["duplicate-disabled", "duplicate-enabled"]);
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
