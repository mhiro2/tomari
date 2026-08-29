//! The persistent CGEventTap that connects real keyboard activity to the pure
//! [`ModifierEngine`](tomari_keyboard::ModifierEngine).
//!
//! A dedicated thread owns the tap and runs a `CFRunLoop`, since
//! `CFRunLoopRun` blocks. The callback observes keyboard events plus pointer
//! button, drag, and scroll events, feeds the engines, and:
//!
//! * **remaps** an ordinary modifier (Control/Option/Command/Shift/fn) by
//!   rewriting its `flagsChanged` flags and keycode in place; while it is held,
//!   its target modifier is also stamped onto keyboard and pointer operations
//!   performed through it so a chord lands as the target (e.g.
//!   Control→Command + C registers as Cmd+C, and Control→Command + click
//!   registers as Cmd-click);
//! * handles **Caps Lock** specially: macOS gives it no usable key-up and lets
//!   it lock, so it is first remapped to F18 at the HID level
//!   ([`crate::capsmap`]) and arrives here as F18 key-down/up, which the tap
//!   drives as the Caps Lock modifier (dropping the F18 event). Tapped it fires
//!   its action (e.g. Esc); held it stamps its target (e.g. Control) onto
//!   following keyboard and pointer events;
//! * stamps the **hyper** combo (⌃⌥⇧⌘) onto keyboard and pointer events while a
//!   hyper key is held;
//! * dispatches a modifier's **tap** action (IME switch, snap, …) on release.
//!
//! Creating the tap requires the *Input Monitoring* permission; if it is not
//! granted the OS returns a null tap (and adds Tomari to the Input Monitoring
//! list so the user can enable it).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use core_graphics::event::{
    CGEvent, CGEventField, CGEventTapOptions, CGEventTapPlacement, CGEventTapProxy, CGEventType,
    CallbackResult, EventField,
};
use tauri::{AppHandle, Manager};
use tomari_core::{AppAction, KeySide, ModifierKey};
use tomari_keyboard::HYPER_MODIFIERS;
use tomari_keyboard::engine::KeyEvent;

use crate::keycodes;
use crate::locks::MutexExt;
use crate::state::AppState;
use crate::tap::{self, RunningTap, TapHealth, TapHealthCell};

/// Marker written into `EVENT_SOURCE_USER_DATA` on input Tomari synthesizes,
/// so its keyboard and pointer gesture taps ignore their own injected events.
pub const SYNTHETIC_MARKER: i64 = 0x746f_6d72; // "tomr"

pub fn is_synthetic(event: &CGEvent) -> bool {
    event.get_integer_value_field(EventField::EVENT_SOURCE_USER_DATA) == SYNTHETIC_MARKER
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
}

/// Whether the *Input Monitoring* permission has been granted.
pub fn input_monitoring_granted() -> bool {
    unsafe { CGPreflightListenEventAccess() }
}

/// Prompt for the *Input Monitoring* permission, opening System Settings the
/// first time. Returns whether it is already granted.
pub fn request_input_monitoring() -> bool {
    unsafe { CGRequestListenEventAccess() }
}

/// The single live event tap, owned globally so its lifecycle is independent of
/// the cross-platform [`AppState`] struct.
static EVENT_TAP: Mutex<Option<RunningTap>> = Mutex::new(None);

/// Where the keyboard tap stands (see [`TapHealth`]); logged on every change.
static HEALTH: TapHealthCell = TapHealthCell::new("keyboard");

/// Whether the *Accessibility* permission — which posting key events needs, tap
/// proxy or not — is granted, mirrored so the callback does not call into TCC.
/// Refreshed by [`restart_result`] and by the permission poller in `main`, whose
/// interval bounds how stale it can be: within that window a tap's keystroke may
/// be skipped when it could have been posted, or posted into the void when the
/// grant has just gone. Both beat an OS call from a callback that holds up all
/// input.
static ACCESSIBILITY: AtomicBool = AtomicBool::new(false);

/// Publish the current *Accessibility* grant for the tap callback to read.
pub fn set_accessibility_granted(granted: bool) {
    ACCESSIBILITY.store(granted, Ordering::SeqCst);
}

/// Set when [`reconcile_caps_mapping`] was asked to change the Caps Lock HID
/// remap while Caps Lock was physically held, and so put the change off until
/// the release (see there). The tap callback drains it on that release.
static CAPS_RECONCILE_DEFERRED: AtomicBool = AtomicBool::new(false);

/// (Re)start the tap to match the current settings: tears down any existing tap
/// and, if keyboard customization is enabled, starts a fresh one. Safe to call
/// repeatedly (e.g. when the feature is toggled). Callers that do not need the
/// outcome (permission polling, wake/session reset) use this; [`commands`]
/// uses [`restart_result`] to surface a failure as an `apply_warnings` entry.
///
/// [`commands`]: crate::commands
pub fn restart(app: &AppHandle) {
    let _ = restart_result(app);
}

