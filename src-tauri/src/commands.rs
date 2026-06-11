//! Tauri commands — the bridge invoked from the React frontend. Argument names
//! here must match the keys passed from `src/lib/api.ts`.

use serde::Serialize;
use tauri::{AppHandle, State};
use tomari_core::{AppAction, AppSettings, DisplayDirection, Hotkey, ModifierRule, WindowPreset};
use tomari_keyboard::accelerator;

use crate::actions;
use crate::error::CmdError;
use crate::shortcuts;
use crate::state::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceleratorCheck {
    valid: bool,
    normalized: Option<String>,
    error: Option<String>,
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
    state.db.get_settings().map_err(CmdError::from)
}

#[tauri::command]
pub fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    mut settings: AppSettings,
) -> CmdResult<()> {
    // Serialize against a concurrent config import, which replaces every table
    // and rebuilds the engines from the database.
    let _config = state.lock_config_mutation();
    // Never trust the frontend: clamp ranges before anything reads or persists
    // the new settings.
    settings.sanitize();
    let previous = state.settings.lock().unwrap().clone();

    state.db.save_settings(&settings)?;
    state
        .engine
        .lock()
        .unwrap()
        .set_hold_threshold(settings.hold_threshold_ms);

    // Reflect the side-effecting toggles, but only when they actually change.
    if previous.launch_at_login != settings.launch_at_login {
        apply_launch_at_login(&app, settings.launch_at_login);
    }
    if previous.show_in_menu_bar != settings.show_in_menu_bar {
        crate::tray::set_visible(&app, settings.show_in_menu_bar);
    }

    let keyboard_toggled = previous.keyboard_enabled != settings.keyboard_enabled;
    let drag_changed = previous.window_management_enabled != settings.window_management_enabled
        || previous.drag_to_snap_enabled != settings.drag_to_snap_enabled;
    let language_changed = previous.language != settings.language;
    *state.settings.lock().unwrap() = settings;

    // The tray menu renders in the configured language, so rebuild it (on the
    // main thread, as the menu APIs require).
    if language_changed {
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || crate::tray::refresh(&handle));
    }

    // Only (re)start the event tap when keyboard customization is actually
    // toggled. Theme changes or dragging a slider must not tear the tap down
    // and rebuild it, which would briefly drop key monitoring.
    #[cfg(target_os = "macos")]
    {
        if keyboard_toggled {
            crate::eventtap::restart(&app);
        }
        if drag_changed {
            crate::drag_to_snap::restart(&app);
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (keyboard_toggled, drag_changed);
    }

    Ok(())
}

/// Register or deregister Tomari as a macOS login item to match the
/// "Launch at login" setting. Failures are logged rather than surfaced, since
/// the preference is still saved and can be retried.
pub(crate) fn apply_launch_at_login(app: &AppHandle, enabled: bool) {
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    if let Err(e) = result {
        tracing::warn!(error = %e, "failed to update launch-at-login");
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
    *pending.0.lock().unwrap() = update;
    Ok(info)
}

/// Download and apply the update found by the last `check_for_update`, then
/// relaunch the app.
#[tauri::command]
pub async fn install_update(app: AppHandle, pending: State<'_, PendingUpdate>) -> CmdResult<()> {
    let update = pending
        .0
        .lock()
        .unwrap()
        .take()
        .ok_or("no pending update — check for updates first")?;
    if let Err(e) = update.download_and_install(|_, _| {}, || {}).await {
        let message = e.to_string();
        // Put the update back so a retry doesn't need a fresh check.
        *pending.0.lock().unwrap() = Some(update);
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
    state.db.upsert_modifier_rule(&rule)?;
    reload_engine_rules(&state)
}

#[tauri::command]
pub fn delete_modifier_rule(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let _config = state.lock_config_mutation();
    state.db.delete_modifier_rule(&id)?;
    reload_engine_rules(&state)
}

fn reload_engine_rules(state: &AppState) -> CmdResult<()> {
    let rules = state.db.list_modifier_rules()?;
    state.engine.lock().unwrap().set_rules(rules);
    // The live tap picks up the new rules straight from the engine, but the Caps
    // Lock HID remap is out-of-band and must be brought into step here — adding,
    // disabling or deleting a Caps Lock rule changes whether it should be on.
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
    // not interleave with an import's `register_all` (which would otherwise
    // leave the OS and the dispatch map disagreeing).
    let _config = state.lock_config_mutation();
    if suspended {
        // Unregistering swallows the release of any held hotkey, which would
        // leave a Quick Peek stuck — dismiss it first (as register_all does).
        #[cfg(target_os = "macos")]
        crate::peek::cancel(&app);

        app.global_shortcut()
            .unregister_all()
            .map_err(|e| CmdError::other(e.to_string()))?;
        state.shortcuts.lock().unwrap().clear();
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

/// Export the entire configuration to a user-chosen JSON file. The native save
/// dialog and the file write happen in Rust (off the main thread, so the
/// blocking dialog does not deadlock the event loop). Resolves to the outcome —
/// saved (with the path) or cancelled.
#[tauri::command]
pub async fn export_config(app: AppHandle) -> CmdResult<crate::import_export::ExportOutcome> {
    tauri::async_runtime::spawn_blocking(move || crate::import_export::run_export(&app))
        .await
        .map_err(|e| CmdError::other(e.to_string()))?
        .map_err(CmdError::other)
}

/// Import a configuration file, replacing the current configuration after a
/// strict, all-or-nothing validation. The native open dialog, validation,
/// pre-import backup and apply all happen in Rust. Resolves to the outcome —
/// applied (with a report), rejected (with the list of problems) or cancelled.
#[tauri::command]
pub async fn import_config(app: AppHandle) -> CmdResult<crate::import_export::ImportOutcome> {
    tauri::async_runtime::spawn_blocking(move || crate::import_export::run_import(&app))
        .await
        .map_err(|e| CmdError::other(e.to_string()))?
        .map_err(CmdError::other)
}
