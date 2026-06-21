//! Tauri commands — the bridge invoked from the React frontend. Argument names
//! here must match the keys passed from `src/lib/api.ts`.

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tomari_core::{AppAction, AppSettings, DisplayDirection, Hotkey, ModifierRule, WindowPreset};
use tomari_keyboard::accelerator;

use crate::actions;
use crate::error::CmdError;
use crate::locks::MutexExt;
use crate::shortcuts;
use crate::state::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceleratorCheck {
    valid: bool,
    normalized: Option<String>,
    error: Option<String>,
}

/// Outcome of [`save_settings`]: the settings always persist (a write failure
/// rejects the command instead), but a side effect may still fail to apply.
/// `apply_warnings` names each one that did, so the UI can warn that the stored
/// preference and the live system state disagree until retried. Empty on a
/// fully applied save.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSettingsOutcome {
    apply_warnings: Vec<&'static str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    version: String,
    notes: Option<String>,
}

/// An update found by [`check_for_update`], held until [`install_update`]
/// consumes it so the install does not have to hit the endpoint again.
#[derive(Default)]
pub struct PendingUpdate(pub std::sync::Mutex<Option<tauri_plugin_updater::Update>>);

/// Commands reject with a [`CmdError`] (a `{ code, message }` pair) so the
/// frontend can localize the frequent failures and fall back to the message
/// for the rest.
type CmdResult<T> = Result<T, CmdError>;

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> CmdResult<AppSettings> {
    // Return the live, sanitized in-memory settings rather than re-reading the
    // database. `AppState.settings` is the single source of truth the engines,
    // tray and taps run from; the DB is only its persistence layer. Reading the
    // row directly would let the UI drift from runtime state if a row were
    // hand-edited, out of range, or written ahead of the live update.
    Ok(state.settings.lock_safe().clone())
}

#[tauri::command]
pub fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: AppSettings,
) -> CmdResult<SaveSettingsOutcome> {
    // Serialize against other concurrent config mutations, which each write to
    // the database and then rebuild the engines from it.
    let _config = state.lock_config_mutation();
    let previous = state.settings.lock_safe().clone();

    // Persisting is the hard requirement: a failure here rejects so the UI knows
    // nothing was saved. The side effects below are applied on a best-effort
    // basis and reported as warnings — the preference is stored either way.
    state.db.save_settings(&settings)?;

    // Reconcile the side-effecting toggles every save — not only when they
    // change — so a prior unresolved failure keeps warning until it is actually
    // fixed, rather than the banner vanishing on the next unrelated save. Both
    // are idempotent: `apply_launch_at_login` writes only on a real difference,
    // and re-setting the tray to its current visibility is a no-op. So a warning
    // here reflects the live mismatch, not merely this save's attempt.
    let mut apply_warnings: Vec<&'static str> = Vec::new();
    if !apply_launch_at_login(&app, settings.launch_at_login) {
        apply_warnings.push("launchAtLogin");
    }
    if !crate::tray::set_visible(&app, settings.show_in_menu_bar) {
        apply_warnings.push("menuBar");
    }

    let keyboard_toggled = previous.keyboard_enabled != settings.keyboard_enabled;
    let window_management_toggled =
        previous.window_management_enabled != settings.window_management_enabled;
    let drag_changed =
        window_management_toggled || previous.drag_to_snap_enabled != settings.drag_to_snap_enabled;
    let move_changed =
        window_management_toggled || previous.drag_to_move_enabled != settings.drag_to_move_enabled;
    let language_changed = previous.language != settings.language;
    let command_ime_changed =
        previous.command_ime_switch_enabled != settings.command_ime_switch_enabled;
    *state.settings.lock_safe() = settings.clone();

    // Broadcast the new settings so the window's provider adopts them — keeping
    // its snapshot in step with any change applied out of band (e.g. a future
    // tray-driven toggle) rather than only the optimistic update it just made.
    let _ = app.emit("tomari:settings-changed", settings);

    // The left/right ⌘ IME toggle is not a stored rule, so flipping it has to
    // reassemble the engine's rule set from the new setting.
    if command_ime_changed && let Err(e) = reload_engine_rules(&state) {
        tracing::warn!(error = %e, "failed to reload engine rules after IME toggle");
    }

    // The tray menu renders in the configured language, so rebuild it (on the
    // main thread, as the menu APIs require).
    if language_changed {
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || crate::tray::refresh(&handle));
    }

    // Only (re)start the event tap when keyboard customization is actually
    // toggled. Flipping unrelated preferences must not tear the tap down and
    // rebuild it, which would briefly drop key monitoring.
    #[cfg(target_os = "macos")]
    {
        if keyboard_toggled {
            crate::eventtap::restart(&app);
        }
        if drag_changed {
            crate::drag_to_snap::restart(&app);
        }
        if move_changed {
            crate::drag_to_move::restart(&app);
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (keyboard_toggled, drag_changed, move_changed);
    }

    Ok(SaveSettingsOutcome { apply_warnings })
}

