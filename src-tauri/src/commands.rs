//! Tauri commands — the bridge invoked from the React frontend. Argument names
//! here must match the keys passed from `src/lib/api.ts`.

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tomari_core::{AppAction, AppSettings, DisplayDirection, Hotkey, ModifierRule, PlacementSlot};
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

/// Payload of `tomari:permissions-changed`, emitted by the permission-polling
/// thread in `main.rs` whenever Accessibility or Input Monitoring transitions.
/// The frontend listens for this to keep permission banners in step with
/// System Settings without a manual refresh.
#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub struct PermissionsChanged {
    pub accessibility: bool,
    pub input_monitoring: bool,
}

/// An update found by [`check_for_update`], held until [`install_update`]
/// consumes it so the install does not have to hit the endpoint again.
///
/// The slot is tagged with a generation that every successful check bumps.
/// `install_update` downloads with the lock released — a check can land in the
/// meantime — so it remembers the generation it took and only restores the
/// update it consumed if the slot has not moved on. Without that tag a failed
/// install would put its stale update back over a fresher check result.
#[derive(Default)]
pub struct PendingUpdate(pub std::sync::Mutex<PendingUpdateSlot>);

#[derive(Default)]
pub struct PendingUpdateSlot {
    update: Option<tauri_plugin_updater::Update>,
    /// Incremented on every check that stores a result; an install compares it
    /// to decide whether restoring its update on failure is still correct.
    generation: u64,
}

/// Commands reject with a [`CmdError`] (a `{ code, message }` pair) so the
/// frontend can localize the frequent failures and fall back to the message
/// for the rest.
type CmdResult<T> = Result<T, CmdError>;

/// Run `task` on the blocking pool with the app state, off the main thread.
///
/// Tauri dispatches a synchronous command on the main thread, which also
/// serves AppKit and the webview. A command that reaches the database (a
/// `SQLITE_BUSY` wait of up to five seconds) or orchestrates a full
/// global-shortcut re-registration would hold that thread — freezing the
/// panel — for as long as it takes. Such commands are `async fn`s that hand
/// their body here. Two caveats keep this honest: the shortcut registration
/// itself is main-thread work (`shortcuts::register_all` reads the database
/// on this thread, then hands the plugin calls and the dispatch-map update to
/// the main thread as one closure — see `shortcuts::on_main_thread` for the
/// deadlock this avoids), so only the database access and the rollback
/// orchestration move off it; and the window commands stay synchronous,
/// because the screen geometry they depend on is only readable on the main
/// thread (see the note above them).
async fn off_main<T, F>(app: AppHandle, task: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce(&AppHandle, &AppState) -> CmdResult<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        task(&app, state.inner())
    })
    .await
    .map_err(|error| CmdError::other(format!("command task failed: {error}")))?
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> CmdResult<AppSettings> {
    // Return the live, sanitized in-memory settings rather than re-reading the
    // database. `AppState.settings` is the single source of truth the engines,
    // tray and taps run from; the DB is only its persistence layer. Reading the
    // row directly would let the UI drift from runtime state if a row were
    // hand-edited, out of range, or written ahead of the live update.
    Ok(state.settings.lock_safe().clone())
}

