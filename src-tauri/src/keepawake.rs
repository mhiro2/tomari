//! Sleep prevention ("keep awake") for long-running background work — e.g.
//! letting an AI agent keep running after the laptop lid is shut.
//!
//! macOS exposes two layers with very different guarantees, so we use both:
//!
//! * An **IOKit power assertion** (`PreventUserIdleSystemSleep`) stops the
//!   system from idle-sleeping. It needs no permission and is released cleanly,
//!   but macOS deliberately ignores it once the lid closes (a thermal safety
//!   choice), so on its own it only covers the lid-open case.
//! * **`pmset disablesleep 1`** sets the kernel `SleepDisabled` flag, which also
//!   vetoes lid-close (clamshell) sleep. It requires administrator rights and
//!   persists until cleared, so it is engaged behind an authentication prompt
//!   and always paired with a failsafe that can clear it again.
//!
//! Keep-awake is **session state**: it always starts off at launch and is never
//! persisted as "on".
//!
//! Two invariants keep the lid-close override from ever stranding the Mac in a
//! never-sleep state:
//!
//! * **Write-ahead marker.** A marker file under the data directory is written
//!   *before* `disablesleep` is enabled and removed *after* it is cleared, so a
//!   crash at any point leaves a record [`reconcile_on_launch`] can act on. An
//!   unreadable sleep state is treated as "unknown" — the marker is kept, never
//!   dropped.
//! * **Ownership.** We only ever clear a `disablesleep` we turned on ourselves
//!   (`we_own_override`); a value already set by the user or another process is
//!   left untouched.
//!
//! The slow, admin-authed `pmset` calls are serialized through `LID_OP_LOCK`
//! and always drive the system toward the *current* desired state, so rapid
//! toggles collapse to the last one instead of racing.

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;

/// Event emitted whenever the keep-awake state changes, so every surface (the
/// panel toggle, the tray checkmark) can stay in sync regardless of which one
/// initiated the change. Matches the `tomari:` event convention used elsewhere.
const CHANGED_EVENT: &str = "tomari:keep-awake-changed";

/// Serializes the slow, admin-authed `pmset` operations. Under this lock each
/// worker drives the lid-close override toward whatever the *current* desired
/// state is, so overlapping toggles collapse to the latest rather than racing.
#[cfg(target_os = "macos")]
static LID_OP_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Set once [`cleanup_blocking`] begins (the process is exiting). It makes
/// [`engage`] refuse, so a toggle that races the shutdown cannot spawn a worker
/// that re-enables the override after cleanup has already cleared it.
static SHUTTING_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Runtime keep-awake state. Not persisted — always starts inactive.
#[derive(Debug, Default)]
pub struct KeepAwake {
    /// Sleep prevention is on (an idle-sleep assertion is held).
    active: bool,
    /// The held IOKit power-assertion id, if any.
    assertion: Option<u32>,
    /// Lid-close veto status, surfaced to the UI.
    lid_close: LidCloseState,
    /// Whether *we* turned `disablesleep` on (vs. it being set before us). We
    /// only ever clear an override we engaged ourselves.
    we_own_override: bool,
}

/// The state of the lid-close veto (`pmset disablesleep`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LidCloseState {
    /// Not engaged.
    #[default]
    Off,
    /// Administrator authorization is in progress.
    Pending,
    /// Lid-close sleep is vetoed — work continues with the lid shut.
    Engaged,
    /// Could not be engaged (authorization declined); lid-open idle prevention
    /// is still active.
    Unavailable,
}

/// The keep-awake status surfaced to the tray and the frontend.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeepAwakeStatus {
    /// Sleep prevention is on.
    pub active: bool,
    /// Lid-close veto state.
    pub lid_close: LidCloseState,
}

/// The current keep-awake status.
pub fn status(state: &AppState) -> KeepAwakeStatus {
    let k = state.keep_awake.lock().unwrap();
    KeepAwakeStatus {
        active: k.active,
        lid_close: k.lid_close,
    }
}

/// Turn sleep prevention on or off, returning the resulting status.
pub fn set(app: &AppHandle, enabled: bool) -> KeepAwakeStatus {
    if enabled { engage(app) } else { disengage(app) }
}

/// Flip sleep prevention. Used by the tray item and the `ToggleKeepAwake`
/// action (hotkeys/leader/taps).
pub fn toggle(app: &AppHandle) -> KeepAwakeStatus {
    let active = app.state::<AppState>().keep_awake.lock().unwrap().active;
    set(app, !active)
}

