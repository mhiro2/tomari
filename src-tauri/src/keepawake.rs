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
//! prevented until it succeeds, so a declined clear keeps keep-awake on.
//! While either direction is pending, ordinary toggles are rejected. An explicit
//! cancel terminates the active authorization process and queues the reverse
//! transition. A `generation` counter lets a slow worker recognize that cancel
//! and prevents its stale result from clobbering the reverse transition — and
//! makes a worker that has not started yet skip its side effects entirely.
//!
//! Three invariants keep the lid-close override from ever stranding the Mac in
//! a never-sleep state:
//!
//! * **Write-ahead marker.** A marker file under the data directory is written
//!   *before* `disablesleep` is enabled and removed only once the clear is
//!   *confirmed*, so a crash at any point leaves a record [`reconcile_on_launch`]
//!   can act on. An unreadable sleep state is treated as "unknown" — the marker
//!   is kept, never dropped.
//! * **Verified against the kernel flag.** Every `pmset` is checked against the
//!   actual `SleepDisabled` value afterwards rather than trusting its reported
//!   exit status: `pmset` can apply the change while `osascript` still reports
//!   failure (or, in principle, the reverse), so the marker and ownership track
//!   the *confirmed* kernel state — never a setter result that did not take.
//! * **Ownership.** We only ever clear a `disablesleep` we turned on ourselves;
//!   a value already set by the user or another process is left untouched.
//!   Ownership is three-valued ([`Ownership`]): *confirmed* once the kernel flag
//!   has been read back set after our enable, *possibly* ours when our enable
//!   ran but the flag could not be read back and a compensating clear could not
//!   be confirmed either, and *unowned* otherwise. A possibly-owned override is
//!   never written off as foreign: the next reconcile that reads it set confirms
//!   it as ours, off clears it, and the UI shows the unresolved state until then.
//!
//! The slow, admin-authed `pmset` calls are serialized through `LID_OP_LOCK`
//! and always drive the system toward the current desired state. This also
//! serializes the cleanup that follows an authorization cancellation.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::locks::MutexExt;
use crate::state::AppState;

/// Event emitted whenever the keep-awake state changes, so every surface (the
/// panel toggle, the tray checkmark) can stay in sync regardless of which one
/// initiated the change. Matches the `tomari:` event convention used elsewhere.
const CHANGED_EVENT: &str = "tomari:keep-awake-changed";

/// Serializes the slow, admin-authed `pmset` operations. Under this lock each
/// worker drives the lid-close override toward the current desired state, and a
/// cancellation can queue its reverse without racing the original operation.
#[cfg(target_os = "macos")]
static LID_OP_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Set once [`cleanup_blocking`] begins (the process is exiting). It makes
/// [`engage`] refuse, so a toggle that races the shutdown cannot spawn a worker
/// that re-enables the override after cleanup has already cleared it.
static SHUTTING_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// PID of the `osascript` process currently presenting administrator
/// authorization. Only one can run because `LID_OP_LOCK` serializes them.
#[cfg(target_os = "macos")]
static AUTH_PID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

const LOW_BATTERY_PERCENT: u8 = 20;
const LONG_RUNNING_PROCESS_SECS: u64 = 5 * 60;

/// Runtime keep-awake state. Not persisted — always starts inactive.
#[derive(Debug)]
pub struct KeepAwake {
    /// Sleep prevention is on (an idle-sleep assertion is held).
    active: bool,
    /// The held IOKit power-assertion id, if any.
    assertion: Option<u32>,
    /// Lid-close veto status, surfaced to the UI.
    lid_close: LidCloseState,
    /// Whether *we* turned `disablesleep` on (vs. it being set before us). We
    /// only ever clear an override we engaged ourselves — or might have.
    ownership: Ownership,
    /// Bumped on every desired-state transition (engage / disengage / shutdown).
    /// Each reconcile worker captures the value *at spawn time* (under the same
    /// lock that bumped it) and carries it through its slow admin-auth dialog; on
    /// writeback it compares that captured value against the current one. If they
    /// differ, a newer transition superseded this cycle, so the worker must not
    /// clobber the newer cycle's `active` or assertion. Capturing at spawn —
    /// rather than after acquiring `LID_OP_LOCK` — is what lets an explicit
    /// cancellation queue a safe reverse transition behind the canceled worker.
    generation: u64,
    /// User-visible transition state. Unlike `active`, this distinguishes an
    /// administrator prompt from a settled system state.
    phase: KeepAwakePhase,
    /// Session-only safety policy selected in the panel.
    options: KeepAwakeOptions,
    /// Last safety or authorization outcome that needs the user's attention.
    notice: Option<KeepAwakeNotice>,
    /// Target of the last failed transition, used by the explicit retry action.
    retry_target: Option<bool>,
    /// A launch found the previous run's failsafe marker with the lid-close
    /// override still set (or unreadable) and the user has not yet said whether
    /// it is Tomari's to clear. While set, the override is not cleared by exit,
    /// and the ordinary on/off paths (panel, tray, hotkey) are refused: any of
    /// them would end in a clear justified by the marker alone. Cleared by the
    /// two decisions — Retry (clear it) or dismiss (leave it, drop the marker).
    leftover_undecided: bool,
    /// Whether a safety guard has already driven an automatic turn-off for this
    /// session, so each session gets one automatic attempt: the clear needs an
    /// administrator prompt that can be declined, and re-deciding every tick
    /// would reopen that dialog on a loop. Cleared *only* by an explicit request
    /// in [`set`] — notably not by settling back on, which is where cancelling
    /// an automatic turn-off lands with its trigger usually still true.
    auto_off_attempted: bool,
    /// Cached real power and kernel state, refreshed by the safety monitor.
    power_source: PowerSource,
    battery_percent: Option<u8>,
    kernel_sleep_disabled: Option<bool>,
    /// Long-running developer jobs detected by the safety monitor.
    long_running_processes: Vec<LongRunningProcess>,
    /// Monotonic stamp taken by every snapshot the frontend can receive — both
    /// [`status`] reads and [`emit_status`] events — under the same lock that
    /// produced it. It counts snapshots issued, not state changes: what matters
    /// is that a snapshot issued later always outranks one issued earlier.
    /// Several threads emit (commands, reconcile workers, the safety monitor)
    /// and each snapshots before it emits, so the events can arrive out of
    /// order; the frontend drops a snapshot older than one it already applied
    /// instead of being stranded on a transition that has since finished.
    revision: u64,
}

impl Default for KeepAwake {
    fn default() -> Self {
        Self {
            active: false,
            assertion: None,
            lid_close: LidCloseState::Off,
            ownership: Ownership::Unowned,
            generation: 0,
            phase: KeepAwakePhase::Off,
            options: KeepAwakeOptions::default(),
            notice: None,
            retry_target: None,
            leftover_undecided: false,
            auto_off_attempted: false,
            power_source: PowerSource::Unknown,
            battery_percent: None,
            kernel_sleep_disabled: None,
            long_running_processes: Vec::new(),
            revision: 0,
        }
    }
}

/// Explicit state machine for the complete Keep Awake operation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum KeepAwakePhase {
    #[default]
    Off,
    Enabling,
    On,
    Disabling,
    Failed,
}