// `async fn`: Tauri dispatches synchronous commands on the main thread, and
// this one can join a tap thread and shell out to `hidutil` two or three times
// (via `eventtap`/`drag_to_snap`/`drag_to_move` restarts below) — enough to
// visibly freeze the UI. Marking it `async` moves execution onto Tauri's async
// runtime instead; `AppState` is `Send + Sync` (every field is a `Mutex` or a
// `Box<dyn Trait + Send + Sync>`), so holding the `State` across the function
// body is sound. Nothing here actually awaits — the body is unchanged sync
// code — this only changes *which thread* runs it.
#[tauri::command]
pub async fn save_settings(
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
    // Don't trust the frontend: bound the fields that drive timers before
    // anything is stored or armed.
    let settings = crate::validate::sanitize_settings(settings)?;
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

    // Global shortcuts belong to keyboard customization too. Rebuild them
    // immediately when the master switch changes so turning it off releases
    // every system registration and turning it on restores the saved set.
    if keyboard_toggled {
        let shortcut_result = shortcuts::register_all(&app, state.inner());
        if let Err(error) = &shortcut_result {
            tracing::warn!(%error, "failed to reconcile global shortcuts after keyboard toggle");
        }
    }
    // Keep reporting a previous mismatch on unrelated saves. The warning is
    // cleared only by a later successful registration pass (keyboard toggle,
    // shortcut save/delete, or recorder resume), not merely because this save
    // happened not to touch shortcuts.
    if state.shortcut_registration_incomplete() {
        apply_warnings.push("globalShortcuts");
    }

    // Broadcast the new settings so the window's provider adopts them — keeping
    // its snapshot in step with any change applied out of band (e.g. a future
    // tray-driven toggle) rather than only the optimistic update it just made.
    let _ = app.emit("tomari:settings-changed", settings.clone());

    // Whether each restartable tap should be running per the just-saved
    // settings, independent of whether this save actually restarted it — the
    // baseline `compose_apply_warnings` falls back to when no restart ran.
    let keyboard_should_run = settings.keyboard_enabled;
    let drag_should_run = settings.window_management_enabled && settings.drag_to_snap_enabled;
    let move_should_run = settings.window_management_enabled && settings.drag_to_move_enabled;

    // The left/right ⌘ IME toggle is not a stored rule, so flipping it has to
    // reassemble the engine's rule set from the new setting. Reconcile it every
    // save against the live engine — like the taps and Caps Lock remap below —
    // not only when the setting changed: an earlier save whose reload failed
    // would otherwise leave the runtime out of step with the DB forever, since
    // the next save sees `command_ime_changed == false` and never retries. Its
    // Caps Lock reconcile outcome is deliberately ignored here (only logged
    // inside `reload_engine_rules` callees): the authoritative `capsLockRemap`
    // check runs once below, after every side effect, against the final live
    // state.
    if (command_ime_changed || !command_ime_rules_live(&state, settings.command_ime_switch_enabled))
        && let Err(e) = reload_engine_rules(&state)
    {
        tracing::warn!(error = %e, "failed to reload engine rules after IME toggle");
    }
    // Report a warning whenever the live engine still does not match the saved
    // setting (a reload failed just now, or an earlier one did and this save was
    // unrelated), so the mismatch keeps surfacing until it actually heals rather
    // than the save reporting a clean success the runtime never honored.
    if !command_ime_rules_live(&state, settings.command_ime_switch_enabled) {
        apply_warnings.push("commandImeRules");
    }

    // The tray menu renders the configured language and gates window recovery
    // on the window-management switch, so either change requires a rebuild.
    if language_changed || previous.window_management_enabled != settings.window_management_enabled
    {
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || crate::tray::refresh(&handle));
    }

    // Bring the menu bar's status items up or down and redraw the auto-collapse
    // deadline. Reconciled on every save, not only when the switch changed: the
    // items are cheap to reapply and a previous failure to create them then has
    // a chance to heal, the same reasoning as the taps below.
    crate::menubar::apply_settings(&app, &previous, &settings);

    // Only (re)start a tap when its own toggle (or the window-management
    // master switch) actually changed. Flipping unrelated preferences must not
    // tear a tap down and rebuild it, which would briefly drop input
    // monitoring. Whether or not a restart ran this save, `compose_apply_warnings`
    // below checks every enabled tap's live state, so a warning never
    // disappears just because the next save was unrelated (see its doc
    // comment).
    #[cfg(target_os = "macos")]
    {
        let keyboard_restart = keyboard_toggled.then(|| crate::eventtap::restart_result(&app));
        let drag_restart = drag_changed.then(|| crate::drag_to_snap::restart_result(&app));
        let move_restart = move_changed.then(|| crate::drag_to_move::restart_result(&app));

        // The one authoritative Caps Lock remap check, after every side effect
        // that can touch it (rule reload, tap restart) has run: reconcile the
        // live HID state against what the just-saved settings and rules ask
        // for. An intermediate failure earlier in this save that the final
        // reconcile fixed raises no warning; a mismatch left over from an
        // *earlier* save keeps warning (and is retried) even when this save
        // touched nothing keyboard-related.
        let caps_remap_ok = crate::eventtap::reconcile_caps_mapping(&state);

        apply_warnings.extend(compose_apply_warnings(&ApplyWarningInputs {
            keyboard: TapCheck {
                should_run: keyboard_should_run,
                restarted_ok: keyboard_restart,
                running: crate::eventtap::is_running(),
            },
            drag_to_snap: TapCheck {
                should_run: drag_should_run,
                restarted_ok: drag_restart,
                running: crate::drag_to_snap::is_running(),
            },
            drag_to_move: TapCheck {
                should_run: move_should_run,
                restarted_ok: move_restart,
                running: crate::drag_to_move::is_running(),
            },
            caps_remap_ok,
        }));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (
            keyboard_toggled,
            drag_changed,
            move_changed,
            keyboard_should_run,
            drag_should_run,
            move_should_run,
        );
    }

    Ok(SaveSettingsOutcome { apply_warnings })
}