/// Same as [`restart`], but reports whether the tap ended up matching the
/// setting: `true` when the feature is off (nothing to start) or the tap
/// started successfully, `false` when it is on but failed to start (typically
/// a missing Input Monitoring grant).
///
/// The Caps Lock HID remap is reconciled alongside the tap — it must never
/// outlive a tap that handles F18 — but its outcome is deliberately *not*
/// part of the return value: `save_settings` checks the remap once, after
/// every side effect has run, via [`reconcile_caps_mapping`], so only the
/// final live state (not an intermediate reconcile here) decides whether a
/// `capsLockRemap` warning is raised.
pub fn restart_result(app: &AppHandle) -> bool {
    let mut guard = EVENT_TAP.lock_safe();
    // Published before the old tap goes down, so no reader sees `Healthy` over
    // a tap being torn down; also retires the old callback's generation.
    HEALTH.begin_start();
    *guard = None; // Drop stops the previous tap.
    // The engine's hold state is dropped below and the remap reconciled right
    // here, so a reconcile put off for a Caps Lock hold has nothing to wait for.
    CAPS_RECONCILE_DEFERRED.store(false, Ordering::SeqCst);

    let Some(state) = app.try_state::<AppState>() else {
        return true;
    };
    // The torn-down tap loses the release of any key held across the restart,
    // so drop the engine's transient "key is held" state. Otherwise a held
    // modifier would linger in `held` and the next solo tap would be misread as
    // a chord. Any target modifier the old tap had pressed downstream for a
    // held remapped key is released first — the physical release that follows
    // will no longer be rewritten by it.
    let accessibility = state.windows.permission_granted();
    set_accessibility_granted(accessibility);
    release_held_remaps(&state, accessibility);

    // Reconcile the Caps Lock HID remap toward `manage`. `capsmap` publishes the
    // *actual* resulting state (not the request) to its proxy flag, so a failed
    // `hidutil` cannot leave the flag out of step with the real mapping — which
    // would route F18, or a stuck Caps, wrongly.
    let reconcile_caps = |manage: bool| {
        let _ = crate::capsmap::reconcile(manage);
    };

    if !state.keyboard_enabled() {
        HEALTH.set(TapHealth::Stopped);
        // Feature off: take the Caps Lock HID remap down along with the tap.
        reconcile_caps(false);
        return true;
    }

    match start(app.clone()) {
        Ok(tap) => {
            *guard = Some(tap);
            // `Healthy` only once the handle is in place, so the state never
            // says "running" ahead of it.
            HEALTH.set(TapHealth::Healthy);
            tracing::info!("keyboard event tap started");
            // Remap Caps Lock to F18 only now the tap that handles F18 is live.
            let manage = state.engine.lock_safe().has_caps_lock_rule();
            reconcile_caps(manage);
            true
        }
        Err(e) => {
            HEALTH.record_start_failure(input_monitoring_granted());
            tracing::warn!(error = %e, "keyboard event tap not started (grant Input Monitoring?)");
            // No tap to handle F18 — keep Caps Lock native.
            reconcile_caps(false);
            false
        }
    }
}

/// Stop the tap for good — quit, and the updater's relaunch, which does not
/// guarantee an `ExitRequested` — releasing whatever it still owes downstream.
/// A tap left running into a long teardown would keep stamping stale targets
/// onto the keystrokes typed meanwhile; a tap that simply dies with the process
/// leaves the app holding them.
pub fn teardown(app: &AppHandle) {
    let mut guard = EVENT_TAP.lock_safe();
    // `Stopped` and the retirement of the running callback's generation in
    // one step, so a report the callback still makes while its thread winds
    // down cannot revive the state.
    HEALTH.stop();
    *guard = None;
    // Quit restores the Caps Lock remap itself; nothing is left to defer.
    CAPS_RECONCILE_DEFERRED.store(false, Ordering::SeqCst);
    if let Some(state) = app.try_state::<AppState>() {
        release_held_remaps(&state, state.windows.permission_granted());
    }
}

/// Balance what the tap has pressed downstream on behalf of remapped keys that
/// are still physically held, then forget the hold.
///
/// A remapped modifier's down is rewritten into its target (Control→Command
/// sends the app a Command down). If the tap is torn down before the physical
/// release — a settings save restarting it, the master switch, wake, quit, or
/// the system disabling the tap — the release arrives untouched as a Control up
/// (or not at all), and the app is left believing Command is still down:
/// keystrokes act as Command chords until Command is pressed and released for
/// real. So for every target still owed a release — recorded at each press, so
/// a rule changed mid-hold does not alter it — a `flagsChanged` clearing it is
/// synthesized here, before the engine forgets which keys were held. If the new
/// tap then rewrites the physical release into the same target, the app sees a
/// second release of a key already up, which is inert. A target whose own key
/// is physically down is left alone (its real release will clear it).
///
/// Needs the Accessibility grant like any synthesized event; `accessibility` is
/// passed in rather than queried so the tap callback can use its mirror instead
/// of calling TCC. Without the grant the hold is still forgotten and the
/// imbalance logged — the remap cannot be undone from here.
fn release_held_remaps(state: &AppState, accessibility: bool) {
    let targets = {
        let mut engine = state.engine.lock_safe();
        let targets = engine.held_remap_targets();
        engine.reset();
        targets
    };
    if targets.is_empty() {
        return;
    }
    if !accessibility {
        tracing::warn!(
            ?targets,
            "cannot release remapped modifiers held across a tap teardown: Accessibility \
             permission not granted"
        );
        return;
    }
    for target in targets {
        match crate::keysend::release_modifier(target) {
            Ok(crate::keysend::Release::Posted) => {
                tracing::info!(
                    ?target,
                    "released a remapped modifier held across a tap teardown"
                )
            }
            Ok(crate::keysend::Release::PhysicallyHeld) => {
                tracing::info!(
                    ?target,
                    "remapped modifier's own key is physically held; leaving it"
                )
            }
            Err(e) => {
                tracing::warn!(?target, error = %e, "could not release a remapped modifier held across a tap teardown")
            }
        }
    }
}

/// Whether the keyboard tap is currently running. A cheap state read so
/// `save_settings` can verify on *every* save that an enabled feature actually
/// has its tap alive — a warning must reflect the live state, not just the
/// last restart attempt, or it would vanish from the UI on the next unrelated
/// save while the tap is still dead.
pub fn is_running() -> bool {
    // Read off the health state rather than the handle: a handle only proves
    // a start once succeeded, while the state also knows a tap the system
    // disabled and that is being asked back (still counted as running — it
    // is not a configuration problem the user can act on), and a revoke or
    // failure the restart recorded. The tap lock is taken first so a restart
    // in flight (which holds it from `Starting` through to its outcome) is
    // waited out rather than read as "not running" mid-way.
    let _serialized = EVENT_TAP.lock_safe();
    matches!(
        HEALTH.state(),
        TapHealth::Healthy | TapHealth::DisabledByTimeout
    )
}