fn engage(app: &AppHandle) -> KeepAwakeStatus {
    let state = app.state::<AppState>();
    // Once shutdown cleanup has begun, refuse to turn on — otherwise a worker
    // spawned here could re-enable the lid-close override after cleanup cleared
    // it (notably during the updater's restart), leaving the Mac unable to sleep.
    if SHUTTING_DOWN.load(std::sync::atomic::Ordering::Acquire) {
        return status(state.inner());
    }
    {
        let mut k = state.keep_awake.lock().unwrap();
        if k.active {
            return KeepAwakeStatus {
                active: true,
                lid_close: k.lid_close,
            };
        }
        k.active = true;
        // The idle-sleep assertion is instant and needs no permission, so take
        // it synchronously — sleep prevention is effective immediately for the
        // lid-open case even before the (slower) lid-close veto is authorized.
        #[cfg(target_os = "macos")]
        {
            k.lid_close = LidCloseState::Pending;
            match create_assertion() {
                Ok(id) => k.assertion = Some(id),
                Err(rc) => tracing::warn!(rc, "failed to create power assertion"),
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            k.lid_close = LidCloseState::Off;
        }
    }
    // Reflect "on" immediately, then engage the lid-close veto in the
    // background — its admin-auth dialog must not block the caller.
    notify(app);
    #[cfg(target_os = "macos")]
    spawn_reconcile(app.clone());
    status(state.inner())
}

fn disengage(app: &AppHandle) -> KeepAwakeStatus {
    let state = app.state::<AppState>();
    {
        let mut k = state.keep_awake.lock().unwrap();
        k.active = false;
        #[cfg(target_os = "macos")]
        if let Some(id) = k.assertion.take() {
            release_assertion(id);
        }
        // The actual `pmset 0` happens in the reconcile worker; once the toggle
        // is off the lid-close state no longer drives any UI, so reflect it off.
        k.lid_close = LidCloseState::Off;
    }
    notify(app);
    #[cfg(target_os = "macos")]
    spawn_reconcile(app.clone());
    status(state.inner())
}

/// Emit the change event and rebuild the tray menu (on the main thread, as the
/// menu APIs require) so the panel and the tray checkmark both follow.
fn notify(app: &AppHandle) {
    let status = status(app.state::<AppState>().inner());
    let _ = app.emit(CHANGED_EVENT, status);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || crate::tray::refresh(&handle));
}

/// What [`reconcile_on_launch`] should do given a leftover marker and the
/// system's current `SleepDisabled` state (`None` = could not be read). Pure,
/// so it is unit-tested.
#[derive(Debug, PartialEq, Eq)]
enum ReconcileAction {
    /// No marker — we never engaged the override; leave the system alone.
    Nothing,
    /// Marker, but the sleep state could not be read; keep the marker so a
    /// later run can retry rather than risk dropping a real override.
    Keep,
    /// Marker but the override is already gone (e.g. a reboot cleared it);
    /// just drop the stale marker.
    RemoveMarker,
    /// Marker and the override is still set after an unclean exit; clear it.
    ClearOverride,
}

fn reconcile_decision(marker_present: bool, sleep_disabled: Option<bool>) -> ReconcileAction {
    match (marker_present, sleep_disabled) {
        (false, _) => ReconcileAction::Nothing,
        (true, None) => ReconcileAction::Keep,
        (true, Some(false)) => ReconcileAction::RemoveMarker,
        (true, Some(true)) => ReconcileAction::ClearOverride,
    }
}

/// Clear a lid-close override left behind by a previous run that exited without
/// cleaning up (a crash or a forced kill). Keep-awake never persists as "on",
/// so at launch the intended state is always off: any override we still own
/// must go. This is the one place an auth prompt can appear at launch, and only
/// after an unclean exit with the override still set (a reboot clears it).
pub fn reconcile_on_launch(_app: &AppHandle) {
    #[cfg(target_os = "macos")]
    match reconcile_decision(marker_exists(), read_sleep_disabled()) {
        ReconcileAction::Nothing => {}
        ReconcileAction::Keep => {
            tracing::warn!("could not read sleep state at launch; keeping the keep-awake marker")
        }
        ReconcileAction::RemoveMarker => remove_marker(),
        ReconcileAction::ClearOverride => {
            tracing::warn!("clearing a leftover lid-close sleep override from a previous run");
            if run_disablesleep(false) {
                remove_marker();
            }
        }
    }
}