/// The `apply_warnings` the live state warrants right now, with no save
/// involved: launch-at-login and the ⌘ IME rules against the saved settings,
/// the global-shortcut registration, each enabled tap's liveness, and whether
/// the Caps Lock HID remap matches the rules. The settings panel reads this
/// when it opens (and each time it is shown again), so a mismatch that
/// outlived the last save — a restore that failed on quit and left its claim
/// record for this launch to retry, a `hidutil` that timed out during the wake
/// reset, a shortcut that failed to register at startup — is shown rather than
/// waiting for the next save to happen to surface it. Read-only: nothing is
/// restarted, written or reconciled here, which is also why `menuBar` is not
/// covered — the tray's visibility has no read-only probe, only the write in
/// `save_settings` reports it. Codes this cannot probe are listed in
/// `unprobed`, so the panel keeps whatever the last save said about them
/// instead of taking their absence here for "healed".
// `async fn`: the Caps Lock check waits on the tap lock, which a save or the
// wake reset can hold across `hidutil` calls (each bounded, but seconds
// long); off the main thread that wait does not freeze the panel that is
// opening.
#[tauri::command]
pub async fn get_apply_warnings(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<LiveApplyWarnings> {
    use tauri_plugin_autostart::ManagerExt;
    let settings = state.settings.lock_safe().clone();
    let mut warnings: Vec<&'static str> = Vec::new();
    if !app
        .autolaunch()
        .is_enabled()
        .is_ok_and(|current| current == settings.launch_at_login)
    {
        warnings.push("launchAtLogin");
    }
    if state.shortcut_registration_incomplete() {
        warnings.push("globalShortcuts");
    }
    if !command_ime_rules_live(&state, settings.command_ime_switch_enabled) {
        warnings.push("commandImeRules");
    }
    #[cfg(target_os = "macos")]
    {
        let live = |should_run: bool, running: bool| TapCheck {
            should_run,
            restarted_ok: None,
            running,
        };
        warnings.extend(compose_apply_warnings(&ApplyWarningInputs {
            keyboard: live(settings.keyboard_enabled, crate::eventtap::is_running()),
            drag_to_snap: live(
                settings.window_management_enabled && settings.drag_to_snap_enabled,
                crate::drag_to_snap::is_running(),
            ),
            drag_to_move: live(
                settings.window_management_enabled && settings.drag_to_move_enabled,
                crate::drag_to_move::is_running(),
            ),
            caps_remap_ok: crate::eventtap::caps_mapping_in_step(&state),
        }));
    }
    // Infallible in practice; `Result` because an async command with borrowed
    // inputs must return one.
    Ok(LiveApplyWarnings {
        warnings,
        unprobed: vec!["menuBar"],
    })
}

/// Outcome of [`get_apply_warnings`]: the codes the live state warrants, plus
/// the codes it has no read-only probe for and therefore says nothing about.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveApplyWarnings {
    warnings: Vec<&'static str>,
    unprobed: Vec<&'static str>,
}

/// Live-state inputs for one restartable tap's `apply_warnings` check (see
/// [`compose_apply_warnings`]).
struct TapCheck {
    /// Whether the just-saved settings say this tap should be running.
    should_run: bool,
    /// This save's restart outcome, if its toggle actually changed this save:
    /// `Some(tap_ok)` (already means "matches `should_run`"). `None` when
    /// nothing toggled and no restart ran.
    restarted_ok: Option<bool>,
    /// Whether the tap is live right now, checked regardless of whether a
    /// restart ran this save.
    running: bool,
}

/// Inputs to [`compose_apply_warnings`]: each tap's live state plus the Caps
/// Lock remap's final live check.
struct ApplyWarningInputs {
    keyboard: TapCheck,
    drag_to_snap: TapCheck,
    drag_to_move: TapCheck,
    /// Whether the Caps Lock HID remap matches what the just-saved settings
    /// and rules ask for, checked once against the live system state after
    /// every side effect of the save has run
    /// (`eventtap::reconcile_caps_mapping`) — never an accumulation of
    /// intermediate reconcile results, which could contradict the final state
    /// in both directions.
    caps_remap_ok: bool,
}

/// Turn a save's tap/remap live state into the `apply_warnings` codes the
/// frontend renders. Pure, and built on *final live state* rather than what
/// this particular save happened to touch: each tap is checked live
/// (`running`), not only against this save's restart outcome, and
/// `caps_remap_ok` is the post-save live check — so a warning from an earlier
/// save keeps showing until the mismatch is actually gone (flipping some
/// unrelated preference must not make it silently disappear), and an
/// intermediate failure this save recovered from raises none. A tap that
/// *did* restart this save instead uses the restart's own outcome
/// (`restarted_ok`), which already reflects whether it now matches the
/// setting.
///
/// Each warning code is pushed at most once — restart outcome and live check
/// for the same tap fold into a single flag before being turned into a code,
/// so callers never need to deduplicate the result themselves.
fn compose_apply_warnings(inputs: &ApplyWarningInputs) -> Vec<&'static str> {
    fn tap_ok(check: &TapCheck) -> bool {
        match check.restarted_ok {
            Some(ok) => ok,
            None => !check.should_run || check.running,
        }
    }

    let mut warnings = Vec::new();
    if !tap_ok(&inputs.keyboard) {
        warnings.push("keyboardTap");
    }
    if !tap_ok(&inputs.drag_to_snap) {
        warnings.push("dragToSnapTap");
    }
    if !tap_ok(&inputs.drag_to_move) {
        warnings.push("dragToMoveTap");
    }
    if !inputs.caps_remap_ok {
        warnings.push("capsLockRemap");
    }
    warnings
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
    let mut slot = pending.0.lock_safe();
    slot.update = update;
    slot.generation = slot.generation.wrapping_add(1);
    Ok(info)
}