impl KeepAwakePhase {
    pub fn is_pending(self) -> bool {
        matches!(self, Self::Enabling | Self::Disabling)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LowBatteryAction {
    #[default]
    Warn,
    TurnOff,
}

/// Session-only safety controls. `ends_at_ms` is an absolute Unix timestamp so
/// the backend, tray, and panel agree even if the panel is closed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeepAwakeOptions {
    /// Relative timer preset. Re-armed on every transition to on.
    pub duration_secs: Option<u64>,
    /// Materialized deadline, or a user-selected absolute end time.
    pub ends_at_ms: Option<u64>,
    pub ac_only: bool,
    pub low_battery_action: LowBatteryAction,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PowerSource {
    Ac,
    Battery,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum KeepAwakeNotice {
    AcRequired,
    AcDisconnected,
    LowBattery,
    TimerElapsed,
    AuthorizationDeclined,
    /// The lid-close override was enabled but its state could not be read back,
    /// and clearing it again could not be confirmed either: the Mac may be unable
    /// to sleep although keep-awake shows off. Retry clears it.
    LidCloseUnconfirmed,
    /// At launch, the previous run's failsafe marker was found with the
    /// lid-close override still set. The marker says a previous run *may* have
    /// left it; the kernel flag records no provenance, so it may equally be the
    /// user's or another tool's since. Nothing is cleared on that evidence
    /// alone: the user decides — Retry clears it, dismissing keeps it and
    /// forgets the marker.
    LeftoverOverride,
}

/// Who set the live `disablesleep` override, as far as Tomari can tell. The
/// kernel flag records no provenance, so this is tracked from what we did and
/// what we then read back.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Ownership {
    /// Not ours: never enabled by us, or confirmed cleared since.
    #[default]
    Unowned,
    /// Our enable ran, but neither it nor a compensating clear could be
    /// confirmed against the kernel flag. Treated as ours for every purpose
    /// that matters — it is cleared on off and at exit, and a later read that
    /// finds it set confirms it — and reported to the UI as unresolved.
    PossiblyOwned,
    /// We enabled it and read it back set.
    Confirmed,
}

impl Ownership {
    /// Whether the override is ours to clear — certainly or possibly.
    fn may_own(self) -> bool {
        self != Self::Unowned
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LongRunningProcess {
    pub pid: u32,
    pub name: String,
    pub elapsed_secs: u64,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeepAwakeStatus {
    /// Sleep prevention is on.
    pub active: bool,
    /// Lid-close veto state.
    pub lid_close: LidCloseState,
    pub phase: KeepAwakePhase,
    pub options: KeepAwakeOptions,
    pub notice: Option<KeepAwakeNotice>,
    pub power_source: PowerSource,
    pub battery_percent: Option<u8>,
    pub kernel_sleep_disabled: Option<bool>,
    pub owns_lid_close: bool,
    /// See `KeepAwake::leftover_undecided`: a decision about a leftover override
    /// is pending, and only the two recovery actions are accepted.
    pub leftover_undecided: bool,
    pub long_running_processes: Vec<LongRunningProcess>,
    /// See [`KeepAwake::revision`]. Ordering stamp, not a value to render.
    pub revision: u64,
}

/// The current keep-awake status. Stamps the snapshot like [`emit_status`] does,
/// so a read and an event are ordered against each other: without it a read
/// taken after an event was snapshotted — but before that event was emitted —
/// would carry the *same* revision while describing newer state, and the
/// frontend would then let the older event overwrite it.
pub fn status(state: &AppState) -> KeepAwakeStatus {
    let mut k = state.keep_awake.lock_safe();
    k.revision += 1;
    status_from(&k)
}

fn status_from(k: &KeepAwake) -> KeepAwakeStatus {
    KeepAwakeStatus {
        active: k.active,
        lid_close: k.lid_close,
        phase: k.phase,
        options: k.options,
        notice: k.notice,
        power_source: k.power_source,
        battery_percent: k.battery_percent,
        kernel_sleep_disabled: k.kernel_sleep_disabled,
        owns_lid_close: k.ownership.may_own(),
        leftover_undecided: k.leftover_undecided,
        long_running_processes: k.long_running_processes.clone(),
        revision: k.revision,
    }
}

/// Turn sleep prevention on or off, returning the resulting status.
pub fn set(app: &AppHandle, enabled: bool, options: Option<KeepAwakeOptions>) -> KeepAwakeStatus {
    let state = app.state::<AppState>();
    {
        let mut k = state.keep_awake.lock_safe();
        if k.phase.is_pending() {
            return status_from(&k);
        }
        // A leftover override awaits the user's decision: an ordinary on or off
        // from here — the panel's Start, the tray item, a hotkey — would end in
        // a clear justified by nothing but the old marker. Only the two
        // recovery actions move the state on.
        if k.leftover_undecided {
            tracing::debug!(
                "keep-awake toggle refused: a leftover lid-close override is undecided"
            );
            return status_from(&k);
        }
        if let Some(options) = options {
            k.options = options;
        }
        k.notice = None;
        k.retry_target = None;
        // An explicit request re-arms the guards, so a declined automatic
        // turn-off can be retried once more for the session it left running.
        k.auto_off_attempted = false;
        // An absolute end time that has already passed cannot bound this session,
        // so it is stale rather than a reason to refuse: drop it and carry on.
        // Refusing instead would dead-end the tray item and the global shortcut —
        // neither can edit the end time, so keep-awake would silently do nothing
        // until the panel was reopened and the deadline changed by hand.
        if enabled && deadline_is_stale(k.options, unix_time_ms()) {
            k.options.ends_at_ms = None;
        }
        if enabled && k.options.ac_only && k.power_source != PowerSource::Ac {
            k.phase = KeepAwakePhase::Failed;
            k.notice = Some(KeepAwakeNotice::AcRequired);
            k.retry_target = Some(true);
            drop(k);
            notify(app);
            return status(state.inner());
        }
    }
    if enabled { engage(app) } else { disengage(app) }
}

/// Flip sleep prevention. Used by the tray item and the `ToggleKeepAwake`
/// action (hotkeys/leader/taps).
pub fn toggle(app: &AppHandle) -> KeepAwakeStatus {
    let state = app.state::<AppState>();
    let k = state.keep_awake.lock_safe();
    if k.phase.is_pending() {
        return status_from(&k);
    }
    let active = k.active;
    drop(k);
    set(app, !active, None)
}

/// Update the session-only timer and power guards without changing the master
/// switch. Active sessions pick up the new policy on the next monitor tick.
pub fn configure(app: &AppHandle, options: KeepAwakeOptions) -> KeepAwakeStatus {
    let state = app.state::<AppState>();
    let mut k = state.keep_awake.lock_safe();
    k.options = options;
    if !k.active && k.options.duration_secs.is_some() {
        k.options.ends_at_ms = None;
    } else if k.active && k.options.duration_secs.is_some() && k.options.ends_at_ms.is_none() {
        arm_duration(&mut k.options);
    }
    drop(k);
    notify(app);
    status(state.inner())
}

/// Retry whichever direction most recently failed authorization or a safety
/// precondition. No-op when there is no failed transition.
///
/// A retry toward *off* while keep-awake is already off is the recovery of a
/// lid-close override that is only possibly ours (`lidCloseUnconfirmed`): there
/// is nothing to turn off in the ordinary sense — `disengage` would return at
/// once — so it runs the clear directly, as its own stamped transition.
pub fn retry(app: &AppHandle) -> KeepAwakeStatus {
    let state = app.state::<AppState>();
    let (target, recovery) = {
        let k = state.keep_awake.lock_safe();
        (
            k.retry_target,
            k.retry_target == Some(false) && !k.active && k.ownership.may_own(),
        )
    };
    if recovery {
        return recover_lid_close(app);
    }
    target.map_or_else(|| status(state.inner()), |target| set(app, target, None))
}

/// Clear a lid-close override that is possibly ours while keep-awake is off:
/// the recovery behind the `lidCloseUnconfirmed` notice. Stamped and run on the
/// worker like any other transition, so a cancel can supersede it and the
/// writeback (`Off`, or `RecoveryFailed`) decides what the UI shows.
fn recover_lid_close(app: &AppHandle) -> KeepAwakeStatus {
    let state = app.state::<AppState>();
    #[cfg(target_os = "macos")]
    let request_generation = {
        let mut k = state.keep_awake.lock_safe();
        if k.phase.is_pending() {
            return status_from(&k);
        }
        k.generation = k.generation.wrapping_add(1);
        k.phase = KeepAwakePhase::Disabling;
        k.notice = None;
        k.retry_target = None;
        // The user chose to clear it: from here the override is ours to
        // finish clearing, on off and at exit included.
        k.leftover_undecided = false;
        k.generation
    };
    #[cfg(target_os = "macos")]
    {
        notify(app);
        spawn_reconcile(app.clone(), false, request_generation);
    }
    status(state.inner())
}

/// Cancel the administrator prompt and drive the system back toward the stable
/// state that preceded it. The reverse reconcile is still serialized behind the
/// canceled worker, which keeps ownership correct if `pmset` won the race and
/// changed the kernel flag just before cancellation.
pub fn cancel_transition(app: &AppHandle) -> KeepAwakeStatus {
    let state = app.state::<AppState>();
    #[cfg(target_os = "macos")]
    let (reverse_target, request_generation) = {
        let mut k = state.keep_awake.lock_safe();
        let reverse_target = match k.phase {
            KeepAwakePhase::Enabling => false,
            // Cancelling the *recovery* clear (`recover_lid_close`: disabling
            // while already off) has no "on" to go back to. Reversing it would
            // start an enable — re-asserting `disablesleep` if the clear had
            // just landed. Supersede the worker and return to the unresolved
            // state the recovery started from, retry still armed.
            KeepAwakePhase::Disabling if !k.active => {
                k.generation = k.generation.wrapping_add(1);
                k.phase = KeepAwakePhase::Failed;
                k.notice = Some(KeepAwakeNotice::LidCloseUnconfirmed);
                k.retry_target = Some(false);
                drop(k);
                kill_authorization();
                notify(app);
                return status(state.inner());
            }
            KeepAwakePhase::Disabling => true,
            _ => return status_from(&k),
        };
        k.generation = k.generation.wrapping_add(1);
        k.phase = if reverse_target {
            KeepAwakePhase::Enabling
        } else {
            KeepAwakePhase::Disabling
        };
        k.notice = None;
        k.retry_target = None;
        (reverse_target, k.generation)
    };
    #[cfg(target_os = "macos")]
    {
        kill_authorization();
        notify(app);
        spawn_reconcile(app.clone(), reverse_target, request_generation);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
    status(state.inner())
}

/// Start the low-frequency system monitor that enforces timers and power guards
/// even while the settings window is closed.
pub fn start_monitor(app: &AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || {
        loop {
            refresh_system_status(&handle);
            std::thread::sleep(std::time::Duration::from_secs(10));
        }
    });
}

fn refresh_system_status(app: &AppHandle) {
    let (power_source, battery_percent) = read_power_status();
    #[cfg(target_os = "macos")]
    let kernel_sleep_disabled = read_sleep_disabled();
    #[cfg(not(target_os = "macos"))]
    let kernel_sleep_disabled = None;
    let processes = detect_long_running_processes();
    let now = unix_time_ms();
    let state = app.state::<AppState>();
    // `Some(request_generation)` once a guard has stamped its turn-off; the
    // worker is spawned after the lock is released.
    let mut automatic_off: Option<Option<u64>> = None;
    let changed = {
        let mut k = state.keep_awake.lock_safe();
        let before = (
            k.power_source,
            k.battery_percent,
            k.kernel_sleep_disabled,
            k.long_running_processes.clone(),
            k.notice,
        );
        k.power_source = power_source;
        k.battery_percent = battery_percent;
        k.kernel_sleep_disabled = kernel_sleep_disabled;
        k.long_running_processes = processes;

        // A guard must not interrupt an administrator prompt that is already up,
        // and must fire at most once per session: `disengage` needs its own
        // (declinable) prompt, so re-deciding every tick while a declined
        // auto-off sits in `Failed` would reopen that dialog every ten seconds.
        // Note the block is *not* `phase != On` — that would also disarm every
        // guard for a session left in `Failed` by a declined manual off, which
        // is still holding sleep off and is exactly when the guards matter.
        let blocked = guards_blocked(k.phase, k.auto_off_attempted);
        match safety_decision(
            k.active,
            blocked,
            k.options,
            power_source,
            battery_percent,
            now,
        ) {
            SafetyDecision::None if k.notice == Some(KeepAwakeNotice::LowBattery) => {
                k.notice = None
            }
            SafetyDecision::None => {}
            SafetyDecision::WarnLowBattery => k.notice = Some(KeepAwakeNotice::LowBattery),
            // Commit the decision — the notice *and* the transition itself —
            // under the same lock that made it, so an option edit landing in
            // between cannot have a stale verdict applied to it.
            SafetyDecision::TurnOff(notice) => {
                k.notice = Some(notice);
                k.auto_off_attempted = true;
                // The deadline has been spent. Clearing it here keeps the status
                // the panel renders honest (no countdown against a time already
                // past) and leaves the next session unbounded rather than
                // pre-expired.
                if notice == KeepAwakeNotice::TimerElapsed {
                    k.options.ends_at_ms = None;
                }
                automatic_off = Some(begin_disable(&mut k));
            }
        }
        before
            != (
                k.power_source,
                k.battery_percent,
                k.kernel_sleep_disabled,
                k.long_running_processes.clone(),
                k.notice,
            )
    };

    if let Some(request_generation) = automatic_off {
        finish_disable(app, request_generation);
    } else if changed {
        emit_status(app);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SafetyDecision {
    None,
    WarnLowBattery,
    TurnOff(KeepAwakeNotice),
}

/// Whether the safety guards must stay quiet. They are blocked while an
/// administrator prompt is up, and once a guard has already driven an automatic
/// turn-off for this session. They are deliberately *not* blocked by `Failed`:
/// a declined manual off leaves the session running with sleep still prevented,
/// which is exactly when a deadline or a dying battery should still act.
fn guards_blocked(phase: KeepAwakePhase, auto_off_attempted: bool) -> bool {
    phase.is_pending() || auto_off_attempted
}

fn safety_decision(
    active: bool,
    transition_blocked: bool,
    options: KeepAwakeOptions,
    power_source: PowerSource,
    battery_percent: Option<u8>,
    now: u64,
) -> SafetyDecision {
    if !active || transition_blocked {
        return SafetyDecision::None;
    }
    if options.ends_at_ms.is_some_and(|deadline| deadline <= now) {
        SafetyDecision::TurnOff(KeepAwakeNotice::TimerElapsed)
    } else if options.ac_only && power_source == PowerSource::Battery {
        SafetyDecision::TurnOff(KeepAwakeNotice::AcDisconnected)
    } else if battery_percent.is_some_and(|percent| percent <= LOW_BATTERY_PERCENT) {
        match options.low_battery_action {
            LowBatteryAction::Warn => SafetyDecision::WarnLowBattery,
            LowBatteryAction::TurnOff => SafetyDecision::TurnOff(KeepAwakeNotice::LowBattery),
        }
    } else {
        SafetyDecision::None
    }
}

fn unix_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

/// Whether the configured absolute end time is already spent, so engaging would
/// otherwise start a session the very next monitor tick tears down. Relative
/// presets are excluded: [`arm_duration`] re-materializes those on every engage,
/// so their `ends_at_ms` is never stale.
fn deadline_is_stale(options: KeepAwakeOptions, now: u64) -> bool {
    options.duration_secs.is_none() && options.ends_at_ms.is_some_and(|deadline| deadline <= now)
}

fn arm_duration(options: &mut KeepAwakeOptions) {
    if let Some(duration_secs) = options.duration_secs {
        options.ends_at_ms =
            Some(unix_time_ms().saturating_add(duration_secs.saturating_mul(1_000)));
    }
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
    // The generation stamped on this engage, captured under the lock that bumps
    // it so the spawned worker reconciles against *this* request, not whatever
    // the latest generation happens to be by the time it wins `LID_OP_LOCK`.
    #[cfg(target_os = "macos")]
    let request_generation;
    {
        let mut k = state.keep_awake.lock_safe();
        if k.active {
            // Already on — nothing to do. Pending phases were rejected by `set`
            // before reaching here; cancellation uses `spawn_reconcile` directly
            // to reverse an in-flight operation under a new generation.
            return status_from(&k);
        }
        // Re-check shutdown *under the lock*. `cleanup_blocking` sets
        // SHUTTING_DOWN before it takes this lock, so an engage that raced it
        // past the unlocked check above would otherwise still slip through here,
        // create an assertion, and spawn a worker that re-engages the lid-close
        // override after cleanup already cleared it — stranding the Mac awake
        // past exit. Bailing here closes that window.
        if SHUTTING_DOWN.load(std::sync::atomic::Ordering::Acquire) {
            return status_from(&k);
        }
        // Relative presets begin when the session actually engages, including
        // tray and shortcut starts that happen after the panel was closed.
        arm_duration(&mut k.options);
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
                    k.phase = KeepAwakePhase::Enabling;
                    k.generation = k.generation.wrapping_add(1);
                    request_generation = k.generation;
                    engaged = true;
                }
                Err(rc) => {
                    tracing::warn!(
                        rc,
                        "failed to create power assertion; keep-awake not engaged"
                    );
                    request_generation = k.generation;
                    engaged = false;
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            k.active = true;
            k.lid_close = LidCloseState::Off;
            k.phase = KeepAwakePhase::On;
        }
    }
    // Re-sync every surface — including a tray checkmark a click just toggled on,
    // which a failed engage must reset. On success, engage the lid-close veto in
    // the background so its admin-auth dialog never blocks the caller.
    notify(app);
    #[cfg(target_os = "macos")]
    if engaged {
        spawn_reconcile(app.clone(), true, request_generation);
    }
    status(state.inner())
}

/// Stamp a turn-off onto state the caller already holds the lock for, returning
/// the generation a reconcile worker must carry (`None` when there is no worker
/// to spawn). Split out from [`disengage`] so a caller that *decided* to turn off
/// under the lock — the safety monitor — can commit that decision in the same
/// critical section: otherwise an option edit landing in the gap would have a
/// stale verdict applied to it, and a toggle in the same gap could leave two
/// workers racing the same transition.
fn begin_disable(k: &mut KeepAwake) -> Option<u64> {
    if !k.active {
        k.phase = KeepAwakePhase::Off;
        return None;
    }
    #[cfg(target_os = "macos")]
    {
        // Do *not* flip `active` or release the assertion yet. Turning off means
        // clearing the lid-close override, which needs an admin dialog that can
        // be declined — and until it is cleared, sleep is still prevented. The
        // worker commits the off (`active = false`, assertion released) only once
        // the clear succeeds; a declined clear keeps keep-awake on rather than
        // show it off while sleep stays blocked. Stamp this transition so an
        // explicit cancellation can supersede it.
        k.generation = k.generation.wrapping_add(1);
        k.phase = KeepAwakePhase::Disabling;
        Some(k.generation)
    }
    #[cfg(not(target_os = "macos"))]
    {
        // No lid-close veto off-platform, so the off is immediate.
        k.active = false;
        k.lid_close = LidCloseState::Off;
        k.phase = KeepAwakePhase::Off;
        None
    }
}

/// Publish a stamped turn-off and hand the (slow, admin-authed) clear to a
/// worker. Always called after [`begin_disable`], with the state lock released.
fn finish_disable(app: &AppHandle, request_generation: Option<u64>) {
    // Publish the pending phase immediately so every entry point is disabled
    // while the administrator prompt is open.
    notify(app);
    #[cfg(target_os = "macos")]
    if let Some(request_generation) = request_generation {
        spawn_reconcile(app.clone(), false, request_generation);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = request_generation;
}

fn disengage(app: &AppHandle) -> KeepAwakeStatus {
    let state = app.state::<AppState>();
    let request_generation = {
        let mut k = state.keep_awake.lock_safe();
        if !k.active {
            k.phase = KeepAwakePhase::Off;
            return status_from(&k);
        }
        begin_disable(&mut k)
    };
    finish_disable(app, request_generation);
    status(state.inner())
}

/// Emit the change event and rebuild the tray menu (on the main thread, as the
/// menu APIs require) so the panel and the tray checkmark both follow.
fn notify(app: &AppHandle) {
    emit_status(app);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || crate::tray::refresh(&handle));
}

fn emit_status(app: &AppHandle) {
    let state = app.state::<AppState>();
    let status = {
        let mut k = state.keep_awake.lock_safe();
        // Stamp the snapshot under the lock that produced it, so a higher
        // revision always means a later-issued snapshot — even when two emitting
        // threads are descheduled between snapshotting and `emit`, and their
        // events reach the webview in the opposite order.
        k.revision += 1;
        status_from(&k)
    };
    let _ = app.emit(CHANGED_EVENT, status);
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
    /// Marker and the override is still set after an unclean exit. The marker
    /// alone cannot say whose the override is now, so it is not cleared on that
    /// evidence: the user is asked (see [`KeepAwakeNotice::LeftoverOverride`]).
    AskUser,
}

fn reconcile_decision(marker_present: bool, sleep_disabled: Option<bool>) -> ReconcileAction {
    match (marker_present, sleep_disabled) {
        (false, _) => ReconcileAction::Nothing,
        (true, None) => ReconcileAction::Keep,
        (true, Some(false)) => ReconcileAction::RemoveMarker,
        (true, Some(true)) => ReconcileAction::AskUser,
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
    ownership: Ownership,
}

/// Drive the lid-close veto toward the desired `active` state, returning the
/// resulting lid-close status and ownership. Free of state mutation beyond the
/// `sys` calls (and a diagnostic log), so it is exercised end-to-end in tests
/// against a fake.
///
/// Every `set_disablesleep` is confirmed against the *actual* kernel flag via a
/// follow-up read rather than trusting the setter's reported result: `pmset`
/// can apply the change and still have `osascript` report failure. The marker
/// and ownership therefore track the verified kernel state — the failsafe is
/// dropped only when the override is confirmed clear, and kept whenever it is
/// still set or unreadable, so the Mac is never left unable to sleep with no
/// record to recover from.
fn reconcile_lid_close_with<S: LidCloseSys>(
    sys: &S,
    active: bool,
    ownership: Ownership,
) -> LidCloseOutcome {
    if !active {
        // Toggled off: only an override we engaged ourselves — or might have —
        // is ours to clear.
        if !ownership.may_own() {
            return LidCloseOutcome {
                lid_close: LidCloseState::Off,
                ownership: Ownership::Unowned,
            };
        }
        // Attempt the clear, then confirm against the kernel flag rather than the
        // setter's result. Drop the marker and release ownership only once the
        // override is *confirmed* cleared; if it is still set (auth declined) or
        // unreadable, keep both so the next launch's reconcile / cleanup retries
        // rather than leaking the override.
        sys.set_disablesleep(false);
        let ownership = match sys.read_sleep_disabled() {
            Some(false) => {
                sys.remove_marker();
                Ownership::Unowned
            }
            Some(true) | None => ownership,
        };
        return LidCloseOutcome {
            lid_close: LidCloseState::Off,
            ownership,
        };
    }
    // If we already own the override it is on by definition; otherwise read it.
    //
    // Known, accepted TOCTOU window: nothing locks `disablesleep` between this
    // read and the `set_disablesleep(true)` call below (`Some(false)` arm), so
    // a concurrent `pmset disablesleep 1` from the user or another process in
    // that gap would be silently overwritten by ours, and we would (wrongly,
    // but harmlessly) record ourselves as the owner instead of them. There is
    // no macOS API to set `SleepDisabled` conditionally on its prior value, and
    // the race requires another process to touch this exact, rarely-toggled
    // flag in the same instant we do — accepted rather than engineered around.
    let sleep_disabled = match ownership {
        Ownership::Confirmed => Some(true),
        Ownership::PossiblyOwned | Ownership::Unowned => sys.read_sleep_disabled(),
    };
    match sleep_disabled {
        // Already vetoed — by us, or by the user/another process. The lid-close
        // guarantee holds either way. Ownership is carried through unchanged:
        // a value we did not set is never taken over (so we never clear someone
        // else's later), and a *possibly* ours stays possibly ours — reading it
        // set now is consistent with our unconfirmed enable but does not prove
        // it (the compensating clear may have worked and someone else set it
        // since), so it keeps being cleared on off and reported as unresolved.
        Some(true) => LidCloseOutcome {
            lid_close: LidCloseState::Engaged,
            ownership,
        },
        // Safe to enable. Persist the marker *before* enabling so a crash in
        // between leaves a record the next launch reconciles. If the marker
        // cannot be written, don't enable — recovery couldn't be guaranteed.
        // This pure step never touches `active`; the worker turns
        // `Unavailable`-for-a-wanted-on into a full roll-back, since the veto is
        // a required part of keep-awake.
        Some(false) => {
            if !sys.write_marker() {
                // No failsafe on disk → don't enable; there would be no record
                // to recover from after a crash.
                sys.remove_marker();
                return LidCloseOutcome {
                    lid_close: LidCloseState::Unavailable,
                    ownership: Ownership::Unowned,
                };
            }
            // Enable, then confirm against the kernel flag instead of the
            // setter's result: `osascript` can report failure after `pmset`
            // already set `SleepDisabled`, and dropping the marker then would
            // strand the Mac awake with no record to recover from.
            sys.set_disablesleep(true);
            confirm_enable(sys)
        }
        // Unreadable: don't clobber an override we can't see, and don't claim a
        // guarantee we can't make.
        None => {
            tracing::warn!("could not read sleep state; not engaging lid-close veto");
            LidCloseOutcome {
                lid_close: LidCloseState::Unavailable,
                ownership,
            }
        }
    }
}

/// Settle an enable that has just run against the kernel flag.
///
/// Read back set: ours, confirmed. Read back clear (auth declined / `pmset`
/// failed): the enable did nothing, drop the marker. Unreadable is the case
/// that must not end in "off" with the Mac unable to sleep: the flag *was* clear
/// before the enable, so if it is set now the enable took. Rather than give up,
/// read once more, and if that fails too, compensate — clear the override in
/// this same worker and confirm *that*. Only when the clear cannot be confirmed
/// either is the override left as [`Ownership::PossiblyOwned`], marker kept, for
/// off, exit and the next reconcile to finish; the worker reports that state
/// rather than a plain failure so the UI shows it as unresolved.
fn confirm_enable<S: LidCloseSys>(sys: &S) -> LidCloseOutcome {
    let confirmed = || LidCloseOutcome {
        lid_close: LidCloseState::Engaged,
        ownership: Ownership::Confirmed,
    };
    let did_nothing = || {
        sys.remove_marker();
        LidCloseOutcome {
            lid_close: LidCloseState::Unavailable,
            ownership: Ownership::Unowned,
        }
    };
    match sys.read_sleep_disabled() {
        Some(true) => return confirmed(),
        Some(false) => return did_nothing(),
        None => {}
    }
    // One more read before doing anything drastic: a transient `pmset -g`
    // failure is the common cause.
    match sys.read_sleep_disabled() {
        Some(true) => return confirmed(),
        Some(false) => return did_nothing(),
        None => {}
    }
    tracing::warn!(
        "could not confirm sleep state after enabling lid-close veto; clearing it again rather \
         than leave the Mac possibly unable to sleep"
    );
    sys.set_disablesleep(false);
    match sys.read_sleep_disabled() {
        // Compensation confirmed: the enable is fully undone.
        Some(false) => did_nothing(),
        // Still set after our clear: it was clear before we began, so this is
        // ours and the clear was declined — own it and stay engaged rather than
        // report off over a live override.
        Some(true) => confirmed(),
        // Nothing can be read: the override may be in force. Keep the marker as
        // the failsafe and remember it as possibly ours, so off and exit clear
        // it, a later read that finds it set confirms it, and the UI says so.
        None => {
            tracing::warn!(
                "could not confirm the compensating clear either; keeping the failsafe marker and \
                 treating the lid-close override as possibly ours"
            );
            LidCloseOutcome {
                lid_close: LidCloseState::Unavailable,
                ownership: Ownership::PossiblyOwned,
            }
        }
    }
}

/// The committed keep-awake state a reconcile worker resolves to. Canceling an
/// admin-auth dialog bumps the generation; a superseded worker must then leave
/// `active` and the assertion to the reverse transition's own worker.
#[derive(Debug, PartialEq, Eq)]
enum ReconcileWriteback {
    /// A newer transition superseded this cycle: record ownership only, touch nothing
    /// else (the newer cycle's worker sets the final state).
    Superseded,
    /// Keep-awake on: idle assertion held and lid-close veto engaged.
    On,
    /// Keep-awake off after the lid-close override was confirmed clear.
    Off,
    /// Enabling failed before the mandatory lid-close veto was established.
    EnableFailed,
    /// Enabling failed *and* the override may nonetheless be in force: our
    /// enable ran but could not be confirmed, and neither could the clear that
    /// followed. Keep-awake is off, but the Mac may be unable to sleep until the
    /// clear is retried.
    EnableUnconfirmed,
    /// Disabling failed and Tomari still owns a live lid-close veto.
    DisableFailed,
    /// A recovery clear — run from the off state, for an override that is only
    /// possibly ours — could not be confirmed. Keep-awake stays off (nothing
    /// holds an assertion), the unresolved notice stays, and retry stays armed.
    RecoveryFailed,
}

/// Decide the committed keep-awake state from a reconcile, given whether the
/// worker was superseded (its captured generation no longer matches), the
/// direction it reconciled toward (`desired_on`), and the resulting lid-close
/// state and ownership. Pure, so the roll-back / stay-on / supersession policy is
/// unit-tested.
fn reconcile_writeback(
    superseded: bool,
    desired_on: bool,
    was_active: bool,
    lid_close: LidCloseState,
    ownership: Ownership,
) -> ReconcileWriteback {
    if superseded {
        ReconcileWriteback::Superseded
    } else if desired_on {
        // Turning on: the veto is mandatory, so commit on only if it engaged;
        // otherwise (declined / unreadable) roll the whole switch back off —
        // saying so when the override may still be in force.
        if lid_close == LidCloseState::Engaged {
            ReconcileWriteback::On
        } else if ownership == Ownership::PossiblyOwned {
            ReconcileWriteback::EnableUnconfirmed
        } else {
            ReconcileWriteback::EnableFailed
        }
    } else if ownership.may_own() {
        // Turning off but the override clear was declined (or could not be
        // confirmed): we still own it — or might — so sleep may still be
        // prevented. From on, keep keep-awake on instead of lying it is off.
        // From off (a recovery of a possibly-owned override) there is no "on"
        // to fall back to: stay off, keep saying it is unresolved.
        if was_active {
            ReconcileWriteback::DisableFailed
        } else {
            ReconcileWriteback::RecoveryFailed
        }
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
fn cleanup_lid_close_with<S: LidCloseSys>(sys: &S, ownership: Ownership) -> Ownership {
    if !(ownership.may_own() || sys.marker_exists()) {
        // Nothing of ours to clear; never touch a foreign override.
        return ownership;
    }
    // Confirm the clear against the kernel flag rather than the setter's result:
    // trusting a reported success that did not take would drop the failsafe
    // marker while `SleepDisabled` is still set, stranding the Mac awake with no
    // record. Remove the marker only once the override is confirmed cleared.
    sys.set_disablesleep(false);
    match sys.read_sleep_disabled() {
        Some(false) => {
            sys.remove_marker();
            Ownership::Unowned
        }
        // Still set, or unreadable: keep the marker so the next launch's
        // reconcile clears the leftover rather than leaking the override.
        Some(true) | None => ownership,
    }
}

/// Reconcile a lid-close override left behind by a previous run that exited
/// without cleaning up (a crash or a forced kill). Keep-awake never persists as
/// "on", so at launch the intended state is always off — but a leftover marker
/// is not proof that the override still set is ours: the kernel flag records no
/// provenance, and the user or another tool may have set it since. So nothing
/// is cleared here on the marker's evidence alone (no auth prompt appears at
/// launch); the override is carried into the runtime state as *possibly* ours
/// and surfaced for the user to decide — Retry clears it, dismissing keeps it
/// and drops the marker. A marker whose override is already gone (a reboot
/// cleared it) is simply dropped.
pub fn reconcile_on_launch(app: &AppHandle) {
    #[cfg(not(target_os = "macos"))]
    let _ = app;
    #[cfg(target_os = "macos")]
    {
        let notice = match reconcile_decision(marker_exists(), read_sleep_disabled()) {
            ReconcileAction::Nothing => None,
            ReconcileAction::Keep => {
                tracing::warn!(
                    "could not read sleep state at launch; keeping the keep-awake marker"
                );
                Some(KeepAwakeNotice::LidCloseUnconfirmed)
            }
            ReconcileAction::RemoveMarker => {
                remove_marker();
                None
            }
            ReconcileAction::AskUser => {
                tracing::warn!(
                    "a previous run left its keep-awake marker with the lid-close sleep override                      still set; leaving it for the user to decide"
                );
                Some(KeepAwakeNotice::LeftoverOverride)
            }
        };
        // A marker that survives launch is an override we may still be holding.
        // Carry that into the runtime state — as *possibly* ours, since the
        // previous run's confirmation is gone with it — so a later engage does
        // not mistake the set flag for someone else's, and the UI offers the
        // decision instead of showing a clean off.
        if let Some(notice) = notice {
            let state = app.state::<AppState>();
            let mut k = state.keep_awake.lock_safe();
            k.ownership = Ownership::PossiblyOwned;
            k.phase = KeepAwakePhase::Failed;
            k.notice = Some(notice);
            k.retry_target = Some(false);
            k.leftover_undecided = true;
        }
    }
}

/// The user chose to leave a leftover lid-close override in place (see
/// [`KeepAwakeNotice::LeftoverOverride`]): it is not ours to clear any more.
/// Drops the marker so the next launch does not ask again — verified gone
/// before the state is committed, since a marker left behind would make exit
/// clear the override after all — and returns keep-awake to a clean off. A
/// no-op unless that decision is pending.
pub fn dismiss_leftover(app: &AppHandle) -> KeepAwakeStatus {
    let state = app.state::<AppState>();
    {
        let mut k = state.keep_awake.lock_safe();
        if !k.leftover_undecided {
            return status_from(&k);
        }
        #[cfg(target_os = "macos")]
        {
            remove_marker();
            if marker_exists() {
                tracing::warn!(
                    "could not drop the keep-awake marker; the leftover lid-close override stays                      undecided"
                );
                return status_from(&k);
            }
        }
        k.ownership = Ownership::Unowned;
        k.phase = KeepAwakePhase::Off;
        k.notice = None;
        k.retry_target = None;
        k.leftover_undecided = false;
    }
    tracing::info!("leftover lid-close sleep override left in place at the user's request");
    notify(app);
    status(state.inner())
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
        k.phase = KeepAwakePhase::Off;
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
        // late `pmset 1` re-enables it and strands the Mac awake. The generation
        // bumped above also makes a worker still queued for the lock skip its
        // side effects entirely; the write-ahead marker is the backstop for
        // anything that still slips past.
        //
        // A worker that holds the lock is sitting behind an administrator
        // dialog, and the lock is released only when that dialog is answered or
        // its `osascript` dies — so the wait is bounded by killing the dialog,
        // repeatedly: a worker that spawns its `osascript` just after one kill
        // (it had won the lock before `SHUTTING_DOWN` was set) is caught by the
        // next. Past the deadline the clear is skipped altogether rather than
        // run unserialized; the marker then carries the override to the next
        // launch, which is exactly what it is for.
        // Lock order is always LID_OP_LOCK → keep_awake (never the reverse).
        let until = std::time::Instant::now() + EXIT_LOCK_DEADLINE;
        let _op = loop {
            kill_authorization();
            if let Some(guard) = LID_OP_LOCK.try_lock_safe() {
                break guard;
            }
            if std::time::Instant::now() >= until {
                tracing::warn!(
                    "a keep-awake worker did not release its lock before exit; leaving the \
                     lid-close override to the next launch's reconcile"
                );
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        let (ownership, undecided) = {
            let k = state.keep_awake.lock_safe();
            (k.ownership, k.leftover_undecided)
        };
        if undecided {
            // The override found at launch was never attributed to us and the
            // user has not decided; clearing it on exit would act on the very
            // evidence launch declined to act on. Keep the marker so the next
            // launch asks again.
            tracing::info!(
                "leaving an unattributed lid-close sleep override in place at exit; the next launch                  will ask again"
            );
            return;
        }
        let remaining = cleanup_lid_close_with(&RealSys::EXITING, ownership);
        if remaining != ownership {
            state.keep_awake.lock_safe().ownership = remaining;
        }
    }
}

// ---------------------------------------------------------------------------
// macOS implementation
// ---------------------------------------------------------------------------

/// Reconcile the lid-close veto on a worker thread, since the admin-auth dialog
/// blocks. `desired_on` is the direction the request asked for (engage /
/// disengage); `request_generation` is the generation stamped on that request,
/// used to detect a cancellation superseding this one.
#[cfg(target_os = "macos")]
fn spawn_reconcile(app: AppHandle, desired_on: bool, request_generation: u64) {
    std::thread::spawn(move || reconcile_lid_close(&app, desired_on, request_generation));
}

/// Drive `pmset disablesleep` toward `desired_on` and commit the resulting
/// keep-awake state. Serialized by `LID_OP_LOCK`, including a reverse operation
/// queued by explicit cancellation.
///
/// The lid-close veto is a required part of keep-awake and both directions go
/// through it, so the worker — not the toggle — commits `active`:
/// * Turning **on**: if the veto cannot be engaged (auth declined, or sleep state
///   unreadable) the whole switch rolls back off.
/// * Turning **off**: the off takes effect only once the override is actually
///   cleared; a declined clear leaves keep-awake on (sleep is still prevented).
///
/// A cancellation that superseded this cycle while the admin-auth dialog was up
/// is detected via `request_generation` — the generation captured when *this*
/// worker was spawned — and leaves the final state to the reverse worker.
#[cfg(target_os = "macos")]
fn reconcile_lid_close(app: &AppHandle, desired_on: bool, request_generation: u64) {
    let _op = LID_OP_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let state = app.state::<AppState>();
    // Nothing has been touched yet, so a cycle already superseded here must run
    // no side effects at all. Without this the lock could hand out in the wrong
    // order — a cancellation's reverse worker (or `cleanup_blocking`, which also
    // bumps the generation) winning `LID_OP_LOCK` ahead of the worker it
    // superseded — and the loser would then go on to enable `disablesleep`
    // *after* the winner cleared it, stranding the Mac unable to sleep while
    // every surface reports keep-awake off. Skipping is safe precisely because
    // this runs before the first `pmset`: a worker already past this point still
    // completes and records the ownership it acquired, exactly as the
    // cancel-during-the-dialog path relies on.
    if state.keep_awake.lock_safe().generation != request_generation {
        return;
    }
    let ownership = state.keep_awake.lock_safe().ownership;
    // Run the (possibly slow, admin-authed) side effects without holding the
    // state lock, then store the resulting status back in one shot.
    let outcome = reconcile_lid_close_with(&RealSys::INTERACTIVE, desired_on, ownership);
    {
        let mut k = state.keep_awake.lock_safe();
        // Always record ownership — even when superseded — so a later worker or
        // cleanup can clear an override this cycle set.
        k.ownership = outcome.ownership;
        // Compare the *current* generation against the one stamped when this
        // worker was spawned. Reading it here (after winning `LID_OP_LOCK`)
        // instead would compare against whatever the latest transition set and
        // could commit a stale authorization result ahead of its reverse worker.
        match reconcile_writeback(
            k.generation != request_generation,
            desired_on,
            k.active,
            outcome.lid_close,
            outcome.ownership,
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
                k.phase = KeepAwakePhase::On;
                k.notice = None;
                k.retry_target = None;
                // `auto_off_attempted` is deliberately *not* cleared here.
                // Cancelling an automatic turn-off lands right in this arm, and
                // the condition that triggered it (on battery, battery low) is
                // usually still true — re-arming would put the administrator
                // prompt back on screen on the very next monitor tick. Only an
                // explicit request from the user re-arms it, in `set`.
            }
            ReconcileWriteback::Off => {
                if let Some(id) = k.assertion.take() {
                    release_assertion(id);
                }
                k.active = false;
                k.lid_close = LidCloseState::Off;
                k.phase = KeepAwakePhase::Off;
                k.retry_target = None;
                // A confirmed clear is exactly what resolves this notice.
                if k.notice == Some(KeepAwakeNotice::LidCloseUnconfirmed) {
                    k.notice = None;
                }
            }
            ReconcileWriteback::RecoveryFailed => {
                k.active = false;
                k.lid_close = LidCloseState::Off;
                k.phase = KeepAwakePhase::Failed;
                k.notice = Some(KeepAwakeNotice::LidCloseUnconfirmed);
                k.retry_target = Some(false);
            }
            ReconcileWriteback::EnableFailed => {
                if let Some(id) = k.assertion.take() {
                    release_assertion(id);
                }
                k.active = false;
                k.lid_close = LidCloseState::Off;
                k.phase = KeepAwakePhase::Failed;
                k.notice = Some(KeepAwakeNotice::AuthorizationDeclined);
                k.retry_target = Some(true);
            }
            ReconcileWriteback::EnableUnconfirmed => {
                if let Some(id) = k.assertion.take() {
                    release_assertion(id);
                }
                // Off, but not clean: the override may be live. The notice says
                // so, `owns_lid_close` stays true, and a retry runs the *clear*
                // (not another enable) so the user can finish the recovery.
                k.active = false;
                k.lid_close = LidCloseState::Off;
                k.phase = KeepAwakePhase::Failed;
                k.notice = Some(KeepAwakeNotice::LidCloseUnconfirmed);
                k.retry_target = Some(false);
            }
            ReconcileWriteback::DisableFailed => {
                k.active = true;
                k.lid_close = LidCloseState::Engaged;
                k.phase = KeepAwakePhase::Failed;
                k.notice = Some(KeepAwakeNotice::AuthorizationDeclined);
                k.retry_target = Some(false);
            }
        }
    }
    notify(app);
}

/// Production [`LidCloseSys`]: the real `pmset` calls and on-disk marker.
#[cfg(target_os = "macos")]
struct RealSys {
    /// How long an administrator dialog may stay unanswered before the `pmset`
    /// is abandoned. `None` waits for the user — the interactive toggles, which
    /// have their own Cancel. Exit paths set a deadline: nobody may be there to
    /// answer, and the process has to be allowed to end.
    auth_deadline: Option<std::time::Duration>,
}

#[cfg(target_os = "macos")]
impl RealSys {
    /// Wait for the user: they asked for this, and can cancel it.
    const INTERACTIVE: Self = Self {
        auth_deadline: None,
    };
    /// Bounded, for quit / logout / the updater's relaunch. (The launch
    /// reconcile shows no dialog at all: it clears nothing on the marker's
    /// evidence alone and leaves the decision to the user.)
    const EXITING: Self = Self {
        auth_deadline: Some(EXIT_AUTH_DEADLINE),
    };
}

/// How long exit-time cleanup waits on the administrator dialog. Logout and the
/// updater are waiting behind it; an unanswered dialog past this is abandoned,
/// the override kept as ours with its marker, and the next launch recovers it.
#[cfg(target_os = "macos")]
const EXIT_AUTH_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// How long exit-time cleanup waits for a reconcile worker to give up
/// `LID_OP_LOCK` (its dialog is being killed the whole time) before skipping
/// the clear.
#[cfg(target_os = "macos")]
const EXIT_LOCK_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// How long a `pmset -g` / `pmset -g batt` / `ps` read may take. None of them
/// needs user input, so a read that takes longer is a stuck process, not a slow
/// one; it is killed and the value reported unknown. This also keeps the single
/// safety monitor thread from stalling for good on one hung child.
const READ_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

#[cfg(target_os = "macos")]
impl LidCloseSys for RealSys {
    fn read_sleep_disabled(&self) -> Option<bool> {
        read_sleep_disabled()
    }
    fn set_disablesleep(&self, on: bool) -> bool {
        run_disablesleep(on, self.auth_deadline)
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
/// standard macOS auth dialog. Returns whether it succeeded. With a `deadline`,
/// a dialog still unanswered when it passes is torn down and counted as not
/// applied — the caller then confirms against the kernel flag as always, so an
/// override that did land in the meantime keeps its marker and ownership.
#[cfg(target_os = "macos")]
fn run_disablesleep(on: bool, deadline: Option<std::time::Duration>) -> bool {
    let value = if on { "1" } else { "0" };
    let script = format!(
        "do shell script \"/usr/bin/pmset -a disablesleep {value}\" with administrator privileges"
    );
    let child = std::process::Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(&script)
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(e) => {
            tracing::warn!(error = %e, on, "failed to run osascript for pmset disablesleep");
            return false;
        }
    };
    let pid = child.id();
    AUTH_PID.store(pid, std::sync::atomic::Ordering::Release);
    // A shutdown that began between `kill_authorization` and this spawn missed
    // the dialog; it will kill it on its next pass, but do not let an interactive
    // wait outlive the process regardless.
    let deadline = if SHUTTING_DOWN.load(std::sync::atomic::Ordering::Acquire) {
        Some(deadline.unwrap_or(EXIT_AUTH_DEADLINE))
    } else {
        deadline
    };
    let result = wait_child(&mut child, deadline);
    let _ = AUTH_PID.compare_exchange(
        pid,
        0,
        std::sync::atomic::Ordering::AcqRel,
        std::sync::atomic::Ordering::Acquire,
    );
    match result {
        ChildExit::Exited(status) if status.success() => true,
        ChildExit::Exited(status) => {
            tracing::warn!(code = ?status.code(), on, "pmset disablesleep was not applied (auth declined?)");
            false
        }
        ChildExit::TimedOut => {
            tracing::warn!(
                on,
                ?deadline,
                "administrator authorization was not answered in time; abandoning pmset disablesleep"
            );
            false
        }
        ChildExit::Failed(e) => {
            tracing::warn!(error = %e, on, "failed to run osascript for pmset disablesleep");
            false
        }
    }
}

/// Run `command` to completion within `deadline`, capturing its stdout. `None`
/// when it could not be spawned or did not finish in time (it is then killed and
/// reaped). Stdout is drained on a helper thread so a child that writes more
/// than the pipe holds cannot deadlock against the wait.
fn output_within(
    command: &mut std::process::Command,
    deadline: std::time::Duration,
) -> Option<std::process::Output> {
    use std::io::Read;

    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let status = match wait_child(&mut child, Some(deadline)) {
        ChildExit::Exited(status) => status,
        ChildExit::TimedOut => {
            tracing::warn!(
                ?deadline,
                "a pmset read did not finish in time; treating the state as unreadable"
            );
            return None;
        }
        ChildExit::Failed(_) => return None,
    };
    // The child has exited, so its end of the pipe is closed and the reader
    // returns promptly.
    let stdout = reader.join().ok()?;
    Some(std::process::Output {
        status,
        stdout,
        stderr: Vec::new(),
    })
}

/// How waiting on a child ended.
#[derive(Debug)]
enum ChildExit {
    Exited(std::process::ExitStatus),
    /// The deadline passed first; the child has been killed and reaped.
    TimedOut,
    Failed(std::io::Error),
}

/// Wait for `child`, for at most `deadline` when one is given. On timeout the
/// child is killed and reaped — never left as a zombie, and never left holding
/// a dialog on screen for a process that is exiting.
fn wait_child(child: &mut std::process::Child, deadline: Option<std::time::Duration>) -> ChildExit {
    let Some(deadline) = deadline else {
        return match child.wait() {
            Ok(status) => ChildExit::Exited(status),
            Err(e) => ChildExit::Failed(e),
        };
    };
    let until = std::time::Instant::now() + deadline;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return ChildExit::Exited(status),
            Ok(None) if std::time::Instant::now() >= until => {
                // Reap only a child the kill reached; a kill that fails means it
                // is already gone or beyond us, and a blocking `wait` on it
                // would be the unbounded wait this exists to avoid.
                if child.kill().is_ok() {
                    let _ = child.wait();
                } else {
                    let _ = child.try_wait();
                }
                return ChildExit::TimedOut;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(e) => return ChildExit::Failed(e),
        }
    }
}

#[cfg(target_os = "macos")]
fn kill_authorization() {
    let pid = AUTH_PID.load(std::sync::atomic::Ordering::Acquire);
    if pid == 0 {
        return;
    }
    match std::process::Command::new("/bin/kill")
        .args(["-TERM", &pid.to_string()])
        .status()
    {
        Ok(status) if status.success() => {}
        Ok(status) => {
            tracing::warn!(code = ?status.code(), pid, "failed to cancel administrator authorization")
        }
        Err(error) => {
            tracing::warn!(%error, pid, "failed to invoke kill for administrator authorization")
        }
    }
}

/// Read the kernel `SleepDisabled` flag from `pmset -g` (no privileges needed).
/// `None` if `pmset` could not be run — treated as "unknown", never as "off".
#[cfg(target_os = "macos")]
fn read_sleep_disabled() -> Option<bool> {
    let output = output_within(
        std::process::Command::new("/usr/bin/pmset").arg("-g"),
        READ_DEADLINE,
    )?;
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

fn read_power_status() -> (PowerSource, Option<u8>) {
    #[cfg(target_os = "macos")]
    {
        let output = match output_within(
            std::process::Command::new("/usr/bin/pmset").args(["-g", "batt"]),
            READ_DEADLINE,
        ) {
            Some(output) if output.status.success() => output,
            _ => return (PowerSource::Unknown, None),
        };
        parse_power_status(&String::from_utf8_lossy(&output.stdout))
    }
    #[cfg(not(target_os = "macos"))]
    {
        (PowerSource::Unknown, None)
    }
}

fn parse_power_status(text: &str) -> (PowerSource, Option<u8>) {
    let source = if text.contains("'AC Power'") {
        PowerSource::Ac
    } else if text.contains("'Battery Power'") {
        PowerSource::Battery
    } else {
        PowerSource::Unknown
    };
    let percent = text.lines().find_map(|line| {
        let marker = line.find('%')?;
        let digits = line[..marker]
            .chars()
            .rev()
            .take_while(char::is_ascii_digit)
            .collect::<String>();
        digits.chars().rev().collect::<String>().parse::<u8>().ok()
    });
    (source, percent)
}

fn detect_long_running_processes() -> Vec<LongRunningProcess> {
    let output = match output_within(
        std::process::Command::new("/bin/ps").args(["-axo", "pid=,etime=,comm="]),
        READ_DEADLINE,
    ) {
        Some(output) if output.status.success() => output,
        _ => return Vec::new(),
    };
    parse_long_running_processes(&String::from_utf8_lossy(&output.stdout))
}

fn parse_long_running_processes(text: &str) -> Vec<LongRunningProcess> {
    const JOB_NAMES: &[&str] = &[
        "bun",
        "cargo",
        "claude",
        "cmake",
        "codex",
        "deno",
        "go",
        "make",
        "ninja",
        "node",
        "npm",
        "pnpm",
        "python",
        "python3",
        "pytest",
        "rustc",
        "swift",
        "xcodebuild",
        "yarn",
    ];
    let own_pid = std::process::id();
    let mut processes = text
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse::<u32>().ok()?;
            let elapsed_secs = parse_elapsed(fields.next()?)?;
            let command = fields.next()?;
            let name = std::path::Path::new(command).file_name()?.to_str()?;
            (pid != own_pid
                && elapsed_secs >= LONG_RUNNING_PROCESS_SECS
                && JOB_NAMES.contains(&name))
            .then(|| LongRunningProcess {
                pid,
                name: name.to_owned(),
                elapsed_secs,
            })
        })
        .collect::<Vec<_>>();
    processes.sort_by_key(|process| std::cmp::Reverse(process.elapsed_secs));
    processes.truncate(5);
    processes
}

fn parse_elapsed(value: &str) -> Option<u64> {
    let (days, clock) = match value.split_once('-') {
        Some((days, clock)) => (days.parse::<u64>().ok()?, clock),
        None => (0, value),
    };
    let parts = clock
        .split(':')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let seconds = match parts.as_slice() {
        [minutes, seconds] => minutes * 60 + seconds,
        [hours, minutes, seconds] => hours * 3_600 + minutes * 60 + seconds,
        _ => return None,
    };
    Some(days * 86_400 + seconds)
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
    fn power_status_reports_source_and_percentage() {
        assert_eq!(
            parse_power_status(
                "Now drawing from 'AC Power'\n -InternalBattery-0 (id=1)\t82%; charging;"
            ),
            (PowerSource::Ac, Some(82))
        );
        assert_eq!(
            parse_power_status(
                "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=1)\t19%; discharging;"
            ),
            (PowerSource::Battery, Some(19))
        );
    }

    #[test]
    fn long_running_process_detection_filters_short_and_unrelated_processes() {
        let processes = parse_long_running_processes(
            " 101 04:59 /usr/bin/cargo\n 102 05:00 /opt/homebrew/bin/codex\n 103 1-02:03:04 /usr/bin/python3\n 104 12:00 /usr/bin/Finder\n",
        );
        assert_eq!(
            processes,
            vec![
                LongRunningProcess {
                    pid: 103,
                    name: "python3".into(),
                    elapsed_secs: 93_784,
                },
                LongRunningProcess {
                    pid: 102,
                    name: "codex".into(),
                    elapsed_secs: 300,
                },
            ]
        );
    }

    #[test]
    fn safety_guards_prioritize_deadline_then_power_then_battery() {
        let options = KeepAwakeOptions {
            duration_secs: None,
            ends_at_ms: Some(1_000),
            ac_only: true,
            low_battery_action: LowBatteryAction::TurnOff,
        };
        assert_eq!(
            safety_decision(true, false, options, PowerSource::Battery, Some(5), 1_000),
            SafetyDecision::TurnOff(KeepAwakeNotice::TimerElapsed)
        );

        let options = KeepAwakeOptions {
            ends_at_ms: None,
            ..options
        };
        assert_eq!(
            safety_decision(true, false, options, PowerSource::Battery, Some(5), 0),
            SafetyDecision::TurnOff(KeepAwakeNotice::AcDisconnected)
        );

        let options = KeepAwakeOptions {
            ac_only: false,
            low_battery_action: LowBatteryAction::Warn,
            ..options
        };
        assert_eq!(
            safety_decision(true, false, options, PowerSource::Battery, Some(20), 0),
            SafetyDecision::WarnLowBattery
        );
    }

    #[test]
    fn safety_guards_do_not_interrupt_an_authorization_transition() {
        let options = KeepAwakeOptions {
            duration_secs: None,
            ends_at_ms: Some(0),
            ac_only: true,
            low_battery_action: LowBatteryAction::TurnOff,
        };
        assert_eq!(
            safety_decision(true, true, options, PowerSource::Battery, Some(1), 1),
            SafetyDecision::None
        );
    }

    #[test]
    fn guards_pause_for_a_prompt_and_for_one_automatic_turn_off_only() {
        assert!(guards_blocked(KeepAwakePhase::Enabling, false));
        assert!(guards_blocked(KeepAwakePhase::Disabling, false));
        assert!(!guards_blocked(KeepAwakePhase::On, false));
        // The clear needs an administrator prompt that can be declined, so a
        // guard fires once per session — otherwise a decline would reopen that
        // dialog on every monitor tick.
        assert!(guards_blocked(KeepAwakePhase::Failed, true));
        assert!(guards_blocked(KeepAwakePhase::On, true));
        // A session left running by a *declined manual off* sits in `Failed`
        // with sleep still prevented; its guards must stay armed.
        assert!(!guards_blocked(KeepAwakePhase::Failed, false));
    }

    #[test]
    fn a_spent_end_time_is_stale_but_a_relative_preset_never_is() {
        let at_time = KeepAwakeOptions {
            duration_secs: None,
            ends_at_ms: Some(1_000),
            ac_only: false,
            low_battery_action: LowBatteryAction::Warn,
        };
        // Already reached: engaging would be torn down on the next monitor tick,
        // so the deadline is dropped rather than the switch refusing — a refusal
        // would leave the tray item and the shortcut with no way to recover.
        assert!(deadline_is_stale(at_time, 1_000));
        assert!(!deadline_is_stale(at_time, 999));
        // A relative preset is re-armed by `arm_duration` on every engage, so a
        // leftover materialized deadline from an earlier session is not stale.
        assert!(!deadline_is_stale(
            KeepAwakeOptions {
                duration_secs: Some(1_800),
                ..at_time
            },
            u64::MAX
        ));
    }

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
    fn reconcile_leftover_override_asks_the_user_instead_of_clearing() {
        // Unclean exit: our marker and the override both remain. The marker
        // says we *may* have set it; the flag cannot say whether it is still
        // ours, so it is not cleared on that evidence — the user decides.
        assert_eq!(
            reconcile_decision(true, Some(true)),
            ReconcileAction::AskUser
        );
    }

    #[test]
    fn writeback_superseded_changes_nothing() {
        // Cancellation bumped the generation while our admin-auth dialog was up:
        // commit nothing, whatever the direction/outcome — the newer cycle's
        // worker owns the final state, so a stale cancel can't clobber it.
        assert_eq!(
            reconcile_writeback(
                true,
                true,
                false,
                LidCloseState::Unavailable,
                Ownership::Unowned
            ),
            ReconcileWriteback::Superseded
        );
        assert_eq!(
            reconcile_writeback(true, false, true, LidCloseState::Off, Ownership::Confirmed),
            ReconcileWriteback::Superseded
        );
    }

    #[test]
    fn writeback_turn_on_requires_the_veto() {
        // Turning on commits only if the veto engaged; a declined / unreadable
        // veto rolls the whole (mandatory-veto) switch back off.
        assert_eq!(
            reconcile_writeback(
                false,
                true,
                false,
                LidCloseState::Engaged,
                Ownership::Confirmed
            ),
            ReconcileWriteback::On
        );
        assert_eq!(
            reconcile_writeback(
                false,
                true,
                false,
                LidCloseState::Unavailable,
                Ownership::Unowned
            ),
            ReconcileWriteback::EnableFailed
        );
    }

    #[test]
    fn writeback_turn_off_stays_on_when_clear_declined() {
        // Turning off but the override clear was declined: we still own it, so
        // sleep is still prevented — keep keep-awake on rather than report off.
        assert_eq!(
            reconcile_writeback(false, false, true, LidCloseState::Off, Ownership::Confirmed),
            ReconcileWriteback::DisableFailed
        );
    }

    #[test]
    fn writeback_turn_off_commits_when_cleared() {
        // Turning off and nothing left we own (cleared, or never ours): commit off.
        assert_eq!(
            reconcile_writeback(false, false, true, LidCloseState::Off, Ownership::Unowned),
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

    /// How a `set_disablesleep` call affects the fake kernel flag — decoupled
    /// from what the call *reports*, so tests can model `pmset` applying the
    /// change while `osascript` still reports failure (and the reverse).
    #[derive(Debug, Clone, Copy)]
    enum PmsetEffect {
        /// The kernel flag becomes the requested value.
        Apply,
        /// The kernel flag is left unchanged (a `pmset` that did nothing).
        NoChange,
        /// The kernel flag becomes unreadable afterwards (`read` returns `None`).
        BecomeUnreadable,
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
        /// What `set_disablesleep` *reports* (false simulates a declined auth
        /// prompt or an `osascript` that exited non-zero).
        pmset_reports: bool,
        /// How `set_disablesleep` mutates the fake kernel flag, independent of
        /// what it reports.
        pmset_effect: PmsetEffect,
        /// Whether the failsafe marker is currently on disk.
        marker: std::cell::Cell<bool>,
        /// Every side-effecting call, in order.
        ops: std::cell::RefCell<Vec<Op>>,
        /// Reads that fail (return `None`) before the flag becomes readable
        /// again, once a `pmset` has run. `None`: reads never fail this way.
        read_failures_left: std::cell::Cell<Option<usize>>,
        /// Whether a `set_disablesleep(false)` ends the read failures.
        reads_recover_on_clear: bool,
        /// Whether any `pmset` has run yet (arms the read failures above).
        pmset_ran: std::cell::Cell<bool>,
    }

    impl FakeSys {
        /// A system whose `SleepDisabled` flag currently reads as `sleep_disabled`.
        fn new(sleep_disabled: Option<bool>) -> Self {
            Self {
                sleep_disabled: std::cell::Cell::new(sleep_disabled),
                marker_write_ok: true,
                pmset_reports: true,
                pmset_effect: PmsetEffect::Apply,
                marker: std::cell::Cell::new(false),
                ops: std::cell::RefCell::new(Vec::new()),
                read_failures_left: std::cell::Cell::new(None),
                reads_recover_on_clear: false,
                pmset_ran: std::cell::Cell::new(false),
            }
        }
        /// Simulate a directory the marker cannot be written to.
        fn marker_write_fails(mut self) -> Self {
            self.marker_write_ok = false;
            self
        }
        /// Simulate a declined admin prompt / failed `pmset`: reports failure and
        /// changes nothing.
        fn pmset_fails(mut self) -> Self {
            self.pmset_reports = false;
            self.pmset_effect = PmsetEffect::NoChange;
            self
        }
        /// Simulate `pmset` applying the change while `osascript` reports failure
        /// — the case that must not drop the failsafe marker.
        fn pmset_applies_despite_reporting_failure(mut self) -> Self {
            self.pmset_reports = false;
            self.pmset_effect = PmsetEffect::Apply;
            self
        }
        /// Simulate a `pmset` that reports success but does not take effect (the
        /// kernel flag is unchanged) — the clear-side analog.
        fn pmset_reports_success_without_effect(mut self) -> Self {
            self.pmset_reports = true;
            self.pmset_effect = PmsetEffect::NoChange;
            self
        }
        /// Simulate the sleep state becoming unreadable after the `pmset` call.
        fn pmset_leaves_state_unreadable(mut self) -> Self {
            self.pmset_effect = PmsetEffect::BecomeUnreadable;
            self
        }
        /// Simulate one transient read failure after the `pmset` call; the flag
        /// itself is applied and readable again afterwards.
        fn readback_fails_once(self) -> Self {
            self.read_failures_left.set(Some(1));
            self
        }
        /// Simulate reads failing after an enable until a clear has run — the
        /// compensating clear is what makes the state readable again.
        fn readback_fails_until_clear(mut self) -> Self {
            self.read_failures_left.set(Some(usize::MAX));
            self.reads_recover_on_clear = true;
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
            // Read failures are armed by the first `pmset` call (they model the
            // read-back), so the pre-check read is unaffected.
            if self.pmset_ran.get()
                && let Some(left) = self.read_failures_left.get()
                && left > 0
            {
                self.read_failures_left.set(Some(left - 1));
                return None;
            }
            self.sleep_disabled.get()
        }
        fn set_disablesleep(&self, on: bool) -> bool {
            self.ops.borrow_mut().push(Op::SetDisablesleep(on));
            self.pmset_ran.set(true);
            if !on && self.reads_recover_on_clear {
                self.read_failures_left.set(None);
            }
            match self.pmset_effect {
                PmsetEffect::Apply => self.sleep_disabled.set(Some(on)),
                PmsetEffect::NoChange => {}
                PmsetEffect::BecomeUnreadable => self.sleep_disabled.set(None),
            }
            self.pmset_reports
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
    fn wait_child_reaps_a_child_that_outlives_its_deadline() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let started = std::time::Instant::now();
        let exit = wait_child(&mut child, Some(std::time::Duration::from_millis(200)));
        assert!(matches!(exit, ChildExit::TimedOut), "{exit:?}");
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        // Killed and reaped: a further wait finds it already gone.
        assert!(child.try_wait().unwrap().is_some());
    }

    #[test]
    fn wait_child_returns_the_status_of_a_child_that_exits_in_time() {
        let mut child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        let exit = wait_child(&mut child, Some(std::time::Duration::from_secs(5)));
        assert!(
            matches!(exit, ChildExit::Exited(status) if status.success()),
            "{exit:?}"
        );
        let mut child = std::process::Command::new("/usr/bin/false")
            .spawn()
            .unwrap();
        let exit = wait_child(&mut child, None);
        assert!(
            matches!(exit, ChildExit::Exited(status) if !status.success()),
            "{exit:?}"
        );
    }

    #[test]
    fn engage_from_clean_state_takes_ownership_and_marks() {
        // Nothing was vetoing sleep; we engage, own the override, and the marker
        // must guard it (written before the `pmset`).
        let sys = FakeSys::new(Some(false));
        let out = reconcile_lid_close_with(&sys, true, Ownership::Unowned);
        assert_eq!(
            out,
            LidCloseOutcome {
                lid_close: LidCloseState::Engaged,
                ownership: Ownership::Confirmed,
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
        let out = reconcile_lid_close_with(&sys, true, Ownership::Unowned);
        assert_eq!(
            out,
            LidCloseOutcome {
                lid_close: LidCloseState::Unavailable,
                ownership: Ownership::Unowned,
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
        let out = reconcile_lid_close_with(&sys, true, Ownership::Unowned);
        assert_eq!(
            out,
            LidCloseOutcome {
                lid_close: LidCloseState::Unavailable,
                ownership: Ownership::Unowned,
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
        let out = reconcile_lid_close_with(&sys, true, Ownership::Unowned);
        assert_eq!(
            out,
            LidCloseOutcome {
                lid_close: LidCloseState::Engaged,
                ownership: Ownership::Unowned,
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
        let out = reconcile_lid_close_with(&sys, true, Ownership::Unowned);
        assert_eq!(
            out,
            LidCloseOutcome {
                lid_close: LidCloseState::Unavailable,
                ownership: Ownership::Unowned,
            }
        );
        assert!(sys.pmset_calls().is_empty());
    }

    #[test]
    fn disable_clears_an_override_we_own() {
        // Normal toggle off: clear our override and drop the marker.
        let sys = FakeSys::new(Some(true)).with_marker();
        let out = reconcile_lid_close_with(&sys, false, Ownership::Confirmed);
        assert_eq!(
            out,
            LidCloseOutcome {
                lid_close: LidCloseState::Off,
                ownership: Ownership::Unowned,
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
        let out = reconcile_lid_close_with(&sys, false, Ownership::Unowned);
        assert_eq!(
            out,
            LidCloseOutcome {
                lid_close: LidCloseState::Off,
                ownership: Ownership::Unowned,
            }
        );
        assert!(sys.pmset_calls().is_empty());
    }

    #[test]
    fn disable_keeps_ownership_when_clear_fails() {
        // Auth declined on the way down: stay owner and keep the marker so launch
        // reconcile / cleanup retries rather than leaking the override.
        let sys = FakeSys::new(Some(true)).with_marker().pmset_fails();
        let out = reconcile_lid_close_with(&sys, false, Ownership::Confirmed);
        assert_eq!(
            out,
            LidCloseOutcome {
                lid_close: LidCloseState::Off,
                ownership: Ownership::Confirmed,
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
        let still_own = cleanup_lid_close_with(&sys, Ownership::Confirmed);
        assert_eq!(still_own, Ownership::Unowned);
        assert_eq!(sys.pmset_calls(), vec![false]);
        assert!(!sys.marker_present());
    }

    #[test]
    fn cleanup_clears_via_failsafe_marker_even_without_ownership() {
        // An engage that crashed after writing the marker but before recording
        // ownership: the marker alone triggers a failsafe clear at exit.
        let sys = FakeSys::new(Some(true)).with_marker();
        let still_own = cleanup_lid_close_with(&sys, Ownership::Unowned);
        assert_eq!(still_own, Ownership::Unowned);
        assert_eq!(sys.pmset_calls(), vec![false]);
        assert!(!sys.marker_present());
    }

    #[test]
    fn cleanup_leaves_a_foreign_override_untouched() {
        // `disablesleep` is on but isn't ours and there's no marker: never clear
        // an override Tomari didn't set, even on quit / logout.
        let sys = FakeSys::new(Some(true));
        let still_own = cleanup_lid_close_with(&sys, Ownership::Unowned);
        assert_eq!(still_own, Ownership::Unowned);
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
        let still_own = cleanup_lid_close_with(&sys, Ownership::Confirmed);
        assert_eq!(
            still_own,
            Ownership::Confirmed,
            "ownership must survive a failed cleanup"
        );
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
        let still_own = cleanup_lid_close_with(&sys, Ownership::Unowned);
        assert_eq!(still_own, Ownership::Unowned);
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

    #[test]
    fn enable_that_applied_but_reported_failure_keeps_the_marker_and_ownership() {
        // `pmset` set `SleepDisabled` but `osascript` reported failure. Trusting
        // the reported result would drop the marker and roll back while the Mac
        // stays unable to sleep with no record — the reliability bug. Confirming
        // against the kernel flag must instead own it and keep the marker.
        let sys = FakeSys::new(Some(false)).pmset_applies_despite_reporting_failure();
        let out = reconcile_lid_close_with(&sys, true, Ownership::Unowned);
        assert_eq!(
            out,
            LidCloseOutcome {
                lid_close: LidCloseState::Engaged,
                ownership: Ownership::Confirmed,
            }
        );
        assert!(
            sys.marker_present(),
            "an override that actually took must stay guarded by its marker"
        );
        assert_eq!(sys.pmset_calls(), vec![true]);
    }

    #[test]
    fn enable_with_unreadable_state_afterwards_compensates_and_reports_possible_ownership() {
        // The state could not be read back after enabling, twice. The worker
        // must not settle for "off" with the Mac possibly unable to sleep: it
        // clears the override again in the same cycle. Here that clear cannot be
        // confirmed either, so the marker stays as the failsafe and the override
        // is recorded as *possibly* ours — never written off as foreign — for
        // off, exit and the next reconcile to finish, and for the UI to report.
        let sys = FakeSys::new(Some(false)).pmset_leaves_state_unreadable();
        let out = reconcile_lid_close_with(&sys, true, Ownership::Unowned);
        assert_eq!(
            out,
            LidCloseOutcome {
                lid_close: LidCloseState::Unavailable,
                ownership: Ownership::PossiblyOwned,
            }
        );
        assert!(
            sys.marker_present(),
            "an unverifiable enable must keep the failsafe marker"
        );
        assert_eq!(
            sys.pmset_calls(),
            vec![true, false],
            "the enable is compensated in the same cycle"
        );
        assert_eq!(
            reconcile_writeback(false, true, false, out.lid_close, out.ownership),
            ReconcileWriteback::EnableUnconfirmed
        );
    }

    #[test]
    fn enable_whose_readback_recovers_on_the_second_read_is_confirmed() {
        // A single transient `pmset -g` failure must not trigger the compensating
        // clear (and its admin prompt): the second read settles it.
        let sys = FakeSys::new(Some(false)).readback_fails_once();
        let out = reconcile_lid_close_with(&sys, true, Ownership::Unowned);
        assert_eq!(
            out,
            LidCloseOutcome {
                lid_close: LidCloseState::Engaged,
                ownership: Ownership::Confirmed,
            }
        );
        assert_eq!(sys.pmset_calls(), vec![true]);
        assert!(sys.marker_present());
    }

    #[test]
    fn enable_whose_compensating_clear_is_confirmed_ends_clean() {
        // Unreadable after the enable, but the compensating clear reads back
        // clear: nothing is left set, so no marker and no ownership remain, and
        // the switch rolls back as a plain failure.
        let sys = FakeSys::new(Some(false)).readback_fails_until_clear();
        let out = reconcile_lid_close_with(&sys, true, Ownership::Unowned);
        assert_eq!(
            out,
            LidCloseOutcome {
                lid_close: LidCloseState::Unavailable,
                ownership: Ownership::Unowned,
            }
        );
        assert_eq!(sys.pmset_calls(), vec![true, false]);
        assert!(!sys.marker_present());
        assert_eq!(
            reconcile_writeback(false, true, false, out.lid_close, out.ownership),
            ReconcileWriteback::EnableFailed
        );
    }

    #[test]
    fn a_possibly_owned_override_read_back_set_stays_possibly_ours() {
        // The next engage finds the override set. It is not treated as someone
        // else's — it stays ours to clear — but reading it set does not prove
        // our enable took either, so the claim is not upgraded.
        let sys = FakeSys::new(Some(true)).with_marker();
        let out = reconcile_lid_close_with(&sys, true, Ownership::PossiblyOwned);
        assert_eq!(
            out,
            LidCloseOutcome {
                lid_close: LidCloseState::Engaged,
                ownership: Ownership::PossiblyOwned,
            }
        );
        assert!(sys.pmset_calls().is_empty());
    }

    #[test]
    fn off_clears_a_possibly_owned_override() {
        // The unresolved state is recovered by the ordinary off path: possibly
        // ours is ours to clear, and a confirmed clear drops marker and claim.
        let sys = FakeSys::new(Some(true)).with_marker();
        let out = reconcile_lid_close_with(&sys, false, Ownership::PossiblyOwned);
        assert_eq!(
            out,
            LidCloseOutcome {
                lid_close: LidCloseState::Off,
                ownership: Ownership::Unowned,
            }
        );
        assert_eq!(sys.pmset_calls(), vec![false]);
        assert!(!sys.marker_present());
    }

    #[test]
    fn a_possibly_owned_override_is_never_reported_off_and_clean() {
        // Enable could not be confirmed either way → the writeback must say so,
        // and turning off while it is still possibly ours must not read as a
        // clean off while the clear is unconfirmed.
        assert_eq!(
            reconcile_writeback(
                false,
                true,
                false,
                LidCloseState::Unavailable,
                Ownership::PossiblyOwned
            ),
            ReconcileWriteback::EnableUnconfirmed
        );
        // From on, a declined clear keeps keep-awake on; from off — the
        // recovery of a possibly-owned override — there is no on to keep, so it
        // stays off with the notice and retry intact.
        assert_eq!(
            reconcile_writeback(
                false,
                false,
                true,
                LidCloseState::Off,
                Ownership::PossiblyOwned
            ),
            ReconcileWriteback::DisableFailed
        );
        assert_eq!(
            reconcile_writeback(
                false,
                false,
                false,
                LidCloseState::Off,
                Ownership::PossiblyOwned
            ),
            ReconcileWriteback::RecoveryFailed
        );
        assert_eq!(
            reconcile_writeback(false, false, false, LidCloseState::Off, Ownership::Unowned),
            ReconcileWriteback::Off
        );
    }

    #[test]
    fn disable_that_reported_success_without_effect_keeps_the_marker_and_ownership() {
        // The clear reported success but `SleepDisabled` is still set. Trusting
        // the report would drop the marker and release ownership while the Mac
        // stays awake with no record. Confirming against the kernel flag must
        // keep both so launch reconcile / cleanup retries.
        let sys = FakeSys::new(Some(true))
            .with_marker()
            .pmset_reports_success_without_effect();
        let out = reconcile_lid_close_with(&sys, false, Ownership::Confirmed);
        assert_eq!(
            out,
            LidCloseOutcome {
                lid_close: LidCloseState::Off,
                ownership: Ownership::Confirmed,
            }
        );
        assert_eq!(sys.pmset_calls(), vec![false]);
        assert!(
            sys.marker_present(),
            "an override that is still set must keep its marker"
        );
    }

    #[test]
    fn disable_with_unreadable_state_afterwards_keeps_the_marker_and_ownership() {
        // The state could not be read back after the clear: keep the marker and
        // ownership rather than risk dropping the failsafe over a live override.
        let sys = FakeSys::new(Some(true))
            .with_marker()
            .pmset_leaves_state_unreadable();
        let out = reconcile_lid_close_with(&sys, false, Ownership::Confirmed);
        assert_eq!(
            out,
            LidCloseOutcome {
                lid_close: LidCloseState::Off,
                ownership: Ownership::Confirmed,
            }
        );
        assert_eq!(sys.pmset_calls(), vec![false]);
        assert!(sys.marker_present());
    }

    #[test]
    fn cleanup_keeps_marker_when_clear_reports_success_without_effect() {
        // Exit-time clear reported success but the override is still set: the
        // marker must survive so the next launch's reconcile clears it rather
        // than the Mac being stranded awake with no record.
        let sys = FakeSys::new(Some(true))
            .with_marker()
            .pmset_reports_success_without_effect();
        let still_own = cleanup_lid_close_with(&sys, Ownership::Confirmed);
        assert_eq!(
            still_own,
            Ownership::Confirmed,
            "ownership must survive an unconfirmed clear"
        );
        assert_eq!(sys.pmset_calls(), vec![false]);
        assert!(
            sys.marker_present(),
            "the marker must survive an unconfirmed clear"
        );
    }
}