/// Reconcile the Caps Lock HID remap (and the F18 proxy flag) with the current
/// rules *without* restarting the tap — after a live rule edit, where the tap
/// reads the engine directly but the HID remap must still be brought into
/// step, and once at the end of every `save_settings`, as the authoritative
/// live check behind the `capsLockRemap` warning. A no-op unless the tap is
/// running, so it never remaps Caps Lock to F18 with no tap to handle it.
///
/// Returns whether the reconcile fully reached what the settings and rules ask
/// for — `false` means a `hidutil` or ownership-record failure left Caps Lock's
/// actual behavior, or the record that governs restoring it, out of step with
/// the saved configuration. Because `capsmap::reconcile` reads the real system
/// state first and retries the transition, calling this on every save both keeps
/// a still-unresolved mismatch warning (it does not silently vanish on an
/// unrelated save) and heals it as soon as the failing step cooperates again.
///
/// While Caps Lock is physically held the HID remap is left as it is and the
/// reconcile is put off until the release. Flipping the remap mid-hold would
/// deliver the release as a different key (a native Caps Lock `flagsChanged`
/// instead of an F18 key-up, or the reverse) or swallow it, leaving the engine
/// holding Caps Lock — and stamping its target onto every keystroke — until the
/// key is pressed again. The deferred case still answers honestly: it reports
/// whether the *current* live mapping already matches what is asked for (and
/// the last reconcile that produced it was clean), so a mismatch left by an
/// earlier `hidutil` failure keeps warning rather than being masked by the
/// hold, and a change genuinely waiting on the release is reported as not yet
/// applied. The release then runs the same reconcile, and the next save
/// re-checks it as usual.
pub fn reconcile_caps_mapping(state: &AppState) -> bool {
    // Hold `EVENT_TAP` for the whole reconcile, as `restart_result` does: the
    // two then serialize, so the "is a tap running" this decides `manage` from
    // cannot go stale under it (a restart stopping the tap, or failing to start
    // it, between the check and the `hidutil` write would otherwise remap Caps
    // Lock to F18 with nothing to handle F18). Lock order is `EVENT_TAP` then
    // engine — the same as `restart_result`, never the reverse; the engine lock
    // is dropped before the reconcile so the tap callback is not held up by it.
    let tap_guard = EVENT_TAP.lock_safe();
    let tap_running = tap_guard.is_some();
    let (manage, deferred) = {
        let engine = state.engine.lock_safe();
        let manage = tap_running && state.keyboard_enabled() && engine.has_caps_lock_rule();
        // The deferred flag is published while the engine lock still proves the
        // key held: the release callback clears the hold and drains the flag
        // after taking the same lock, so it cannot slip between the two and
        // leave a set flag with nobody left to drain it.
        let deferred = engine.is_held(ModifierKey::CapsLock);
        CAPS_RECONCILE_DEFERRED.store(deferred, Ordering::SeqCst);
        (manage, deferred)
    };
    if deferred {
        tracing::info!(
            manage,
            "caps-lock HID remap reconcile deferred until Caps Lock is released"
        );
        return crate::capsmap::matches(manage);
    }
    let reconciled = crate::capsmap::reconcile(manage).reconciled;
    drop(tap_guard);
    reconciled
}

/// Whether the Caps Lock HID remap currently matches what the settings and
/// rules ask for, *without* touching it — the read-only counterpart of
/// [`reconcile_caps_mapping`] for a health check that must not shell out.
/// Same lock order as everywhere else: `EVENT_TAP`, then the engine.
pub fn caps_mapping_in_step(state: &AppState) -> bool {
    // Held through the status read, like `reconcile_caps_mapping` holds it
    // through the write, so a restart landing in between cannot pair a stale
    // "tap running" with the mapping state it just changed.
    let tap_guard = EVENT_TAP.lock_safe();
    let manage = tap_guard.is_some()
        && state.keyboard_enabled()
        && state.engine.lock_safe().has_caps_lock_rule();
    let in_step = crate::capsmap::matches(manage);
    drop(tap_guard);
    in_step
}

/// Run a Caps Lock HID reconcile that [`reconcile_caps_mapping`] put off for a
/// hold, once the hold is over. Called from the tap callback, so the reconcile
/// itself — a `hidutil` round-trip — is moved to its own thread: a child
/// process must never run inside the callback, where it would stall every
/// keystroke and get the tap disabled for timeout. A no-op unless a reconcile
/// is actually pending, so the common release costs one atomic read.
fn run_deferred_caps_reconcile(app: &AppHandle) {
    if !CAPS_RECONCILE_DEFERRED.swap(false, Ordering::SeqCst) {
        return;
    }
    let handle = app.clone();
    let spawned = std::thread::Builder::new()
        .name("tomari-caps-reconcile".into())
        .spawn(move || {
            let Some(state) = handle.try_state::<AppState>() else {
                return;
            };
            if reconcile_caps_mapping(&state) {
                tracing::info!("deferred caps-lock HID remap reconcile applied");
            } else {
                tracing::warn!("deferred caps-lock HID remap reconcile did not match the rules");
            }
        });
    if let Err(e) = spawned {
        // Leave it pending: the next save's authoritative check retries it.
        CAPS_RECONCILE_DEFERRED.store(true, Ordering::SeqCst);
        tracing::warn!(error = %e, "could not spawn the deferred caps-lock HID remap reconcile");
    }
}

/// Mutable state local to the tap thread, reached through a `Mutex` because the
/// CGEventTap callback is `Fn`, not `FnMut`.
#[derive(Default)]
struct TapState {
    /// Whether *any* hyper key is currently held (tracked off the engine's held
    /// set, so holding two and releasing one keeps hyper active).
    hyper_active: bool,
    /// Modifier flags to `(remove, add)` on keystrokes typed while remapped
    /// keys are held, so a chord through them carries the target modifier. Caps
    /// Lock especially needs this: as a lock key it leaves the OS with no
    /// held-modifier state, so the rewritten `flagsChanged` flag is not carried
    /// onto the following keys (see
    /// [`tomari_keyboard::ModifierEngine::held_remap_stamp`]).
    remap_stamp: (Vec<ModifierKey>, Vec<ModifierKey>),
}

const EVENT_TYPES: [CGEventType; 13] = [
    CGEventType::KeyDown,
    CGEventType::KeyUp,
    CGEventType::FlagsChanged,
    CGEventType::LeftMouseDown,
    CGEventType::LeftMouseUp,
    CGEventType::LeftMouseDragged,
    CGEventType::RightMouseDown,
    CGEventType::RightMouseUp,
    CGEventType::RightMouseDragged,
    CGEventType::OtherMouseDown,
    CGEventType::OtherMouseUp,
    CGEventType::OtherMouseDragged,
    CGEventType::ScrollWheel,
];