/// Download and apply the update found by the last `check_for_update`, then
/// relaunch the app.
#[tauri::command]
pub async fn install_update(app: AppHandle, pending: State<'_, PendingUpdate>) -> CmdResult<()> {
    let (update, generation) = {
        let mut slot = pending.0.lock_safe();
        let update = slot
            .update
            .take()
            .ok_or("no pending update — check for updates first")?;
        (update, slot.generation)
    };
    if let Err(e) = update.download_and_install(|_, _| {}, || {}).await {
        let message = e.to_string();
        // Put the update back so a retry doesn't need a fresh check — but only
        // if no check has stored a newer one while the download ran. Restoring
        // over a fresher result would make the next install apply the older
        // update the UI is no longer offering.
        let mut slot = pending.0.lock_safe();
        if slot.generation == generation && slot.update.is_none() {
            slot.update = Some(update);
        }
        return Err(CmdError::other(message));
    }
    // `restart` does not guarantee an `ExitRequested` event, so release sleep
    // prevention (including the lid-close override) here before relaunching,
    // and stop the keyboard tap so a remapped modifier it holds downstream is
    // released rather than left pressed across the relaunch.
    #[cfg(target_os = "macos")]
    crate::eventtap::teardown(&app);
    crate::keepawake::cleanup_blocking(&app);
    app.restart();
}

#[tauri::command]
pub async fn list_hotkeys(app: AppHandle) -> CmdResult<Vec<Hotkey>> {
    off_main(app, |_, state| {
        state.db.list_hotkeys().map_err(CmdError::from)
    })
    .await
}

#[tauri::command]
pub async fn save_hotkey(app: AppHandle, hotkey: Hotkey) -> CmdResult<()> {
    off_main(app, move |app, state| {
        save_hotkey_blocking(app, state, hotkey)
    })
    .await
}