/// Reconcile the macOS login item with the "Launch at login" preference.
/// Returns whether the live state matches `enabled` afterward; a failure is
/// logged and reported as `false` (the preference is still saved and can be
/// retried), rather than surfaced as a hard error. Safe to call on every save:
/// it writes only when the live state actually differs, so repeated
/// reconciliation neither duplicates the login item nor errors on a redundant
/// disable, and the returned bool reflects a real, still-unresolved mismatch.
pub(crate) fn apply_launch_at_login(app: &AppHandle, enabled: bool) -> bool {
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    // Already in the desired state: nothing to write, no mismatch to report.
    if manager.is_enabled().is_ok_and(|current| current == enabled) {
        return true;
    }
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    match result {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(error = %e, "failed to update launch-at-login");
            false
        }
    }
}

/// Ask the update endpoint whether a newer version exists. The found update is
/// parked in [`PendingUpdate`] so `install_update` can reuse it.
#[tauri::command]
pub async fn check_for_update(
    app: AppHandle,
    pending: State<'_, PendingUpdate>,
) -> CmdResult<Option<UpdateInfo>> {
    use tauri_plugin_updater::UpdaterExt;
    let update = app
        .updater()
        .map_err(|e| CmdError::other(e.to_string()))?
        .check()
        .await
        .map_err(|e| CmdError::other(e.to_string()))?;
    let info = update.as_ref().map(|u| UpdateInfo {
        version: u.version.clone(),
        notes: u.body.clone(),
    });
    *pending.0.lock_safe() = update;
    Ok(info)
}

/// Download and apply the update found by the last `check_for_update`, then
/// relaunch the app.
#[tauri::command]
pub async fn install_update(app: AppHandle, pending: State<'_, PendingUpdate>) -> CmdResult<()> {
    let update = pending
        .0
        .lock_safe()
        .take()
        .ok_or("no pending update — check for updates first")?;
    if let Err(e) = update.download_and_install(|_, _| {}, || {}).await {
        let message = e.to_string();
        // Put the update back so a retry doesn't need a fresh check.
        *pending.0.lock_safe() = Some(update);
        return Err(CmdError::other(message));
    }
    // `restart` does not guarantee an `ExitRequested` event, so release sleep
    // prevention (including the lid-close override) here before relaunching.
    crate::keepawake::cleanup_blocking(&app);
    app.restart();
}

#[tauri::command]
pub fn list_hotkeys(state: State<'_, AppState>) -> CmdResult<Vec<Hotkey>> {
    state.db.list_hotkeys().map_err(CmdError::from)
}

#[tauri::command]
pub fn save_hotkey(app: AppHandle, state: State<'_, AppState>, hotkey: Hotkey) -> CmdResult<()> {
    // Don't trust the frontend: normalize the accelerator and reject empty /
    // overlong ids and labels and bare-letter shortcuts before anything is
    // stored or registered.
    let hotkey = crate::validate::sanitize_hotkey(hotkey)?;
    let _config = state.lock_config_mutation();
    // Registration can fail even for a valid accelerator (e.g. a conflict with
    // another app), so snapshot the stored row and roll back on failure — the
    // DB must not keep a hotkey the UI reported as rejected.
    let previous = state
        .db
        .list_hotkeys()?
        .into_iter()
        .find(|h| h.id == hotkey.id);
    state.db.upsert_hotkey(&hotkey)?;

    // A failure of the hotkey being saved — or of any hotkey sharing its
    // accelerator (registration is first-come, so the saved row can win the
    // accelerator and silently knock out an existing hotkey) — warrants a
    // rollback. Failures of unrelated hotkeys are pre-existing conditions
    // (e.g. a conflict another app introduced since), already logged by
    // `register_all`, and no reason to reject this save.
    let failure: Option<CmdError> = match shortcuts::register_all(&app, state.inner()) {
        Ok(failures) => failures
            .into_iter()
            .find(|f| f.id == hotkey.id || same_accelerator(&f.accelerator, &hotkey.accelerator))
            .map(|f| {
                CmdError::shortcut_conflict(format!(
                    "could not register {}: {}",
                    f.accelerator, f.error
                ))
            }),
        Err(e) => Some(CmdError::other(e)),
    };
    if let Some(error) = failure {
        let restored = match &previous {
            Some(prev) => state.db.upsert_hotkey(prev),
            None => state.db.delete_hotkey(&hotkey.id),
        };
        if let Err(rollback) = restored {
            tracing::warn!(error = %rollback, "failed to roll back hotkey after registration failure");
        }
        if let Err(rollback) = shortcuts::register_all(&app, state.inner()) {
            tracing::warn!(error = %rollback, "failed to re-register shortcuts after rollback");
        }
        return Err(error);
    }
    Ok(())
}