// core-graphics 0.25 omits the third-axis and momentum fields from its
// EventField constants. These are the stable public CGEventField values from
// CGEventTypes.h.
const SCROLL_DELTA_AXIS_3: CGEventField = 13;
const SCROLL_FIXED_POINT_DELTA_AXIS_3: CGEventField = 95;
const SCROLL_POINT_DELTA_AXIS_3: CGEventField = 98;
const SCROLL_MOMENTUM_PHASE: CGEventField = 123;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PointerEventKind {
    Button,
    Drag,
    Scroll,
}

fn pointer_event_kind(etype: CGEventType) -> Option<PointerEventKind> {
    match etype {
        CGEventType::LeftMouseDown
        | CGEventType::LeftMouseUp
        | CGEventType::RightMouseDown
        | CGEventType::RightMouseUp
        | CGEventType::OtherMouseDown
        | CGEventType::OtherMouseUp => Some(PointerEventKind::Button),
        CGEventType::LeftMouseDragged
        | CGEventType::RightMouseDragged
        | CGEventType::OtherMouseDragged => Some(PointerEventKind::Drag),
        CGEventType::ScrollWheel => Some(PointerEventKind::Scroll),
        _ => None,
    }
}

fn pointer_event_interrupts_tap(
    kind: PointerEventKind,
    scroll_has_delta: bool,
    scroll_is_momentum: bool,
) -> bool {
    kind != PointerEventKind::Scroll || (scroll_has_delta && !scroll_is_momentum)
}

fn start(app: AppHandle) -> Result<RunningTap, String> {
    tap::spawn(
        "tomari-eventtap",
        "event tap",
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        EVENT_TYPES.to_vec(),
        move |port_holder| {
            let state = Arc::new(Mutex::new(TapState::default()));
            // The generation this tap is started under; its health reports
            // are dropped once a later start retires it.
            let generation = HEALTH.generation();
            Box::new(move |proxy, etype, event: &CGEvent| {
                handle_event(&app, &state, &port_holder, generation, proxy, etype, event)
            })
        },
    )
}

fn handle_event(
    app: &AppHandle,
    state: &Arc<Mutex<TapState>>,
    port_holder: &Arc<AtomicUsize>,
    generation: u64,
    proxy: CGEventTapProxy,
    etype: CGEventType,
    event: &CGEvent,
) -> CallbackResult {
    // The system disabled the tap (timeout / heavy input): re-enable it. While
    // disabled the tap sees no events, so a key-up released mid-outage is lost —
    // the engine's `held`/`press` and this thread's `TapState` (hyper-held,
    // remap stamp) would otherwise linger, and every later keystroke
    // would carry a stale Hyper combo or remap target while a solo tap misfired
    // as a chord. Drop all transient hold/press state before re-arming, mirroring
    // how the drag taps discard an in-flight gesture on disable. The Caps Lock
    // HID remap (`CAPS_PROXY_ACTIVE`) is left untouched: it is real system state
    // independent of the tap, not a per-hold belief.
    if matches!(
        etype,
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
    ) {
        if let Some(app_state) = app.try_state::<AppState>() {
            // The outage may have swallowed the physical release of a remapped
            // key too: release its target as a restart would. The grant comes
            // from the mirror — no TCC call from the callback.
            release_held_remaps(&app_state, ACCESSIBILITY.load(Ordering::SeqCst));
        }
        // The reset just forgot any Caps Lock hold, so a remap change that was
        // waiting on its release can go ahead now.
        run_deferred_caps_reconcile(app);
        {
            let mut ts = state.lock_safe();
            // Clear the derived stamps: they are recomputed from the engine's
            // held set on the next modifier event, but a keystroke arriving
            // before that must not carry a stale Hyper combo or remap target.
            ts.hyper_active = false;
            ts.remap_stamp = (Vec::new(), Vec::new());
        }
        HEALTH.record_disabled(generation);
        HEALTH.record_reenable(generation, tap::reenable(port_holder));
        return CallbackResult::Keep;
    }

    // Ignore input Tomari itself synthesized.
    if is_synthetic(event) {
        return CallbackResult::Keep;
    }

    let Some(app_state) = app.try_state::<AppState>() else {
        return CallbackResult::Keep;
    };
    let app_state = app_state.inner();

    if !app_state.keyboard_enabled() {
        return CallbackResult::Keep;
    }

    let now = app_state.now_ms();

    match etype {
        CGEventType::FlagsChanged => on_flags_changed(app, app_state, state, proxy, event, now),
        CGEventType::KeyDown => on_key_down(app, app_state, state, proxy, event, now),
        CGEventType::KeyUp => on_key_up(app, app_state, state, proxy, event, now),
        _ => on_pointer_event(app_state, state, etype, event, now),
    }
}

/// Derive whether a `flagsChanged` event is a press (`true`) or release
/// (`false`) from the event's flag bits, not from remembered down/up state.
///
/// A tap restart (settings save, permission grant, new `TapState`) or a tap
/// outage can swallow a transition mid-hold; a toggle would then read the
/// eventual release as a press and stay inverted for that keycode from then on.
/// Reading the flags instead keeps a stale state from inverting the
/// interpretation.
///
/// Left/right modifiers carry an IOKit device-specific bit while physically
/// held; Fn tracks its generic flag bit directly. Caps Lock never reaches this:
/// its AlphaShift bit reflects the lock state, not the key, and macOS delivers
/// one event per press with no release, so there is nothing here to derive a
/// hold from — see [`on_flags_changed`].
fn derive_is_down(keycode: i64, modkey: ModifierKey, flags_bits: u64) -> bool {
    debug_assert_ne!(modkey, ModifierKey::CapsLock);
    if let Some(bit) = keycodes::device_flag_for_keycode(keycode) {
        flags_bits & bit != 0
    } else {
        flags_bits & keycodes::flag_for(modkey).bits() != 0
    }
}