/// Release everything before the process exits. Runs synchronously from the
/// `RunEvent::ExitRequested` handler (and from the updater before it relaunches)
/// so the lid-close override never outlives Tomari. Best-effort: if clearing the
/// override fails (auth declined) or an op is still in flight, the write-ahead
/// marker is kept so the next launch's reconcile retries.
pub fn cleanup_blocking(app: &AppHandle) {
    // Block any further engages for the rest of the process lifetime, so a
    // toggle racing the shutdown can't re-strand the override after we clear it.
    SHUTTING_DOWN.store(true, std::sync::atomic::Ordering::Release);
    let state = app.state::<AppState>();
    {
        let mut k = state.keep_awake.lock().unwrap();
        k.active = false;
        #[cfg(target_os = "macos")]
        if let Some(id) = k.assertion.take() {
            release_assertion(id);
        }
    }
    #[cfg(target_os = "macos")]
    {
        // Serialize with any in-flight reconcile worker (it holds this lock
        // across its `pmset` call) so we cannot clear the override just before a
        // late `pmset 1` re-enables it and strands the Mac awake. This may
        // briefly wait on an auth dialog the worker itself triggered; the
        // write-ahead marker is the backstop for anything that still slips past.
        // Lock order is always LID_OP_LOCK → keep_awake (never the reverse).
        let _op = LID_OP_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let we_own = state.keep_awake.lock().unwrap().we_own_override;
        if (we_own || marker_exists()) && run_disablesleep(false) {
            remove_marker();
            state.keep_awake.lock().unwrap().we_own_override = false;
        }
    }
}

// ---------------------------------------------------------------------------
// macOS implementation
// ---------------------------------------------------------------------------

/// Reconcile the lid-close veto on a worker thread, since the admin-auth dialog
/// blocks.
#[cfg(target_os = "macos")]
fn spawn_reconcile(app: AppHandle) {
    std::thread::spawn(move || reconcile_lid_close(&app));
}

/// Drive `pmset disablesleep` toward the current desired state. Serialized by
/// `LID_OP_LOCK` and keyed off the live `active` flag, so a burst of toggles
/// settles on the last one: a worker that finds keep-awake already off (because
/// the user toggled back) simply clears anything an earlier worker engaged.
#[cfg(target_os = "macos")]
fn reconcile_lid_close(app: &AppHandle) {
    let _op = LID_OP_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let state = app.state::<AppState>();
    let (active, we_own) = {
        let k = state.keep_awake.lock().unwrap();
        (k.active, k.we_own_override)
    };

    if active {
        // Read the current state once. If we already own the override, it is by
        // definition on.
        let sleep_disabled = if we_own {
            Some(true)
        } else {
            read_sleep_disabled()
        };
        match sleep_disabled {
            // Already vetoed — by us, or by the user/another process. Either way
            // the lid-close guarantee holds; never take ownership of a value we
            // did not set, so we never clear someone else's.
            Some(true) => state.keep_awake.lock().unwrap().lid_close = LidCloseState::Engaged,
            // Safe to enable. Persist the failsafe marker *before* enabling: a
            // crash in between then leaves a record the next launch reconciles,
            // and a marker with the override never actually set is harmlessly
            // dropped at next launch. If the marker can't be written, do not
            // enable — we could not guarantee recovery.
            Some(false) => {
                let (lid_close, owned) = if write_marker() && run_disablesleep(true) {
                    (LidCloseState::Engaged, true)
                } else {
                    remove_marker();
                    (LidCloseState::Unavailable, false)
                };
                let mut k = state.keep_awake.lock().unwrap();
                k.lid_close = lid_close;
                k.we_own_override = owned;
            }
            // Sleep state unreadable: don't risk clobbering an override we can't
            // see, and don't claim a guarantee we can't make.
            None => {
                tracing::warn!("could not read sleep state; not engaging lid-close veto");
                state.keep_awake.lock().unwrap().lid_close = LidCloseState::Unavailable;
            }
        }
    } else {
        if we_own && run_disablesleep(false) {
            remove_marker();
            state.keep_awake.lock().unwrap().we_own_override = false;
        }
        state.keep_awake.lock().unwrap().lid_close = LidCloseState::Off;
    }
    notify(app);
}

/// Set or clear `pmset disablesleep` with administrator privileges, via the
/// standard macOS auth dialog. Returns whether it succeeded.
#[cfg(target_os = "macos")]
fn run_disablesleep(on: bool) -> bool {
    let value = if on { "1" } else { "0" };
    let script = format!(
        "do shell script \"/usr/bin/pmset -a disablesleep {value}\" with administrator privileges"
    );
    match std::process::Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(&script)
        .status()
    {
        Ok(status) if status.success() => true,
        Ok(status) => {
            tracing::warn!(code = ?status.code(), on, "pmset disablesleep was not applied (auth declined?)");
            false
        }
        Err(e) => {
            tracing::warn!(error = %e, on, "failed to run osascript for pmset disablesleep");
            false
        }
    }
}