/// Whether two accelerator spellings denote the same shortcut (e.g.
/// `CmdOrCtrl+K` vs `Cmd+K`), compared via the parsed form when possible.
fn same_accelerator(a: &str, b: &str) -> bool {
    use std::str::FromStr;
    use tauri_plugin_global_shortcut::Shortcut;
    match (Shortcut::from_str(a), Shortcut::from_str(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a.eq_ignore_ascii_case(b),
    }
}

#[tauri::command]
pub fn delete_hotkey(app: AppHandle, state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let _config = state.lock_config_mutation();
    state.db.delete_hotkey(&id)?;
    // Remaining hotkeys that fail to re-register are logged by `register_all`;
    // they are not this deletion's fault and must not make it look failed.
    shortcuts::register_all(&app, state.inner())
        .map(|_| ())
        .map_err(CmdError::other)
}

#[tauri::command]
pub fn list_modifier_rules(state: State<'_, AppState>) -> CmdResult<Vec<ModifierRule>> {
    state.db.list_modifier_rules().map_err(CmdError::from)
}

#[tauri::command]
pub fn save_modifier_rule(state: State<'_, AppState>, rule: ModifierRule) -> CmdResult<()> {
    let _config = state.lock_config_mutation();
    // Snapshot the stored row so a failed live reload can be rolled back — the
    // DB must not keep a rule the live engine never picked up, which would
    // "save successfully" yet take no effect until the next launch.
    let previous = state
        .db
        .list_modifier_rules()?
        .into_iter()
        .find(|r| r.id == rule.id);
    state.db.upsert_modifier_rule(&rule)?;
    if let Err(error) = reload_engine_rules(&state) {
        let restored = match &previous {
            Some(prev) => state.db.upsert_modifier_rule(prev),
            None => state.db.delete_modifier_rule(&rule.id),
        };
        if let Err(rollback) = restored {
            tracing::warn!(error = %rollback, "failed to roll back modifier rule after reload failure");
        }
        // Best-effort: bring the live engine back in step with the restored DB.
        if let Err(rollback) = reload_engine_rules(&state) {
            tracing::warn!(error = %rollback, "failed to reload engine rules after rollback");
        }
        return Err(error);
    }
    Ok(())
}

#[tauri::command]
pub fn delete_modifier_rule(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let _config = state.lock_config_mutation();
    // Snapshot the row so a failed live reload can restore it — as with save, the
    // DB must not diverge from the live engine on a reload error.
    let previous = state
        .db
        .list_modifier_rules()?
        .into_iter()
        .find(|r| r.id == id);
    state.db.delete_modifier_rule(&id)?;
    if let Err(error) = reload_engine_rules(&state) {
        if let Some(prev) = &previous
            && let Err(rollback) = state.db.upsert_modifier_rule(prev)
        {
            tracing::warn!(error = %rollback, "failed to restore modifier rule after reload failure");
        }
        // Best-effort: bring the live engine back in step with the restored DB.
        if let Err(rollback) = reload_engine_rules(&state) {
            tracing::warn!(error = %rollback, "failed to reload engine rules after rollback");
        }
        return Err(error);
    }
    Ok(())
}

fn reload_engine_rules(state: &AppState) -> CmdResult<()> {
    // The stored rules plus the built-in left/right ⌘ IME toggle, which lives
    // behind a setting rather than as an editable row.
    let mut rules = state.db.list_modifier_rules()?;
    if state.settings.lock_safe().command_ime_switch_enabled {
        rules.extend(tomari_core::defaults::command_ime_rules());
    }
    state.engine.lock_safe().set_rules(rules);
    // The live tap picks up the new rules straight from the engine, but the Caps
    // Lock HID remap is out-of-band and must be brought into step here — adding,
    // disabling or deleting a Caps Lock rule changes whether it should be on.
    // The event tap is macOS-only, so gate the call to keep a non-macOS
    // `cargo check` building (the rest of this file already gates eventtap).
    #[cfg(target_os = "macos")]
    crate::eventtap::reconcile_caps_mapping(state);
    Ok(())
}

#[tauri::command]
pub fn list_window_presets() -> Vec<WindowPreset> {
    WindowPreset::ALL.to_vec()
}

/// Snap the focused window. Returns the preset actually applied — a repeated
/// half-snap cycles 1/2 → 1/3 → 2/3, so it can differ from the request — so
/// the frontend can label it in the UI language. `None` when window management
/// is disabled.
#[tauri::command]
pub fn snap_window(
    state: State<'_, AppState>,
    preset: WindowPreset,
) -> CmdResult<Option<WindowPreset>> {
    crate::window_ops::snap(
        state.inner(),
        preset,
        crate::window_ops::SnapBehavior::Cycle,
    )
}

#[tauri::command]
pub fn move_window_to_display(
    state: State<'_, AppState>,
    direction: DisplayDirection,
) -> CmdResult<()> {
    crate::window_ops::move_to_display(state.inner(), direction)
}

#[tauri::command]
pub fn undo_window(state: State<'_, AppState>) -> CmdResult<()> {
    crate::window_ops::undo(state.inner())
}

#[tauri::command]
pub fn accessibility_status(state: State<'_, AppState>) -> bool {
    state.windows.permission_granted()
}

#[tauri::command]
pub fn request_accessibility() -> bool {
    #[cfg(target_os = "macos")]
    {
        tomari_window::request_permission()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Temporarily unregister all global shortcuts (`suspended = true`) so a chord
/// being recorded in the settings panel reaches the webview instead of firing
/// its bound action; `false` re-registers everything from the database.
#[tauri::command]
pub fn set_hotkeys_suspended(
    app: AppHandle,
    state: State<'_, AppState>,
    suspended: bool,
) -> CmdResult<()> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    // This mutates the shortcut registration and `state.shortcuts`, so it must
    // not interleave with a hotkey save's `register_all` (which would otherwise
    // leave the OS and the dispatch map disagreeing).
    let _config = state.lock_config_mutation();
    if suspended {
        app.global_shortcut()
            .unregister_all()
            .map_err(|e| CmdError::other(e.to_string()))?;
        state.shortcuts.lock_safe().clear();
        Ok(())
    } else {
        // Hotkeys that fail to come back are logged by `register_all`; the
        // recorder resuming must not error over a pre-existing conflict.
        shortcuts::register_all(&app, state.inner())
            .map(|_| ())
            .map_err(CmdError::other)
    }
}

#[tauri::command]
pub fn validate_accelerator(accelerator: String) -> AcceleratorCheck {
    match accelerator::normalize(&accelerator) {
        Ok(normalized) => AcceleratorCheck {
            valid: true,
            normalized: Some(normalized),
            error: None,
        },
        Err(e) => AcceleratorCheck {
            valid: false,
            normalized: None,
            error: Some(e.to_string()),
        },
    }
}

#[tauri::command]
pub fn run_action(app: AppHandle, state: State<'_, AppState>, action: AppAction) -> CmdResult<()> {
    actions::dispatch(&action, &app, state.inner())
}

/// Current sleep-prevention status, for the panel to render on open.
#[tauri::command]
pub fn get_keep_awake(state: State<'_, AppState>) -> crate::keepawake::KeepAwakeStatus {
    crate::keepawake::status(state.inner())
}

/// Turn sleep prevention on or off from the panel toggle. Returns the resulting
/// status; the lid-close veto is engaged in the background (it prompts for the
/// administrator password), so `lidClose` starts `pending` and settles to
/// `engaged` / `unavailable` shortly after via the `tomari:keep-awake-changed`
/// event.
#[tauri::command]
pub fn set_keep_awake(app: AppHandle, enabled: bool) -> crate::keepawake::KeepAwakeStatus {
    crate::keepawake::set(&app, enabled)
}