/// Whether a modifier's *native* `flagsChanged` events are fed to the engine as
/// a hold. Everything but Caps Lock: see [`on_flags_changed`] for why the lock
/// key is not, and [`on_key_down`] for how it is driven through F18 instead.
fn tracks_native_modifier(modkey: ModifierKey) -> bool {
    modkey != ModifierKey::CapsLock
}

/// What a native lock key's press feeds the engine instead of a hold: an
/// ordinary key-down, which turns a modifier tap in progress into a chord
/// without leaving any "held" belief behind.
fn native_lock_key_event(now: u64) -> KeyEvent {
    KeyEvent::OtherInput { at_ms: now }
}

/// A managed modifier's down/up transition fed to [`drive_modifier`].
struct ModifierEvent {
    key: ModifierKey,
    side: KeySide,
    is_down: bool,
}

/// Drive a managed modifier's down/up into the engines and apply its side
/// effects: hyper-held tracking, the remap-stamp set applied to later
/// keystrokes, and dispatching a completed tap action.
///
/// Returns the key's held roles `(remap, hyper)` so a caller that passes the
/// originating modifier event through (a real `flagsChanged`) can rewrite it in
/// place. The Caps-Lock-as-F18 path ignores the roles and drops its key event:
/// Caps Lock has no `flagsChanged` to rewrite, so its remap target reaches
/// keystrokes purely through the stamp set this records.
fn drive_modifier(
    app: &AppHandle,
    app_state: &AppState,
    state: &Arc<Mutex<TapState>>,
    proxy: CGEventTapProxy,
    ev: &ModifierEvent,
    now: u64,
) -> (Option<ModifierKey>, bool) {
    let ModifierEvent { key, side, is_down } = *ev;

    // Feed the engine and read back this key's held roles plus the post-event
    // held set (so the stamp tracking reflects this very up/down). The remap and
    // hyper roles are read *before* the event is processed: for a release the
    // engine still holds the roles recorded at the press, which is what the app
    // has been holding and therefore what the release must be rewritten into —
    // even if the rules changed in between. Processing the release forgets them.
    let (tap_action, remap, hyper, any_hyper_held, held_remap_stamp) = {
        let mut engine = app_state.engine.lock_safe();
        let remap = engine.remap_for(key, side);
        let hyper = engine.is_hyper(key, side);
        let action = engine.process(if is_down {
            KeyEvent::ModifierDown {
                key,
                side,
                at_ms: now,
            }
        } else {
            KeyEvent::ModifierUp {
                key,
                side,
                at_ms: now,
            }
        });
        (
            action,
            remap,
            hyper,
            engine.is_any_hyper_held(),
            engine.held_remap_stamp(),
        )
    };

    {
        let mut ts = state.lock_safe();
        // Track whether *any* hyper key is still held, not just this event's
        // direction: releasing one of two held hyper keys must not drop the
        // ⌃⌥⇧⌘ stamp while the other is still down.
        ts.hyper_active = any_hyper_held;
        // Refresh the flags stamped onto later keystrokes from the now-updated
        // set of held remapped keys (this event may have added or removed one).
        ts.remap_stamp = held_remap_stamp;
    }

    if let Some(action) = tap_action {
        dispatch_tap_action(app, proxy, action);
    }

    // A Caps Lock HID remap change held back for this key's hold lands now that
    // the hold is over (see `reconcile_caps_mapping`).
    if key == ModifierKey::CapsLock && !is_down {
        run_deferred_caps_reconcile(app);
    }

    (remap, hyper)
}

fn on_flags_changed(
    app: &AppHandle,
    app_state: &AppState,
    state: &Arc<Mutex<TapState>>,
    proxy: CGEventTapProxy,
    event: &CGEvent,
    now: u64,
) -> CallbackResult {
    let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
    let Some((modkey, side)) = keycodes::modifier_for_keycode(keycode) else {
        return CallbackResult::Keep;
    };

    // A *native* Caps Lock event is passed through untouched and kept out of
    // the engine's hold tracking. It only arrives here while no rule manages
    // Caps Lock (with one, the HID remap turns the key into F18 key-down/up,
    // handled in `on_key_down`/`on_key_up`), and as a lock key it offers nothing
    // to track: macOS sends one `flagsChanged` per press and no release, and
    // the AlphaShift bit is the lock state, not the key. Toggling a "down"
    // belief off each event — as this used to — inverted after a single press,
    // and a release lost to a tap outage or restart kept it inverted, so the
    // engine believed Caps Lock held and read every other modifier's solo tap
    // as a chord (suppressing the ⌘ IME toggle) until the next Caps Lock press.
    // The press still counts as a key pressed *during* another modifier's hold,
    // though: ⌘ down · Caps Lock · ⌘ up is a chord, not a ⌘ tap.
    if !tracks_native_modifier(modkey) {
        app_state
            .engine
            .lock_safe()
            .process(native_lock_key_event(now));
        return CallbackResult::Keep;
    }

    // Derive down/up from the event itself, not remembered state (see
    // `derive_is_down`).
    let is_down = derive_is_down(keycode, modkey, event.get_flags().bits());

    let (remap, hyper) = drive_modifier(
        app,
        app_state,
        state,
        proxy,
        &ModifierEvent {
            key: modkey,
            side,
            is_down,
        },
        now,
    );

    // A hyper key contributes its combo to later keystrokes, not to its own
    // event — strip the source flag so e.g. Caps Lock does not toggle.
    if hyper {
        let mut flags = event.get_flags();
        flags.remove(keycodes::flag_for(modkey));
        event.set_flags(flags);
        return CallbackResult::Keep;
    }

    // Remap: rewrite the event to the target modifier in place.
    if let Some(target) = remap {
        let mut flags = event.get_flags();
        flags.remove(keycodes::flag_for(modkey));
        if is_down {
            flags.insert(keycodes::flag_for(target));
        }
        event.set_flags(flags);
        event.set_integer_value_field(
            EventField::KEYBOARD_EVENT_KEYCODE,
            keycodes::primary_keycode(target),
        );
    }

    CallbackResult::Keep
}

