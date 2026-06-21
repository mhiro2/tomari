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
//! The lid-close veto is **required**, not optional, and both directions go
//! through it on a worker thread — which (not the toggle) commits `active`. An
//! engage takes the idle assertion immediately and shows on; if the veto then
//! cannot be engaged (auth declined, or an unreadable sleep state) the whole
//! switch rolls back off. A disengage defers turning off to the worker: clearing
//! the override needs an admin dialog that can be declined, and sleep is still
//! prevented until it succeeds, so a declined clear keeps keep-awake on. A
//! `generation` counter bumped on every toggle lets a slow worker tell that a
//! newer toggle superseded it mid-dialog, so a stale cancel never clobbers a
//! switch the user re-toggled.
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

use crate::locks::MutexExt;
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
    /// Bumped on every desired-state transition (engage / disengage / shutdown).
    /// A background reconcile captures it before its slow admin-auth dialog and
    /// re-checks it on writeback: if it changed, a newer toggle superseded this
    /// cycle, so the worker must not clobber the newer cycle's `active` or
    /// assertion (e.g. roll back an "on" the user has since re-enabled).
    generation: u64,
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
    /// The veto could not be engaged (authorization declined, or the sleep state
    /// was unreadable). An internal signal only: the reconcile worker turns this
    /// into a full roll-back (keep-awake off), so it is never a resting UI state.
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
    let k = state.keep_awake.lock_safe();
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
    let active = app.state::<AppState>().keep_awake.lock_safe().active;
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
    // Whether the idle-sleep assertion was actually taken. Only then do we have
    // a live "on" worth reconciling the lid-close veto for.
    #[cfg(target_os = "macos")]
    let engaged;
    {
        let mut k = state.keep_awake.lock_safe();
        if k.active {
            // Already on — nothing to do. This is safe against an in-flight
            // turn-off (whose worker is driving the override toward cleared) only
            // because every entry point (panel switch, tray, hotkey, deep link)
            // derives its target from the committed `active`: during an off-pending
            // `active` is still `true`, so they all request "off", never "on".
            // Cancelling the auth dialog is the "stay on" affordance. A future
            // *explicit* set-on path would instead need to supersede a pending
            // turn-off here (bump the generation and re-drive the veto on).
            return KeepAwakeStatus {
                active: true,
                lid_close: k.lid_close,
            };
        }
        // Re-check shutdown *under the lock*. `cleanup_blocking` sets
        // SHUTTING_DOWN before it takes this lock, so an engage that raced it
        // past the unlocked check above would otherwise still slip through here,
        // create an assertion, and spawn a worker that re-engages the lid-close
        // override after cleanup already cleared it — stranding the Mac awake
        // past exit. Bailing here closes that window.
        if SHUTTING_DOWN.load(std::sync::atomic::Ordering::Acquire) {
            return KeepAwakeStatus {
                active: false,
                lid_close: k.lid_close,
            };
        }
        // The idle-sleep assertion is the foundation of keep-awake — it needs no
        // permission and is what actually holds sleep off for the lid-open case.
        // Take it synchronously and flip `active` on *only once it succeeds*, so a
        // rare IOKit failure can never leave the switch reported "on" with nothing
        // behind it. The admin-authed lid-close veto is reconciled in the
        // background below; it is a required part of keep-awake, so declining its
        // prompt rolls the whole switch back off (see `reconcile_lid_close`).
        #[cfg(target_os = "macos")]
        {
            match create_assertion() {
                Ok(id) => {
                    k.assertion = Some(id);
                    k.active = true;
                    k.lid_close = LidCloseState::Pending;
                    k.generation = k.generation.wrapping_add(1);
                    engaged = true;
                }
                Err(rc) => {
                    tracing::warn!(
                        rc,
                        "failed to create power assertion; keep-awake not engaged"
                    );
                    engaged = false;
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            k.active = true;
            k.lid_close = LidCloseState::Off;
        }
    }
    // Re-sync every surface — including a tray checkmark a click just toggled on,
    // which a failed engage must reset. On success, engage the lid-close veto in
    // the background so its admin-auth dialog never blocks the caller.
    notify(app);
    #[cfg(target_os = "macos")]
    if engaged {
        spawn_reconcile(app.clone(), true);
    }
    status(state.inner())
}

fn disengage(app: &AppHandle) -> KeepAwakeStatus {
    let state = app.state::<AppState>();
    {
        let mut k = state.keep_awake.lock_safe();
        if !k.active {
            return KeepAwakeStatus {
                active: false,
                lid_close: k.lid_close,
            };
        }
        #[cfg(target_os = "macos")]
        {
            // Do *not* flip `active` or release the assertion yet. Turning off
            // means clearing the lid-close override, which needs an admin dialog
            // that can be declined — and until it is cleared, sleep is still
            // prevented. The worker commits the off (`active = false`, assertion
            // released) only once the clear succeeds; a declined clear keeps
            // keep-awake on rather than show it off while sleep stays blocked.
            // Bump the generation so an in-flight engage worker is superseded.
            k.generation = k.generation.wrapping_add(1);
        }
        #[cfg(not(target_os = "macos"))]
        {
            // No lid-close veto off-platform, so the off is immediate.
            k.active = false;
            k.lid_close = LidCloseState::Off;
        }
    }
    #[cfg(target_os = "macos")]
    {
        // The off is reflected by the worker once the override is cleared, so
        // there is nothing to notify here yet.
        spawn_reconcile(app.clone(), false);
    }
    #[cfg(not(target_os = "macos"))]
    notify(app);
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

/// The side effects the lid-close state machine drives, factored behind a trait
/// so [`reconcile_lid_close_with`] and [`cleanup_lid_close_with`] can be unit
/// tested against a fake — exercising declined auth, an unwritable marker, and a
/// foreign override without a real `pmset`, marker file, or admin prompt.
trait LidCloseSys {
    /// Read the kernel `SleepDisabled` flag (`None` = could not be read).
    fn read_sleep_disabled(&self) -> Option<bool>;
    /// Set or clear `pmset disablesleep`; returns whether it was applied.
    fn set_disablesleep(&self, on: bool) -> bool;
    /// Write the failsafe marker; returns whether it is durably on disk.
    fn write_marker(&self) -> bool;
    /// Remove the failsafe marker (best effort).
    fn remove_marker(&self);
    /// Whether the failsafe marker is present.
    fn marker_exists(&self) -> bool;
}

/// The fields the pure lid-close reconcile decides. Deliberately excludes
/// `active`: this step resolves only the veto's state and ownership, keeping the
/// slow `pmset`/auth work free of the toggle's flag. The worker then maps the
/// outcome onto `active` via [`reconcile_writeback`] — an `Unavailable` veto for
/// a still-wanted "on" rolls the whole switch back off.
#[derive(Debug, PartialEq, Eq)]
struct LidCloseOutcome {
    lid_close: LidCloseState,
    we_own: bool,
}

/// Drive the lid-close veto toward the desired `active` state, returning the
/// resulting lid-close status and ownership. Free of state mutation beyond the
/// `sys` calls (and a diagnostic log), so it is exercised end-to-end in tests
/// against a fake.
fn reconcile_lid_close_with<S: LidCloseSys>(
    sys: &S,
    active: bool,
    we_own: bool,
) -> LidCloseOutcome {
    if !active {
        // Toggled off: clear only an override we engaged ourselves, and only
        // drop the marker once the clear actually succeeded. If the clear fails
        // (auth declined on the way down) keep ownership and the marker so the
        // next launch's reconcile / cleanup retries rather than leaking it.
        let we_own = if we_own && sys.set_disablesleep(false) {
            sys.remove_marker();
            false
        } else {
            we_own
        };
        return LidCloseOutcome {
            lid_close: LidCloseState::Off,
            we_own,
        };
    }
    // If we already own the override it is on by definition; otherwise read it.
    let sleep_disabled = if we_own {
        Some(true)
    } else {
        sys.read_sleep_disabled()
    };
    match sleep_disabled {
        // Already vetoed — by us, or by the user/another process. The lid-close
        // guarantee holds; never take ownership of a value we did not set, so we
        // never clear someone else's override later.
        Some(true) => LidCloseOutcome {
            lid_close: LidCloseState::Engaged,
            we_own,
        },
        // Safe to enable. Persist the marker *before* enabling so a crash in
        // between leaves a record the next launch reconciles. If the marker
        // cannot be written, don't enable — recovery couldn't be guaranteed. If
        // the (admin-authed) `pmset` is then declined, enable nothing, drop the
        // marker, and surface `Unavailable`. This pure step never touches
        // `active`; the worker turns `Unavailable`-for-a-wanted-on into a full
        // roll-back, since the veto is a required part of keep-awake.
        Some(false) => {
            if sys.write_marker() && sys.set_disablesleep(true) {
                LidCloseOutcome {
                    lid_close: LidCloseState::Engaged,
                    we_own: true,
                }
            } else {
                sys.remove_marker();
                LidCloseOutcome {
                    lid_close: LidCloseState::Unavailable,
                    we_own: false,
                }
            }
        }
        // Unreadable: don't clobber an override we can't see, and don't claim a
        // guarantee we can't make.
        None => {
            tracing::warn!("could not read sleep state; not engaging lid-close veto");
            LidCloseOutcome {
                lid_close: LidCloseState::Unavailable,
                we_own,
            }
        }
    }
}

/// The committed keep-awake state a reconcile worker resolves to. A toggle that
/// flips the desired state while the worker's admin-auth dialog is up bumps the
/// generation; a superseded worker must then leave `active` and the assertion to
/// the newer cycle's own worker.
#[derive(Debug, PartialEq, Eq)]
enum ReconcileWriteback {
    /// A newer toggle superseded this cycle: record ownership only, touch nothing
    /// else (the newer cycle's worker sets the final state).
    Superseded,
    /// Keep-awake on: idle assertion held, veto engaged. Either a successful
    /// engage, or a turn-off whose override clear was declined (sleep is still
    /// prevented, so we keep it on rather than report it off).
    On,
    /// Keep-awake off: release the assertion. Either a successful turn-off, or an
    /// engage whose mandatory veto could not be engaged (declined / unreadable),
    /// which rolls the whole switch back off.
    Off,
}

/// Decide the committed keep-awake state from a reconcile, given whether the
/// worker was superseded (its captured generation no longer matches), the
/// direction it reconciled toward (`desired_on`), and the resulting lid-close
/// state and ownership. Pure, so the roll-back / stay-on / supersession policy is
/// unit-tested.
fn reconcile_writeback(
    superseded: bool,
    desired_on: bool,
    lid_close: LidCloseState,
    we_own: bool,
) -> ReconcileWriteback {
    if superseded {
        ReconcileWriteback::Superseded
    } else if desired_on {
        // Turning on: the veto is mandatory, so commit on only if it engaged;
        // otherwise (declined / unreadable) roll the whole switch back off.
        if lid_close == LidCloseState::Engaged {
            ReconcileWriteback::On
        } else {
            ReconcileWriteback::Off
        }
    } else if we_own {
        // Turning off but the override clear was declined: we still own it, so
        // sleep is still prevented — keep keep-awake on instead of lying it is off.
        ReconcileWriteback::On
    } else {
        // Turning off and nothing is left we own (cleared, or never ours): off.
        ReconcileWriteback::Off
    }
}

/// Decide the exit-time cleanup, returning the resulting ownership. Clears the
/// override when we own it, or when a leftover marker says we might (an engage
/// that crashed after writing the marker but before recording ownership). A
/// `disablesleep` we never touched — no ownership, no marker — is left alone, so
/// quit / logout / updater restart never clears a foreign override.
fn cleanup_lid_close_with<S: LidCloseSys>(sys: &S, we_own: bool) -> bool {
    if (we_own || sys.marker_exists()) && sys.set_disablesleep(false) {
        sys.remove_marker();
        false
    } else {
        we_own
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
        let mut k = state.keep_awake.lock_safe();
        k.active = false;
        #[cfg(target_os = "macos")]
        {
            if let Some(id) = k.assertion.take() {
                release_assertion(id);
            }
            // Supersede any in-flight engage worker so its writeback cannot
            // re-assert `active` after we have begun tearing everything down.
            k.generation = k.generation.wrapping_add(1);
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
        let _op = LID_OP_LOCK.lock_safe();
        let we_own = state.keep_awake.lock_safe().we_own_override;
        let still_own = cleanup_lid_close_with(&RealSys, we_own);
        if still_own != we_own {
            state.keep_awake.lock_safe().we_own_override = still_own;
        }
    }
}

// ---------------------------------------------------------------------------
// macOS implementation
// ---------------------------------------------------------------------------

/// Reconcile the lid-close veto on a worker thread, since the admin-auth dialog
/// blocks. `desired_on` is the direction the toggle asked for (engage / disengage).
#[cfg(target_os = "macos")]
fn spawn_reconcile(app: AppHandle, desired_on: bool) {
    std::thread::spawn(move || reconcile_lid_close(&app, desired_on));
}

/// Drive `pmset disablesleep` toward `desired_on` and commit the resulting
/// keep-awake state. Serialized by `LID_OP_LOCK`, so a burst of toggles settles
/// on the last one.
///
/// The lid-close veto is a required part of keep-awake and both directions go
/// through it, so the worker — not the toggle — commits `active`:
/// * Turning **on**: if the veto cannot be engaged (auth declined, or sleep state
///   unreadable) the whole switch rolls back off.
/// * Turning **off**: the off takes effect only once the override is actually
///   cleared; a declined clear leaves keep-awake on (sleep is still prevented).
///
/// A toggle that superseded this cycle while the admin-auth dialog was up is
/// detected via the generation and left to its own worker, so a stale cancel
/// never clobbers a freshly re-toggled switch.
#[cfg(target_os = "macos")]
fn reconcile_lid_close(app: &AppHandle, desired_on: bool) {
    let _op = LID_OP_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let state = app.state::<AppState>();
    let (we_own, generation) = {
        let k = state.keep_awake.lock_safe();
        (k.we_own_override, k.generation)
    };
    // Run the (possibly slow, admin-authed) side effects without holding the
    // state lock, then store the resulting status back in one shot.
    let outcome = reconcile_lid_close_with(&RealSys, desired_on, we_own);
    {
        let mut k = state.keep_awake.lock_safe();
        // Always record ownership — even when superseded — so a later worker or
        // cleanup can clear an override this cycle set.
        k.we_own_override = outcome.we_own;
        match reconcile_writeback(
            k.generation != generation,
            desired_on,
            outcome.lid_close,
            outcome.we_own,
        ) {
            ReconcileWriteback::Superseded => return,
            ReconcileWriteback::On => {
                // Keep-awake on: the idle assertion must be held (it is — engage
                // took it, and a deferred-off cycle never released it), and the
                // veto shows engaged. Re-acquire defensively if it is somehow gone.
                if k.assertion.is_none() {
                    match create_assertion() {
                        Ok(id) => k.assertion = Some(id),
                        Err(rc) => tracing::warn!(rc, "failed to re-create power assertion"),
                    }
                }
                k.active = true;
                k.lid_close = LidCloseState::Engaged;
            }
            ReconcileWriteback::Off => {
                if let Some(id) = k.assertion.take() {
                    release_assertion(id);
                }
                k.active = false;
                k.lid_close = LidCloseState::Off;
            }
        }
    }
    notify(app);
}

/// Production [`LidCloseSys`]: the real `pmset` calls and on-disk marker.
#[cfg(target_os = "macos")]
struct RealSys;

#[cfg(target_os = "macos")]
impl LidCloseSys for RealSys {
    fn read_sleep_disabled(&self) -> Option<bool> {
        read_sleep_disabled()
    }
    fn set_disablesleep(&self, on: bool) -> bool {
        run_disablesleep(on)
    }
    fn write_marker(&self) -> bool {
        write_marker()
    }
    fn remove_marker(&self) {
        remove_marker()
    }
    fn marker_exists(&self) -> bool {
        marker_exists()
    }
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

    #[test]
    fn writeback_superseded_changes_nothing() {
        // A toggle bumped the generation while our admin-auth dialog was up:
        // commit nothing, whatever the direction/outcome — the newer cycle's
        // worker owns the final state, so a stale cancel can't clobber it.
        assert_eq!(
            reconcile_writeback(true, true, LidCloseState::Unavailable, false),
            ReconcileWriteback::Superseded
        );
        assert_eq!(
            reconcile_writeback(true, false, LidCloseState::Off, true),
            ReconcileWriteback::Superseded
        );
    }

    #[test]
    fn writeback_turn_on_requires_the_veto() {
        // Turning on commits only if the veto engaged; a declined / unreadable
        // veto rolls the whole (mandatory-veto) switch back off.
        assert_eq!(
            reconcile_writeback(false, true, LidCloseState::Engaged, true),
            ReconcileWriteback::On
        );
        assert_eq!(
            reconcile_writeback(false, true, LidCloseState::Unavailable, false),
            ReconcileWriteback::Off
        );
    }

    #[test]
    fn writeback_turn_off_stays_on_when_clear_declined() {
        // Turning off but the override clear was declined: we still own it, so
        // sleep is still prevented — keep keep-awake on rather than report off.
        assert_eq!(
            reconcile_writeback(false, false, LidCloseState::Off, true),
            ReconcileWriteback::On
        );
    }

    #[test]
    fn writeback_turn_off_commits_when_cleared() {
        // Turning off and nothing left we own (cleared, or never ours): commit off.
        assert_eq!(
            reconcile_writeback(false, false, LidCloseState::Off, false),
            ReconcileWriteback::Off
        );
    }

    /// A side-effecting call recorded by [`FakeSys`], in invocation order, so
    /// tests can assert not just *which* effects ran but their *ordering* — most
    /// importantly that the marker is written before the override is enabled.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Op {
        WriteMarker,
        RemoveMarker,
        SetDisablesleep(bool),
    }

    /// In-memory [`LidCloseSys`] for failure-mode tests: configurable `pmset` and
    /// marker-write behavior over an observable marker and an ordered op log, so
    /// the lid-close state machine runs end-to-end without touching the system,
    /// an admin prompt, or the filesystem.
    struct FakeSys {
        /// What `read_sleep_disabled` returns — the system's view before we act.
        sleep_disabled: std::cell::Cell<Option<bool>>,
        /// Whether `write_marker` succeeds (false simulates an unwritable dir).
        marker_write_ok: bool,
        /// Whether `set_disablesleep` succeeds (false simulates a declined auth
        /// prompt or a failed `pmset`).
        pmset_ok: bool,
        /// Whether the failsafe marker is currently on disk.
        marker: std::cell::Cell<bool>,
        /// Every side-effecting call, in order.
        ops: std::cell::RefCell<Vec<Op>>,
    }

    impl FakeSys {
        /// A system whose `SleepDisabled` flag currently reads as `sleep_disabled`.
        fn new(sleep_disabled: Option<bool>) -> Self {
            Self {
                sleep_disabled: std::cell::Cell::new(sleep_disabled),
                marker_write_ok: true,
                pmset_ok: true,
                marker: std::cell::Cell::new(false),
                ops: std::cell::RefCell::new(Vec::new()),
            }
        }
        /// Simulate a directory the marker cannot be written to.
        fn marker_write_fails(mut self) -> Self {
            self.marker_write_ok = false;
            self
        }
        /// Simulate a declined admin prompt / failed `pmset`.
        fn pmset_fails(mut self) -> Self {
            self.pmset_ok = false;
            self
        }
        /// Start with the failsafe marker already on disk.
        fn with_marker(self) -> Self {
            self.marker.set(true);
            self
        }
        /// The recorded side effects, in order.
        fn ops(&self) -> Vec<Op> {
            self.ops.borrow().clone()
        }
        /// The `set_disablesleep` arguments, in order, for asserting on which
        /// `pmset` calls actually ran.
        fn pmset_calls(&self) -> Vec<bool> {
            self.ops
                .borrow()
                .iter()
                .filter_map(|op| match op {
                    Op::SetDisablesleep(on) => Some(*on),
                    _ => None,
                })
                .collect()
        }
        fn marker_present(&self) -> bool {
            self.marker.get()
        }
    }

    impl LidCloseSys for FakeSys {
        fn read_sleep_disabled(&self) -> Option<bool> {
            self.sleep_disabled.get()
        }
        fn set_disablesleep(&self, on: bool) -> bool {
            self.ops.borrow_mut().push(Op::SetDisablesleep(on));
            if self.pmset_ok {
                self.sleep_disabled.set(Some(on));
                true
            } else {
                false
            }
        }
        fn write_marker(&self) -> bool {
            self.ops.borrow_mut().push(Op::WriteMarker);
            if self.marker_write_ok {
                self.marker.set(true);
                true
            } else {
                false
            }
        }
        fn remove_marker(&self) {
            self.ops.borrow_mut().push(Op::RemoveMarker);
            self.marker.set(false);
        }
        fn marker_exists(&self) -> bool {
            self.marker.get()
        }
    }

    #[test]
    fn engage_from_clean_state_takes_ownership_and_marks() {
        // Nothing was vetoing sleep; we engage, own the override, and the marker
        // must guard it (written before the `pmset`).
        let sys = FakeSys::new(Some(false));
        let out = reconcile_lid_close_with(&sys, true, false);
        assert_eq!(
            out,
            LidCloseOutcome {
                lid_close: LidCloseState::Engaged,
                we_own: true,
            }
        );
        assert!(
            sys.marker_present(),
            "a live override must be guarded by a marker"
        );
        // The write-ahead invariant: the marker must be persisted *before* the
        // override is enabled, so a crash in between always leaves a record the
        // next launch can reconcile. Assert the ordering, not just the calls.
        assert_eq!(
            sys.ops(),
            vec![Op::WriteMarker, Op::SetDisablesleep(true)],
            "the marker must be written before pmset enables the override"
        );
    }

    #[test]
    fn declined_auth_is_unavailable_and_leaves_no_marker() {
        // The admin prompt was cancelled. lid-close is Unavailable and we own
        // nothing. This pure step leaves `active` alone (the worker owns the
        // roll-back: it turns Unavailable-for-a-wanted-on into a full switch-off),
        // so the assertion outcome here is just `Unavailable`. No marker may linger.
        let sys = FakeSys::new(Some(false)).pmset_fails();
        let out = reconcile_lid_close_with(&sys, true, false);
        assert_eq!(
            out,
            LidCloseOutcome {
                lid_close: LidCloseState::Unavailable,
                we_own: false,
            }
        );
        assert!(
            !sys.marker_present(),
            "a failed enable must not leave a marker behind"
        );
        assert_eq!(sys.pmset_calls(), vec![true]);
    }

    #[test]
    fn marker_write_failure_skips_pmset() {
        // If the failsafe marker can't be persisted we must not enable the
        // override — there would be no record to recover from after a crash.
        let sys = FakeSys::new(Some(false)).marker_write_fails();
        let out = reconcile_lid_close_with(&sys, true, false);
        assert_eq!(
            out,
            LidCloseOutcome {
                lid_close: LidCloseState::Unavailable,
                we_own: false,
            }
        );
        assert!(
            sys.pmset_calls().is_empty(),
            "pmset must not run without a marker"
        );
        assert!(!sys.marker_present());
    }

    #[test]
    fn preexisting_override_is_used_but_not_owned() {
        // `disablesleep` was already set by the user or another process. We rely
        // on it for the lid-close guarantee but never take ownership, so we will
        // not clear it later.
        let sys = FakeSys::new(Some(true));
        let out = reconcile_lid_close_with(&sys, true, false);
        assert_eq!(
            out,
            LidCloseOutcome {
                lid_close: LidCloseState::Engaged,
                we_own: false,
            }
        );
        assert!(
            sys.pmset_calls().is_empty(),
            "must not re-issue pmset for an override we don't own"
        );
        assert!(
            !sys.marker_present(),
            "must not mark an override we don't own"
        );
    }

    #[test]
    fn unreadable_state_on_enable_is_unavailable() {
        // Can't read SleepDisabled: don't clobber a possibly-foreign override and
        // don't claim the lid-close guarantee.
        let sys = FakeSys::new(None);
        let out = reconcile_lid_close_with(&sys, true, false);
        assert_eq!(
            out,
            LidCloseOutcome {
                lid_close: LidCloseState::Unavailable,
                we_own: false,
            }
        );
        assert!(sys.pmset_calls().is_empty());
    }

    #[test]
    fn disable_clears_an_override_we_own() {
        // Normal toggle off: clear our override and drop the marker.
        let sys = FakeSys::new(Some(true)).with_marker();
        let out = reconcile_lid_close_with(&sys, false, true);
        assert_eq!(
            out,
            LidCloseOutcome {
                lid_close: LidCloseState::Off,
                we_own: false,
            }
        );
        assert_eq!(sys.pmset_calls(), vec![false]);
        assert!(!sys.marker_present());
    }

    #[test]
    fn disable_leaves_a_foreign_override_untouched() {
        // We turned our idle assertion off, but the lingering `disablesleep`
        // isn't ours — never touch it.
        let sys = FakeSys::new(Some(true));
        let out = reconcile_lid_close_with(&sys, false, false);
        assert_eq!(
            out,
            LidCloseOutcome {
                lid_close: LidCloseState::Off,
                we_own: false,
            }
        );
        assert!(sys.pmset_calls().is_empty());
    }

    #[test]
    fn disable_keeps_ownership_when_clear_fails() {
        // Auth declined on the way down: stay owner and keep the marker so launch
        // reconcile / cleanup retries rather than leaking the override.
        let sys = FakeSys::new(Some(true)).with_marker().pmset_fails();
        let out = reconcile_lid_close_with(&sys, false, true);
        assert_eq!(
            out,
            LidCloseOutcome {
                lid_close: LidCloseState::Off,
                we_own: true,
            }
        );
        assert_eq!(sys.pmset_calls(), vec![false]);
        assert!(
            sys.marker_present(),
            "keep the marker so recovery can retry"
        );
    }

    #[test]
    fn cleanup_clears_an_override_we_own() {
        // Clean exit (tray quit / logout / normal close / updater restart): the
        // override we engaged must be cleared so it never outlives Tomari.
        let sys = FakeSys::new(Some(true)).with_marker();
        let still_own = cleanup_lid_close_with(&sys, true);
        assert!(!still_own);
        assert_eq!(sys.pmset_calls(), vec![false]);
        assert!(!sys.marker_present());
    }

    #[test]
    fn cleanup_clears_via_failsafe_marker_even_without_ownership() {
        // An engage that crashed after writing the marker but before recording
        // ownership: the marker alone triggers a failsafe clear at exit.
        let sys = FakeSys::new(Some(true)).with_marker();
        let still_own = cleanup_lid_close_with(&sys, false);
        assert!(!still_own);
        assert_eq!(sys.pmset_calls(), vec![false]);
        assert!(!sys.marker_present());
    }

    #[test]
    fn cleanup_leaves_a_foreign_override_untouched() {
        // `disablesleep` is on but isn't ours and there's no marker: never clear
        // an override Tomari didn't set, even on quit / logout.
        let sys = FakeSys::new(Some(true));
        let still_own = cleanup_lid_close_with(&sys, false);
        assert!(!still_own);
        assert!(
            sys.pmset_calls().is_empty(),
            "must not clear a foreign override at exit"
        );
    }

    #[test]
    fn cleanup_keeps_ownership_when_clear_fails() {
        // Auth declined during shutdown: stay owner and keep the marker so the
        // next launch's reconcile clears the leftover override.
        let sys = FakeSys::new(Some(true)).with_marker().pmset_fails();
        let still_own = cleanup_lid_close_with(&sys, true);
        assert!(still_own, "ownership must survive a failed cleanup");
        assert!(
            sys.marker_present(),
            "the marker must survive a failed cleanup"
        );
    }

    #[test]
    fn cleanup_keeps_marker_when_failsafe_clear_fails() {
        // A crash-leftover marker triggers a failsafe clear even without recorded
        // ownership, but the clear itself is declined: the marker must survive so
        // the next launch's reconcile retries rather than leaking the override.
        let sys = FakeSys::new(Some(true)).with_marker().pmset_fails();
        let still_own = cleanup_lid_close_with(&sys, false);
        assert!(!still_own);
        assert_eq!(
            sys.pmset_calls(),
            vec![false],
            "the failsafe clear must run"
        );
        assert!(
            sys.marker_present(),
            "the marker must survive a failed failsafe clear"
        );
    }
}