/// Read the kernel `SleepDisabled` flag from `pmset -g` (no privileges needed).
/// `None` if `pmset` could not be run — treated as "unknown", never as "off".
#[cfg(target_os = "macos")]
fn read_sleep_disabled() -> Option<bool> {
    let output = std::process::Command::new("/usr/bin/pmset")
        .arg("-g")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        if let Some(rest) = line.trim_start().strip_prefix("SleepDisabled") {
            return Some(rest.trim().starts_with('1'));
        }
    }
    // The flag is simply absent from `pmset -g` when sleep is not disabled.
    Some(false)
}

/// Path to the failsafe marker: present while we may hold a lid-close override.
#[cfg(target_os = "macos")]
fn marker_path() -> Option<std::path::PathBuf> {
    tomari_core::AppPaths::resolve()
        .ok()
        .map(|p| p.data_dir.join("keepawake.lock"))
}

#[cfg(target_os = "macos")]
fn marker_exists() -> bool {
    marker_path().is_some_and(|p| p.exists())
}

/// Write the failsafe marker, returning whether it is now durably on disk. The
/// override is only enabled when this succeeds, so a marker always guards a live
/// override.
#[cfg(target_os = "macos")]
fn write_marker() -> bool {
    let Some(path) = marker_path() else {
        tracing::warn!("could not resolve the data directory for the keep-awake marker");
        return false;
    };
    match std::fs::write(&path, b"1") {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(error = %e, "failed to write keep-awake marker");
            false
        }
    }
}

#[cfg(target_os = "macos")]
fn remove_marker() {
    if let Some(path) = marker_path() {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(error = %e, "failed to remove keep-awake marker"),
        }
    }
}

#[cfg(target_os = "macos")]
fn create_assertion() -> Result<u32, i32> {
    sys::create()
}

#[cfg(target_os = "macos")]
fn release_assertion(id: u32) {
    sys::release(id);
}

#[cfg(target_os = "macos")]
mod sys {
    use core_foundation::base::TCFType;
    use core_foundation::string::{CFString, CFStringRef};

    /// `kIOPMAssertionLevelOn`.
    const ASSERTION_LEVEL_ON: u32 = 255;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPMAssertionCreateWithName(
            assertion_type: CFStringRef,
            assertion_level: u32,
            assertion_name: CFStringRef,
            assertion_id: *mut u32,
        ) -> i32;
        fn IOPMAssertionRelease(assertion_id: u32) -> i32;
    }

    /// Create a `PreventUserIdleSystemSleep` assertion, returning its id (or the
    /// non-zero `IOReturn` on failure).
    pub fn create() -> Result<u32, i32> {
        let kind = CFString::new("PreventUserIdleSystemSleep");
        let name = CFString::new("Tomari keep awake");
        let mut id: u32 = 0;
        let rc = unsafe {
            IOPMAssertionCreateWithName(
                kind.as_concrete_TypeRef(),
                ASSERTION_LEVEL_ON,
                name.as_concrete_TypeRef(),
                &mut id,
            )
        };
        if rc == 0 { Ok(id) } else { Err(rc) }
    }

    pub fn release(id: u32) {
        let rc = unsafe { IOPMAssertionRelease(id) };
        if rc != 0 {
            tracing::warn!(rc, "IOPMAssertionRelease failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconcile_without_marker_does_nothing() {
        // We never engaged the override, so a SleepDisabled set by something
        // else must be left alone, whatever its value (or readability).
        assert_eq!(
            reconcile_decision(false, Some(false)),
            ReconcileAction::Nothing
        );
        assert_eq!(
            reconcile_decision(false, Some(true)),
            ReconcileAction::Nothing
        );
        assert_eq!(reconcile_decision(false, None), ReconcileAction::Nothing);
    }

    #[test]
    fn reconcile_keeps_marker_when_state_unknown() {
        // An unreadable sleep state must not drop a marker that may guard a real
        // override — keep it and retry on a later launch.
        assert_eq!(reconcile_decision(true, None), ReconcileAction::Keep);
    }

    #[test]
    fn reconcile_stale_marker_is_dropped() {
        // Our marker survived but the override is already gone (e.g. a reboot).
        assert_eq!(
            reconcile_decision(true, Some(false)),
            ReconcileAction::RemoveMarker
        );
    }

    #[test]
    fn reconcile_leftover_override_is_cleared() {
        // Unclean exit: our marker and the override both remain.
        assert_eq!(
            reconcile_decision(true, Some(true)),
            ReconcileAction::ClearOverride
        );
    }
}