fn on_key_down(
    app: &AppHandle,
    app_state: &AppState,
    state: &Arc<Mutex<TapState>>,
    proxy: CGEventTapProxy,
    event: &CGEvent,
    now: u64,
) -> CallbackResult {
    let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);

    // Caps Lock arrives here, as F18, only because we remapped it at the HID
    // level (see `capsmap`) — that is the only way to get a real key-down/up for
    // it and to stop it locking. Drive it as the Caps Lock modifier and drop the
    // F18 event so no app sees a stray F18. Autorepeat keyDowns during a hold are
    // dropped without re-driving, so they neither re-fire side effects nor reach
    // the app while the matching keyUp is still dropped.
    //
    // Known limitation: while the Caps Lock proxy is active, this keycode is
    // read as Caps Lock unconditionally — a genuine physical F18 press (an
    // external keyboard with a dedicated F18 key, say) would be swallowed and
    // driven as Caps Lock too. There is no way to tell the two apart at this
    // layer: `capsmap`'s HID remap makes them arrive as the exact same keycode
    // with no distinguishing bit. Accepted as a design trade-off — real F18
    // keys are vanishingly rare, and Caps Lock's own delivery leaves no less
    // invasive alternative.
    if keycode == crate::capsmap::F18_KEYCODE && crate::capsmap::caps_proxy_active() {
        // Autorepeat key-downs during a hold neither re-fire side effects nor
        // reach an app; only the initial press drives the modifier down.
        if event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) == 0 {
            drive_modifier(
                app,
                app_state,
                state,
                proxy,
                &ModifierEvent {
                    key: ModifierKey::CapsLock,
                    side: KeySide::Either,
                    is_down: true,
                },
                now,
            );
        }
        return CallbackResult::Drop;
    }

    // Any non-modifier key turns a pending modifier tap into a chord.
    app_state
        .engine
        .lock_safe()
        .process(KeyEvent::OtherInput { at_ms: now });

    stamp_held_modifiers(state, event);
    CallbackResult::Keep
}

fn on_key_up(
    app: &AppHandle,
    app_state: &AppState,
    state: &Arc<Mutex<TapState>>,
    proxy: CGEventTapProxy,
    event: &CGEvent,
    now: u64,
) -> CallbackResult {
    let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);

    // The Caps Lock proxy (F18) release ends the Caps Lock modifier — its quick
    // press/release here is what fires the tap action (e.g. Esc). Drop the event.
    // Same known limitation as `on_key_down`: a real physical F18 is
    // indistinguishable from the proxy while it is active.
    if keycode == crate::capsmap::F18_KEYCODE && crate::capsmap::caps_proxy_active() {
        drive_modifier(
            app,
            app_state,
            state,
            proxy,
            &ModifierEvent {
                key: ModifierKey::CapsLock,
                side: KeySide::Either,
                is_down: false,
            },
            now,
        );
        return CallbackResult::Drop;
    }

    stamp_held_modifiers(state, event);
    CallbackResult::Keep
}

fn on_pointer_event(
    app_state: &AppState,
    state: &Arc<Mutex<TapState>>,
    etype: CGEventType,
    event: &CGEvent,
    now: u64,
) -> CallbackResult {
    let Some(kind) = pointer_event_kind(etype) else {
        return CallbackResult::Keep;
    };

    // A button or drag event is always evidence that the held modifier was
    // used with the pointer. Scroll events can carry phase-only packets with no
    // movement, and momentum can continue after the user's fingers have left
    // the trackpad, so only a real, non-momentum delta interrupts a pending tap.
    let has_scroll_delta = kind == PointerEventKind::Scroll && scroll_has_delta(event);
    let is_momentum = kind == PointerEventKind::Scroll && scroll_is_momentum(event);
    let interrupts_tap = pointer_event_interrupts_tap(kind, has_scroll_delta, is_momentum);
    if interrupts_tap {
        app_state
            .engine
            .lock_safe()
            .process(KeyEvent::OtherInput { at_ms: now });
    }

    stamp_held_modifiers(state, event);
    CallbackResult::Keep
}

fn scroll_has_delta(event: &CGEvent) -> bool {
    let discrete = [
        EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_1,
        EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_2,
        SCROLL_DELTA_AXIS_3,
        EventField::SCROLL_WHEEL_EVENT_POINT_DELTA_AXIS_1,
        EventField::SCROLL_WHEEL_EVENT_POINT_DELTA_AXIS_2,
        SCROLL_POINT_DELTA_AXIS_3,
    ]
    .map(|field| event.get_integer_value_field(field));
    let continuous = [
        EventField::SCROLL_WHEEL_EVENT_FIXED_POINT_DELTA_AXIS_1,
        EventField::SCROLL_WHEEL_EVENT_FIXED_POINT_DELTA_AXIS_2,
        SCROLL_FIXED_POINT_DELTA_AXIS_3,
    ]
    .map(|field| event.get_double_value_field(field));
    scroll_delta_is_non_zero(discrete, continuous)
}

fn scroll_delta_is_non_zero(discrete: [i64; 6], continuous: [f64; 3]) -> bool {
    discrete.into_iter().any(|delta| delta != 0) || continuous.into_iter().any(|delta| delta != 0.0)
}

fn scroll_is_momentum(event: &CGEvent) -> bool {
    momentum_phase_is_active(event.get_integer_value_field(SCROLL_MOMENTUM_PHASE))
}

fn momentum_phase_is_active(phase: i64) -> bool {
    phase != 0
}

/// Stamp onto an input event the modifier flags contributed by keys held in a
/// special role: the hyper combo (⌃⌥⇧⌘) while a hyper key is held, and a
/// remapped key's target modifier while it is held. The latter makes a chord
/// through a remapped key behave as that modifier for both keyboard and pointer
/// operations (so holding Caps Lock→Control and clicking yields Ctrl-click)
/// even where the OS does not carry the rewritten `flagsChanged` flag forward.
/// The OS does not carry it for Caps Lock, which as a lock key leaves it with no
/// held-modifier state.
fn stamp_held_modifiers(state: &Arc<Mutex<TapState>>, event: &CGEvent) {
    let (hyper_active, (remove, add)) = {
        let ts = state.lock_safe();
        (ts.hyper_active, ts.remap_stamp.clone())
    };
    if !hyper_active && remove.is_empty() && add.is_empty() {
        return;
    }
    event.set_flags(stamped_modifier_flags(
        event.get_flags(),
        hyper_active,
        &remove,
        &add,
    ));
}