fn save_hotkey_blocking(app: &AppHandle, state: &AppState, hotkey: Hotkey) -> CmdResult<()> {
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
    let failure: Option<CmdError> = match shortcuts::register_all(app, state) {
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
        // Between the registration pass above and the re-registration below
        // the main thread runs its event loop for a moment, so a chord pressed
        // exactly then can reach the just-registered (about to be rolled back)
        // action once, or miss an existing hotkey it displaced. Accepted: the
        // window is one loop turn and only the user's own chord lands in it;
        // folding the failure judgment and the restore into the registration
        // closure would tie this command's rollback rules into `shortcuts`.
        let restored = match &previous {
            Some(prev) => state.db.upsert_hotkey(prev),
            None => state.db.delete_hotkey(&hotkey.id),
        };
        if let Err(rollback) = restored {
            tracing::warn!(error = %rollback, "failed to roll back hotkey after registration failure");
        }
        if let Err(rollback) = shortcuts::register_all(app, state) {
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
pub async fn delete_hotkey(app: AppHandle, id: String) -> CmdResult<()> {
    off_main(app, move |app, state| {
        delete_hotkey_blocking(app, state, id)
    })
    .await
}

fn delete_hotkey_blocking(app: &AppHandle, state: &AppState, id: String) -> CmdResult<()> {
    let _config = state.lock_config_mutation();
    // Snapshot the row before deleting so a systemic re-registration failure can
    // be rolled back: `register_all` returns `Err` only when it left the live
    // set untouched (the DB was unreadable, or the old registrations could not
    // be cleared), which would strand the OS registration and dispatch map on a
    // hotkey the DB has already dropped — the three-way desync the UI then sees.
    let previous = state.db.list_hotkeys()?.into_iter().find(|h| h.id == id);
    state.db.delete_hotkey(&id)?;
    // Remaining hotkeys that fail to re-register are logged by `register_all`
    // and returned as per-hotkey failures; they are pre-existing conflicts
    // unrelated to this deletion (removing a hotkey only frees an accelerator,
    // never breaks another), so `Ok(_)` is a success here regardless of them.
    if let Err(e) = shortcuts::register_all(app, state) {
        // Restore the deleted row so the DB matches the still-live OS set, then
        // re-register best-effort — same rollback contract as `save_hotkey`.
        if let Some(prev) = &previous {
            if let Err(rollback) = state.db.upsert_hotkey(prev) {
                tracing::warn!(error = %rollback, "failed to restore hotkey after delete re-registration failure");
            }
            if let Err(rollback) = shortcuts::register_all(app, state) {
                tracing::warn!(error = %rollback, "failed to re-register shortcuts after delete rollback");
            }
        }
        return Err(CmdError::other(e));
    }
    Ok(())
}

#[tauri::command]
pub async fn list_modifier_rules(app: AppHandle) -> CmdResult<Vec<ModifierRule>> {
    off_main(app, |_, state| {
        state.db.list_modifier_rules().map_err(CmdError::from)
    })
    .await
}

// `async fn`: a failed reload can call `reload_engine_rules` (and thus
// `hidutil` via `reconcile_caps_mapping`) up to twice on the rollback path,
// synchronously. Moving it off the main thread keeps a slow `hidutil` from
// freezing the UI, same rationale as `save_settings` above.
#[tauri::command]
pub async fn save_modifier_rule(state: State<'_, AppState>, rule: ModifierRule) -> CmdResult<()> {
    let _config = state.lock_config_mutation();
    // Don't trust the frontend: reject empty / overlong / reserved ids and
    // labels, contradictory hyper+remap rules, unsendable tap keystrokes, and
    // rules that collide with another stored rule (or the reserved left/right ⌘
    // IME-toggle slots) on the same modifier and side, before anything is
    // stored. The stored set is also the snapshot used for both the collision
    // check and the rollback below.
    let existing = state.db.list_modifier_rules()?;
    let rule = crate::validate::sanitize_modifier_rule(rule, &existing)?;
    // Snapshot the stored row so a failed live reload can be rolled back — the
    // DB must not keep a rule the live engine never picked up, which would
    // "save successfully" yet take no effect until the next launch.
    let previous = existing.into_iter().find(|r| r.id == rule.id);
    state.db.upsert_modifier_rule(&rule)?;
    match reload_engine_rules(&state) {
        // The engine reloaded but `hidutil` left the Caps Lock remap out of
        // step — not a reason to roll back the save (the rule *is* live in the
        // engine), just a live mismatch to log. `save_settings` surfaces the
        // equivalent case via `apply_warnings`; this command's return type has
        // no room for one, so a log is the closest equivalent here.
        Ok(false) => {
            tracing::warn!("caps-lock HID remap did not match the reloaded rules after save")
        }
        Ok(true) => {}
        Err(error) => {
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
    }
    Ok(())
}

// `async fn`: same rationale as `save_modifier_rule` — a failed reload can
// shell out to `hidutil` synchronously up to twice on the rollback path.
#[tauri::command]
pub async fn delete_modifier_rule(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let _config = state.lock_config_mutation();
    // Snapshot the row so a failed live reload can restore it — as with save, the
    // DB must not diverge from the live engine on a reload error.
    let previous = state
        .db
        .list_modifier_rules()?
        .into_iter()
        .find(|r| r.id == id);
    state.db.delete_modifier_rule(&id)?;
    match reload_engine_rules(&state) {
        // Same as `save_modifier_rule`: the engine reloaded but the Caps Lock
        // remap did not follow; log it rather than roll back a delete that did
        // take effect in the engine.
        Ok(false) => {
            tracing::warn!("caps-lock HID remap did not match the reloaded rules after delete")
        }
        Ok(true) => {}
        Err(error) => {
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
    }
    Ok(())
}

/// Reload the engine's rule set from the database, returning whether the Caps
/// Lock HID remap ended up matching it. `Err` is reserved for the DB read
/// failing outright; a `hidutil` failure while reconciling the remap is instead
/// reported as `Ok(false)`, since the engine itself did reload successfully —
/// only the out-of-band remap is left out of step. `save_modifier_rule` /
/// `delete_modifier_rule` log a `false`; `save_settings` ignores it outright,
/// because its `capsLockRemap` warning comes from one authoritative live
/// check after all of the save's side effects, not from intermediate results.
/// Whether the live engine's rule set is in step with `want` for the built-in
/// ⌘ IME-toggle rules: all of them present when the setting is on, none when it
/// is off. Used by `save_settings` to detect a rule reload that silently failed
/// (the DB and settings say one thing, the runtime another).
fn command_ime_rules_live(state: &AppState, want: bool) -> bool {
    let ime_rules = tomari_core::defaults::command_ime_rules();
    let engine = state.engine.lock_safe();
    let present = ime_rules
        .iter()
        .filter(|r| engine.contains_rule_id(&r.id))
        .count();
    if want {
        present == ime_rules.len()
    } else {
        present == 0
    }
}

fn reload_engine_rules(state: &AppState) -> CmdResult<bool> {
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
    // (Not while Caps Lock is physically held: the reconcile then waits for the
    // release, so the key finishes its hold under the mapping it began with.)
    // The event tap is macOS-only, so gate the call to keep a non-macOS
    // `cargo check` building (the rest of this file already gates eventtap).
    #[cfg(target_os = "macos")]
    let caps_ok = crate::eventtap::reconcile_caps_mapping(state);
    #[cfg(not(target_os = "macos"))]
    let caps_ok = true;
    Ok(caps_ok)
}

// The window commands stay synchronous — on the main thread — on purpose.
// They do reach the focused app over Accessibility, but they also read screen
// geometry (`WindowManager::work_area` / `screen_work_areas`), which AppKit
// only answers on the main thread: off it the manager falls back to the main
// display, so a window on a secondary display would be normalized, recalled
// and moved against the wrong work area. Each Accessibility call is bounded by
// the messaging timeout, and every operation holds `lock_window_mutation` so
// the panel, the shortcuts and the drag worker cannot interleave one.
#[tauri::command]
pub fn undo_window(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<crate::window_ops::HistoryActionResult> {
    let result = crate::window_ops::undo(state.inner());
    crate::tray::refresh(&app);
    result
}

#[tauri::command]
pub fn redo_window(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<crate::window_ops::HistoryActionResult> {
    let result = crate::window_ops::redo(state.inner());
    crate::tray::refresh(&app);
    result
}

#[tauri::command]
pub fn get_window_history_status(
    state: State<'_, AppState>,
) -> crate::window_ops::WindowHistoryStatus {
    crate::window_ops::window_history_status(state.inner())
}

#[tauri::command]
pub fn get_placement_context(
    state: State<'_, AppState>,
) -> CmdResult<crate::window_ops::PlacementContext> {
    crate::window_ops::placement_context(state.inner())
}

#[tauri::command]
pub fn capture_window_placement(
    state: State<'_, AppState>,
    target: crate::window_ops::WindowTarget,
    slot: PlacementSlot,
) -> CmdResult<crate::window_ops::PlacementEditResult> {
    crate::window_ops::capture_placement(state.inner(), &target, slot)
}

#[tauri::command]
pub fn forget_window_placement(
    state: State<'_, AppState>,
    target: crate::window_ops::WindowTarget,
    slot: PlacementSlot,
) -> CmdResult<crate::window_ops::PlacementEditResult> {
    crate::window_ops::forget_placement(state.inner(), &target, slot)
}

#[tauri::command]
pub fn undo_window_placement_edit(
    state: State<'_, AppState>,
) -> CmdResult<crate::window_ops::HistoryActionResult> {
    crate::window_ops::undo_placement_edit(state.inner())
}

#[tauri::command]
pub fn recall_window_placement(
    app: AppHandle,
    state: State<'_, AppState>,
    target: crate::window_ops::WindowTarget,
) -> CmdResult<PlacementSlot> {
    let result = crate::window_ops::recall_placement_for(state.inner(), &target);
    crate::tray::refresh(&app);
    result
}

#[tauri::command]
pub fn move_window_to_display_and_recall(
    app: AppHandle,
    state: State<'_, AppState>,
    target: crate::window_ops::WindowTarget,
    direction: DisplayDirection,
) -> CmdResult<crate::window_ops::MoveRecallResult> {
    let result =
        crate::window_ops::move_to_display_and_recall_for(state.inner(), &target, direction);
    crate::tray::refresh(&app);
    result
}

/// Everything the frontend's setup checklist needs at startup, pulled with one
/// command once the WebView is ready. Pulled rather than pushed: an event
/// emitted from `setup` would race the WebView load and could arrive before
/// any listener exists.
#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub struct SetupStatus {
    pub first_run: bool,
    /// Previously granted permissions are believed lost to an app update
    /// (ad-hoc signing revokes them on every update). See [`crate::regrant`].
    pub update_regrant: bool,
    pub accessibility: bool,
    pub input_monitoring: bool,
}

#[tauri::command]
pub fn setup_status(state: State<'_, AppState>) -> SetupStatus {
    SetupStatus {
        first_run: state.first_run,
        update_regrant: state.update_regrant(),
        accessibility: state.windows.permission_granted(),
        input_monitoring: input_monitoring_status(),
    }
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

#[tauri::command]
pub fn input_monitoring_status() -> bool {
    #[cfg(target_os = "macos")]
    {
        crate::eventtap::input_monitoring_granted()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

#[tauri::command]
pub fn request_input_monitoring() -> bool {
    #[cfg(target_os = "macos")]
    {
        crate::eventtap::request_input_monitoring()
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
pub async fn set_hotkeys_suspended(app: AppHandle, suspended: bool) -> CmdResult<()> {
    off_main(app, move |app, state| {
        set_hotkeys_suspended_blocking(app, state, suspended)
    })
    .await
}

fn set_hotkeys_suspended_blocking(
    app: &AppHandle,
    state: &AppState,
    suspended: bool,
) -> CmdResult<()> {
    // This mutates the shortcut registration and `state.shortcuts`, so it must
    // not interleave with a hotkey save's `register_all` (which would otherwise
    // leave the OS and the dispatch map disagreeing).
    let _config = state.lock_config_mutation();
    if suspended {
        shortcuts::suspend_all(app).map_err(CmdError::other)
    } else {
        // Hotkeys that fail to come back are logged by `register_all`; the
        // recorder resuming must not error over a pre-existing conflict.
        shortcuts::register_all(app, state)
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
    // The same validator every stored action passes: an action that would be
    // refused on save is refused to run once, too.
    let action = crate::validate::sanitize_app_action(action)?;
    actions::dispatch(&action, &app, state.inner())
}

/// Current sleep-prevention status, for the panel to render on open.
#[tauri::command]
pub fn get_keep_awake(state: State<'_, AppState>) -> crate::keepawake::KeepAwakeStatus {
    crate::keepawake::status(state.inner())
}

/// Turn sleep prevention on or off from the panel toggle. Returns the resulting
/// status; the lid-close veto is engaged in the background (it prompts for the
/// administrator password), so `phase` remains `enabling` / `disabling` until
/// it settles via the `tomari:keep-awake-changed` event. Every toggle surface
/// stays disabled during those phases.
#[tauri::command]
pub fn set_keep_awake(
    app: AppHandle,
    enabled: bool,
    options: Option<crate::keepawake::KeepAwakeOptions>,
) -> crate::keepawake::KeepAwakeStatus {
    crate::keepawake::set(&app, enabled, options)
}

#[tauri::command]
pub fn configure_keep_awake(
    app: AppHandle,
    options: crate::keepawake::KeepAwakeOptions,
) -> crate::keepawake::KeepAwakeStatus {
    crate::keepawake::configure(&app, options)
}

#[tauri::command]
pub fn cancel_keep_awake_transition(app: AppHandle) -> crate::keepawake::KeepAwakeStatus {
    crate::keepawake::cancel_transition(&app)
}

#[tauri::command]
pub fn retry_keep_awake_transition(app: AppHandle) -> crate::keepawake::KeepAwakeStatus {
    crate::keepawake::retry(&app)
}

/// Leave a lid-close override found at launch in place (see
/// `KeepAwakeNotice::LeftoverOverride`) and forget the marker that pointed at it.
#[tauri::command]
pub fn dismiss_keep_awake_recovery(app: AppHandle) -> crate::keepawake::KeepAwakeStatus {
    crate::keepawake::dismiss_leftover(&app)
}

/// Whether menu bar tidying is on and, if so, whether it is collapsed right
/// now — for the panel to render on open.
#[tauri::command]
pub fn get_menu_bar(state: State<'_, AppState>) -> crate::menubar::MenuBarStatus {
    crate::menubar::status(state.inner())
}

/// Inspect the real menu bar arrangement. The Accessibility scan can message
/// every running app that owns a status item, so isolate it on the blocking
/// pool instead of occupying AppKit's main thread or an async runtime worker.
#[tauri::command]
pub async fn list_menu_bar_items(app: AppHandle) -> CmdResult<crate::menubar::MenuBarInventory> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        crate::menubar::inventory(&app, state.inner())
    })
    .await
    .map_err(|error| CmdError::other(format!("menu bar inventory task failed: {error}")))
}

/// Move one item from the latest inventory snapshot across Tomari's divider.
/// This synthesizes a real Command-drag and performs blocking AX verification,
/// so it runs on the blocking pool rather than AppKit's main thread or an async
/// runtime worker.
#[tauri::command]
pub async fn move_menu_bar_item(
    app: AppHandle,
    item_id: String,
    target_zone: crate::menubar::MenuBarItemZone,
) -> CmdResult<crate::menubar::MenuBarMoveResult> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        crate::menubar::move_item(&app, state.inner(), &item_id, target_zone)
    })
    .await
    .map_err(|error| CmdError::other(format!("menu bar movement task failed: {error}")))
}

/// Expand or collapse the tidied menu bar items from the panel. A no-op while
/// the feature is off, which the returned status reflects.
#[tauri::command]
pub fn set_menu_bar_collapsed(app: AppHandle, collapsed: bool) -> crate::menubar::MenuBarStatus {
    crate::menubar::set_collapsed(&app, collapsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `TapCheck` that was not touched this save: only the live state
    /// matters.
    fn not_restarted(should_run: bool, running: bool) -> TapCheck {
        TapCheck {
            should_run,
            restarted_ok: None,
            running,
        }
    }

    /// A `TapCheck` whose toggle changed this save, so its restart outcome is
    /// authoritative regardless of `running` (a real restart already reads the
    /// live state itself).
    fn restarted(should_run: bool, restart_ok: bool) -> TapCheck {
        TapCheck {
            should_run,
            restarted_ok: Some(restart_ok),
            running: restart_ok,
        }
    }

    fn inputs(
        keyboard: TapCheck,
        drag_to_snap: TapCheck,
        drag_to_move: TapCheck,
    ) -> ApplyWarningInputs {
        ApplyWarningInputs {
            keyboard,
            drag_to_snap,
            drag_to_move,
            caps_remap_ok: true,
        }
    }

    fn test_state(engine_rules: Vec<ModifierRule>) -> AppState {
        use tomari_core::{Database, Rect};
        use tomari_keyboard::ModifierEngine;
        use tomari_window::MockWindowManager;
        AppState::new(
            Database::open_in_memory().unwrap(),
            ModifierEngine::new(engine_rules),
            Box::new(MockWindowManager::new(Rect::new(0.0, 0.0, 100.0, 100.0))),
            AppSettings::default(),
            false,
        )
    }

    #[test]
    fn command_ime_rules_live_tracks_whether_the_engine_matches_the_setting() {
        let ime_rules = tomari_core::defaults::command_ime_rules();

        // Engine carrying the built-in IME rules: "on" is in step, "off" is not.
        let with_rules = test_state(ime_rules.clone());
        assert!(command_ime_rules_live(&with_rules, true));
        assert!(!command_ime_rules_live(&with_rules, false));

        // Engine missing them (a reload that failed to add them): "on" is out of
        // step — the case that must raise the commandImeRules warning.
        let without_rules = test_state(vec![]);
        assert!(!command_ime_rules_live(&without_rules, true));
        assert!(command_ime_rules_live(&without_rules, false));
    }

    #[test]
    fn a_fully_healthy_save_warns_about_nothing() {
        let warnings = compose_apply_warnings(&inputs(
            not_restarted(true, true),
            not_restarted(false, false),
            not_restarted(false, false),
        ));
        assert!(warnings.is_empty());
    }

    #[test]
    fn a_disabled_feature_never_warns_even_if_not_running() {
        // Not running is expected when the feature is off — no warning.
        let warnings = compose_apply_warnings(&inputs(
            not_restarted(false, false),
            not_restarted(false, false),
            not_restarted(false, false),
        ));
        assert!(warnings.is_empty());
    }

    #[test]
    fn an_enabled_but_dead_tap_warns_even_without_a_restart_this_save() {
        // Regression guard: a keyboard tap that died on an earlier save must
        // keep warning on a later save that only changed an unrelated
        // preference (e.g. language) and so never touched the tap.
        let warnings = compose_apply_warnings(&inputs(
            not_restarted(true, false),
            not_restarted(false, false),
            not_restarted(false, false),
        ));
        assert_eq!(warnings, vec!["keyboardTap"]);
    }

    #[test]
    fn a_failed_restart_warns_regardless_of_the_stale_running_flag() {
        let warnings = compose_apply_warnings(&inputs(
            not_restarted(false, false),
            restarted(true, false),
            not_restarted(false, false),
        ));
        assert_eq!(warnings, vec!["dragToSnapTap"]);
    }

    #[test]
    fn a_successful_restart_clears_any_prior_warning() {
        let warnings = compose_apply_warnings(&inputs(
            not_restarted(false, false),
            not_restarted(false, false),
            restarted(true, true),
        ));
        assert!(warnings.is_empty());
    }

    #[test]
    fn independent_taps_each_report_their_own_code() {
        let warnings = compose_apply_warnings(&inputs(
            not_restarted(true, false),
            restarted(true, false),
            restarted(true, true),
        ));
        assert_eq!(warnings, vec!["keyboardTap", "dragToSnapTap"]);
    }

    #[test]
    fn a_live_caps_remap_mismatch_warns_even_on_an_unrelated_save() {
        // `caps_remap_ok` is the post-save live check, run on every save —
        // so a mismatch left by an earlier save's `hidutil` failure keeps
        // warning here even though this save touched nothing
        // keyboard-related and the healthy taps raise nothing themselves.
        let mut all_inputs = inputs(
            not_restarted(true, true),
            not_restarted(false, false),
            not_restarted(false, false),
        );
        all_inputs.caps_remap_ok = false;
        let warnings = compose_apply_warnings(&all_inputs);
        assert_eq!(warnings, vec!["capsLockRemap"]);
    }

    #[test]
    fn a_matching_final_caps_state_warns_nothing_whatever_happened_mid_save() {
        // The final live check is the only caps input: a mid-save reconcile
        // failure that the last reconcile recovered from is invisible here by
        // construction, so a matching final state must yield no warning.
        let all_inputs = inputs(
            restarted(true, true),
            not_restarted(false, false),
            not_restarted(false, false),
        );
        assert!(compose_apply_warnings(&all_inputs).is_empty());
    }
}