fn stamped_modifier_flags(
    mut flags: core_graphics::event::CGEventFlags,
    hyper_active: bool,
    remove: &[ModifierKey],
    add: &[ModifierKey],
) -> core_graphics::event::CGEventFlags {
    // Replace held remapped keys' source modifiers with their targets, so a
    // chord through them carries only the target (a remapped momentary modifier
    // must not leave both set, e.g. Control→Control+Command). Remove first,
    // then add, so a target that coincides with a removed source still lands.
    for modifier in remove {
        flags.remove(keycodes::flag_for(*modifier));
    }
    for modifier in add {
        flags.insert(keycodes::flag_for(*modifier));
    }
    // Hyper forces the full ⌃⌥⇧⌘ combo, so apply it last — it must win over a
    // remap that removed one of those flags as its source.
    if hyper_active {
        for modifier in HYPER_MODIFIERS {
            flags.insert(keycodes::flag_for(modifier));
        }
    }
    flags
}

/// Perform a completed tap's action.
///
/// Synthesized input — `SwitchIme`, `SendKeystroke` — is posted right here, from
/// the callback, through the tap's proxy (`CGEventTapPostEvent`). That is the
/// only place the ordering the user relies on can be guaranteed: the events go
/// into the stream at this tap's position, ahead of every event that has not yet
/// passed through it, so the character typed after a ⌘ tap cannot overtake the
/// IME switch. Queued onto another thread — the main thread especially, which
/// also serves AppKit, the webview and every Accessibility round-trip — the
/// switch could land after that character, which would then reach the old input
/// method.
///
/// Downstream, the pair therefore arrives just *before* the modifier release the
/// callback then returns (`⌘ down · 英数 down/up · ⌘ up`) rather than after it.
/// The synthesized events carry their own, explicit flags, so the app and the
/// input method read them as the plain keys they are; only a downstream tap
/// that infers modifier state from `flagsChanged` history would see them as
/// typed during the hold. Accepted over re-posting the release ahead of them
/// and dropping the original, which cannot be verified without a device.
///
/// Posting needs the Accessibility grant like any other event synthesis; the
/// tap proxy is not a way around it, and `CGEventTapPostEvent` reports nothing
/// when the post is ignored. The grant is read off a mirror the permission
/// poller keeps, never from TCC inside the callback. Everything else needs
/// AppKit and is queued on the main thread; a failure to queue is logged rather
/// than lost.
fn dispatch_tap_action(app: &AppHandle, proxy: CGEventTapProxy, action: AppAction) {
    if !matches!(
        action,
        AppAction::SwitchIme(_) | AppAction::SendKeystroke(_)
    ) {
        return dispatch_on_main(app, action);
    }
    if !ACCESSIBILITY.load(Ordering::SeqCst) {
        tracing::warn!("event-tap keystroke skipped: Accessibility permission not granted");
        return;
    }
    let sink = crate::keysend::Sink::Tap(proxy);
    let posted = match &action {
        AppAction::SwitchIme(mode) => crate::keysend::switch_ime(*mode, sink),
        AppAction::SendKeystroke(accel) => crate::keysend::send_accelerator(accel, sink),
        _ => unreachable!("routed above"),
    };
    if let Err(e) = posted {
        tracing::warn!(error = %e, "event-tap keystroke action failed");
    }
}

/// Run an action on the main thread (UI and webview calls require it).
fn dispatch_on_main(app: &AppHandle, action: AppAction) {
    let handle = app.clone();
    if let Err(e) = app.run_on_main_thread(move || {
        if let Some(state) = handle.try_state::<AppState>()
            && let Err(e) = crate::actions::dispatch(&action, &handle, state.inner())
        {
            tracing::warn!(error = %e, "event-tap action failed");
        }
    }) {
        tracing::warn!(error = %e, "could not queue an event-tap action on the main thread");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keycode of a managed modifier on the given side (mirrors the tap's own
    /// `modifier_for_keycode` table).
    fn keycode(modkey: ModifierKey, side: KeySide) -> i64 {
        match (modkey, side) {
            (ModifierKey::Command, KeySide::Left) => 55,
            (ModifierKey::Command, KeySide::Right) => 54,
            (ModifierKey::Shift, KeySide::Left) => 56,
            (ModifierKey::Shift, KeySide::Right) => 60,
            (ModifierKey::Control, KeySide::Left) => 59,
            (ModifierKey::Control, KeySide::Right) => 62,
            (ModifierKey::Option, KeySide::Left) => 58,
            (ModifierKey::Option, KeySide::Right) => 61,
            (ModifierKey::CapsLock, _) => 57,
            (ModifierKey::Function, _) => 63,
            other => panic!("unmapped test keycode for {other:?}"),
        }
    }

    #[test]
    fn native_caps_lock_is_never_tracked_as_a_hold() {
        // Regression guard: Caps Lock's native event used to toggle a "down"
        // belief, so a single press (macOS sends no release) or a release lost
        // to a tap outage left the engine holding Caps Lock and reading every
        // other modifier's solo tap as a chord. The lock key is kept out of
        // hold tracking entirely; every momentary modifier still goes in.
        assert!(!tracks_native_modifier(ModifierKey::CapsLock));
        for modkey in [
            ModifierKey::Command,
            ModifierKey::Shift,
            ModifierKey::Control,
            ModifierKey::Option,
            ModifierKey::Function,
        ] {
            assert!(tracks_native_modifier(modkey), "{modkey:?}");
        }
    }

    #[test]
    fn every_subscribed_pointer_event_is_classified() {
        for etype in EVENT_TYPES.into_iter().skip(3) {
            assert!(pointer_event_kind(etype).is_some(), "{etype:?}");
        }
        for etype in [
            CGEventType::KeyDown,
            CGEventType::KeyUp,
            CGEventType::FlagsChanged,
            CGEventType::MouseMoved,
        ] {
            assert!(pointer_event_kind(etype).is_none(), "{etype:?}");
        }
    }

    #[test]
    fn pointer_buttons_drags_and_real_scroll_interrupt_a_pending_tap() {
        assert!(pointer_event_interrupts_tap(
            PointerEventKind::Button,
            false,
            false,
        ));
        assert!(pointer_event_interrupts_tap(
            PointerEventKind::Drag,
            false,
            false,
        ));
        assert!(pointer_event_interrupts_tap(
            PointerEventKind::Scroll,
            true,
            false,
        ));
        assert!(!pointer_event_interrupts_tap(
            PointerEventKind::Scroll,
            false,
            false,
        ));
        assert!(!pointer_event_interrupts_tap(
            PointerEventKind::Scroll,
            true,
            true,
        ));
    }

    #[test]
    fn phase_only_scroll_is_not_input_but_scroll_with_a_delta_is() {
        assert!(!scroll_delta_is_non_zero([0; 6], [0.0; 3]));
        assert!(scroll_delta_is_non_zero([0, 0, 1, 0, 0, 0], [0.0; 3]));
        assert!(scroll_delta_is_non_zero([0; 6], [0.0, 0.0, 0.25]));
    }

    #[test]
    fn every_momentum_phase_is_excluded_from_tap_interruption() {
        assert!(!momentum_phase_is_active(0));
        for phase in [1, 2, 3] {
            assert!(momentum_phase_is_active(phase));
        }
    }

    #[test]
    fn held_remap_and_hyper_flags_can_be_stamped_onto_pointer_events() {
        use core_graphics::event::CGEventFlags;

        let flags = stamped_modifier_flags(
            CGEventFlags::CGEventFlagControl,
            false,
            &[ModifierKey::Control],
            &[ModifierKey::Command],
        );
        assert!(!flags.contains(CGEventFlags::CGEventFlagControl));
        assert!(flags.contains(CGEventFlags::CGEventFlagCommand));

        let hyper = stamped_modifier_flags(CGEventFlags::empty(), true, &[], &[]);
        for modifier in HYPER_MODIFIERS {
            assert!(hyper.contains(keycodes::flag_for(modifier)), "{modifier:?}");
        }
    }

    #[test]
    fn a_native_caps_lock_press_turns_a_pending_tap_into_a_chord() {
        // ⌘ down · Caps Lock · ⌘ up must not fire ⌘'s tap: the lock key is not
        // tracked as a hold, but its press still interrupts the pending tap —
        // and leaves nothing held behind, so the next solo ⌘ tap fires.
        use tomari_core::domain::action::ImeMode;
        use tomari_core::domain::keyboard::ModifierRule;
        let mut engine = tomari_keyboard::ModifierEngine::new(vec![ModifierRule {
            id: "cmd".into(),
            label: "cmd".into(),
            modifier: ModifierKey::Command,
            side: KeySide::Left,
            remap_to: None,
            hyper: false,
            tap: AppAction::SwitchIme(ImeMode::Alphanumeric),
            enabled: true,
        }]);
        let down = KeyEvent::ModifierDown {
            key: ModifierKey::Command,
            side: KeySide::Left,
            at_ms: 0,
        };
        let up = |at_ms| KeyEvent::ModifierUp {
            key: ModifierKey::Command,
            side: KeySide::Left,
            at_ms,
        };
        assert_eq!(engine.process(down), None);
        assert_eq!(engine.process(native_lock_key_event(10)), None);
        assert_eq!(engine.process(up(50)), None, "chorded with Caps Lock");
        assert!(!engine.is_held(ModifierKey::CapsLock));
        assert_eq!(engine.process(down), None);
        assert_eq!(
            engine.process(up(150)),
            Some(AppAction::SwitchIme(ImeMode::Alphanumeric)),
            "the next solo tap fires"
        );
    }

    #[test]
    fn left_right_modifiers_read_their_device_bit() {
        // Left Command's down/up follows NX_DEVICELCMDKEYMASK in the flags,
        // never the (sideless) generic Command bit.
        let lcmd = keycode(ModifierKey::Command, KeySide::Left);
        let bit = keycodes::device_flag_for_keycode(lcmd).unwrap();

        assert!(derive_is_down(lcmd, ModifierKey::Command, bit));
        assert!(!derive_is_down(lcmd, ModifierKey::Command, 0));
        // The generic Command flag alone, without the device bit, is a release.
        let generic = keycodes::flag_for(ModifierKey::Command).bits();
        assert!(!derive_is_down(lcmd, ModifierKey::Command, generic));
    }

    #[test]
    fn left_and_right_of_a_modifier_are_tracked_independently() {
        let lshift = keycode(ModifierKey::Shift, KeySide::Left);
        let rshift = keycode(ModifierKey::Shift, KeySide::Right);
        let lbit = keycodes::device_flag_for_keycode(lshift).unwrap();
        let rbit = keycodes::device_flag_for_keycode(rshift).unwrap();

        // Holding only the left side: left reads down, right reads up.
        assert!(derive_is_down(lshift, ModifierKey::Shift, lbit));
        assert!(!derive_is_down(rshift, ModifierKey::Shift, lbit));
        // Both sides held: each reads its own bit.
        assert!(derive_is_down(rshift, ModifierKey::Shift, lbit | rbit));
    }

    #[test]
    fn device_bit_keys_ignore_remembered_state_after_a_restart() {
        // Regression guard: the old toggle inverted when a tap restart swallowed
        // a transition mid-hold. Reading the flags makes a held key read `down`
        // on every event regardless of accumulated parity.
        let lctrl = keycode(ModifierKey::Control, KeySide::Left);
        let bit = keycodes::device_flag_for_keycode(lctrl).unwrap();
        for _ in 0..5 {
            assert!(
                derive_is_down(lctrl, ModifierKey::Control, bit),
                "a still-held key must keep reading as down"
            );
        }
    }

    #[test]
    fn fn_key_reads_its_generic_flag_bit() {
        let fn_code = keycode(ModifierKey::Function, KeySide::Either);
        assert!(keycodes::device_flag_for_keycode(fn_code).is_none());
        let bit = keycodes::flag_for(ModifierKey::Function).bits();

        assert!(derive_is_down(fn_code, ModifierKey::Function, bit));
        assert!(!derive_is_down(fn_code, ModifierKey::Function, 0));
    }
}
