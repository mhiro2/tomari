//! Drag-to-snap: drag a window (no modifier) to a screen edge or corner and it
//! snaps when you let go. A listen-only CGEventTap on its own thread watches the
//! left button's press/drag/release; once the window is actually moving the drag
//! arms, previews the target zone while the cursor hugs a border, and applies it
//! on release.
//!
//! Listening requires *Input Monitoring*; moving the window requires
//! *Accessibility*.
//!
//! ## Nothing slow on the callback
//!
//! Listen-only is about not *modifying* events, not about staying out of the
//! way: the system still waits for the callback to return before the event
//! travels on, which is why a slow one gets the tap disabled by timeout. So this
//! tap follows the same rule as the active [`drag_to_move`](crate::drag_to_move)
//! one — the callback calls into no other process and waits on nothing
//! unbounded. It reads the event's location and flags, checks one atomic
//! ([`ENABLED`], mirrored out of the settings for exactly this reason), posts a
//! [`Command`] into a bounded, coalescing queue ([`crate::mailbox`]: lifecycle
//! commands go lock-free, a cursor sample that finds the slot contended is
//! dropped rather than waited for), and returns.
//!
//! Everything else happens on the single [worker thread](worker_loop) started
//! with the tap: the hit-test that finds the window under the press, the frame
//! reads that tell a real window drag from a text selection, and the write that
//! snaps it on release. Each of those is Accessibility IPC bounded only by the
//! AX messaging timeout, and the pointer stream must not wait on any of it.
//!
//! What that stall used to cost was not just latency: with *three-finger drag*
//! turned on, macOS synthesizes the left button from finger movement, so the
//! start of a four-finger swipe (the fingers do not land at once) arrives here
//! as a mouse-down — and a hit-test blocking that event long enough kept
//! WindowServer from recognizing the swipe at all, which showed up as
//! Mission Control's space-switching gestures intermittently not working while
//! Tomari ran.
//!
//! Commands carry the generation of the press they belong to, and the queue is
//! drained before *every* Accessibility call, so a press superseded while a call
//! was in flight is folded into the intent and dropped without a call of its
//! own — the same shape as the drag-to-move applier.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use core_graphics::event::{
    CGEvent, CGEventTapOptions, CGEventTapPlacement, CGEventType, CallbackResult,
};
use tauri::{AppHandle, Manager};
use tomari_core::{Rect, WindowPreset};
use tomari_window::{DragWindow, WindowHandle, compute_frame, edge_snap_preset, screen_at_cursor};

use crate::locks::MutexExt;
use crate::mailbox::{self, Coalesce, Receiver, Sender};
use crate::overlay;
use crate::state::AppState;
use crate::tap::{self, RunningTap, TapHealth, TapHealthCell};

/// The single live drag-to-snap tap, owned globally like the keyboard tap.
static DRAG_TAP: Mutex<Option<RunningTap>> = Mutex::new(None);

/// Where this tap stands (see [`TapHealth`]); logged on every change.
static HEALTH: TapHealthCell = TapHealthCell::new("drag-to-snap");

/// Whether the feature is on, mirrored out of the settings by
/// [`restart_result`] so the callback can check it with one atomic load instead
/// of taking the settings lock on every mouse event. Authoritative for the
/// callback only: [`restart_result`] reads the settings themselves, and every
/// path that can change either the master switch or this feature's own toggle
/// goes through it.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// (Re)start the tap to match the current settings: tear down any existing tap
/// and, if drag-to-snap is enabled, start a fresh one. Callers that do not need
/// the outcome (permission polling, wake/session reset) use this; [`commands`]
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
pub fn restart_result(app: &AppHandle) -> bool {
    let Some(state) = app.try_state::<AppState>() else {
        return true;
    };
    // Runtime effects always precede the tap lock. Shutdown closes this gate,
    // drains work that already entered it, and only then tears the tap down.
    let Some(_effect) = state.lifecycle.runtime_effect() else {
        return true;
    };
    let mut guard = DRAG_TAP.lock_safe();
    let enabled = drag_to_snap_enabled_for(app);
    // Published before the old tap goes down, so no reader sees `Healthy` over
    // a tap being torn down; also retires the old callback's generation.
    HEALTH.begin_start(enabled);
    *guard = None; // Drop stops the previous tap and joins its worker.
    // Any snap preview on screen belongs to the tap we just dropped; a restart
    // (settings change, wake, permission grant) must not leave it stuck.
    overlay::hide(app);

    ENABLED.store(enabled, Ordering::SeqCst);
    if !enabled {
        HEALTH.set(TapHealth::Stopped);
        return true;
    }

    match start(app.clone()) {
        Ok(tap) => {
            *guard = Some(tap);
            // `Healthy` only once the handle is in place, so the state never
            // says "running" ahead of it.
            HEALTH.set(TapHealth::Healthy);
            tracing::info!("drag-to-snap event tap started");
            true
        }
        Err(e) => {
            HEALTH.record_start_failure(crate::eventtap::input_monitoring_granted());
            tracing::warn!(error = %e, "drag-to-snap event tap not started (grant Input Monitoring?)");
            false
        }
    }
}

/// Stop the tap permanently as part of the shared terminal shutdown.
pub fn teardown(app: &AppHandle) {
    let mut guard = DRAG_TAP.lock_safe();
    HEALTH.stop();
    ENABLED.store(false, Ordering::SeqCst);
    *guard = None;
    overlay::hide(app);
}

/// Whether the drag-to-snap tap is currently running. A cheap state read
/// so `save_settings` can verify on *every* save that an enabled feature
/// actually has its tap alive — a warning must reflect the live state, not
/// just the last restart attempt, or it would vanish from the UI on the next
/// unrelated save while the tap is still dead.
pub fn is_running() -> bool {
    // Read off the health state rather than the handle: a handle only proves
    // a start once succeeded, while the state also knows a tap the system
    // disabled and that is being asked back (still counted as running — it
    // is not a configuration problem the user can act on), and a revoke or
    // failure the restart recorded. The tap lock is taken first so a restart
    // in flight (which holds it from `Starting` through to its outcome) is
    // waited out rather than read as "not running" mid-way.
    let _serialized = DRAG_TAP.lock_safe();
    matches!(
        HEALTH.state(),
        TapHealth::Healthy | TapHealth::DisabledByTimeout
    )
}

pub fn health_snapshot() -> crate::tap::TapHealthSnapshot {
    HEALTH.snapshot()
}

/// Whether drag-to-snap should run: it shares the window-management master
/// switch and has its own opt-in toggle.
fn drag_to_snap_enabled_for(app: &AppHandle) -> bool {
    app.try_state::<AppState>()
        .map(|s| s.settings.lock_safe().drag_to_snap_tap_enabled())
        .unwrap_or(false)
}

/// How far (points) the dragged window's origin must move from its mouse-down
/// frame before a drag counts as a real window move and arms drag-to-snap.
const MOVE_EPSILON: f64 = 1.0;

/// How far (points) the cursor must move from its mouse-down location before an
/// unarmed drag is even worth an AX frame read. Below this, the pointer is
/// essentially where it was pressed (a click, a tiny jitter) and the window
/// could not plausibly have moved past `MOVE_EPSILON` yet — skip the AX call
/// entirely rather than confirm a foregone conclusion.
const CURSOR_MOVE_EPSILON: f64 = 2.0;

/// Minimum time between AX frame reads while a drag is unarmed. Mouse-dragged
/// events arrive at 60-120 Hz; without this an unarmed drag (e.g. a text
/// selection over another app) would perform an AX IPC round trip for every
/// single one of them, which can trip that app's own responsiveness. Once armed
/// this no longer applies — armed drags resolve purely from the cached
/// cursor/screen geometry and never read the window's frame again.
const FRAME_CHECK_INTERVAL: Duration = Duration::from_millis(50);

/// How stale a press may be when the worker first gets to it. The hit-test uses
/// the *press* coordinates, so it is only truthful while the window is still
/// roughly where it was pressed; if the worker was held up (an earlier AX call
/// against a wedged app can hold it for the messaging timeout), those
/// coordinates may by now sit over a window behind the one being dragged, and
/// the frame read would record a mid-drag position as Undo's origin. Dropping
/// such a press costs at most one missed snap; taking it could snap the wrong
/// window and put that in the undo history. Comfortably above the microseconds
/// an idle worker needs, well below the AX messaging timeout.
const PRESS_FRESHNESS: Duration = Duration::from_millis(50);

/// One instruction from the callback to the worker thread. Every variant
/// carries the generation of the press it belongs to, so the worker can drop
/// anything belonging to a press that has since been superseded without making
/// an Accessibility call for it.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Command {
    /// The left button went down at `location`, at `at`. `chord` is whether the
    /// drag-to-move modifiers were held: that gesture drives the window itself,
    /// so this press is not ours to snap.
    Press {
        generation: u64,
        location: (f64, f64),
        at: Instant,
        chord: bool,
    },
    /// The cursor moved with the button down.
    Drag {
        generation: u64,
        location: (f64, f64),
        chord: bool,
    },
    /// The button came up. It carries no location on purpose: the drop belongs
    /// to the zone that was last *previewed*, which is what the user saw, even
    /// if the release itself lands outside every edge band.
    Release { generation: u64 },
    /// The press is abandoned: the tap was disabled mid-drag (events were
    /// missed, so the release may never arrive) or is being torn down.
    Cancel { generation: u64 },
}

impl Coalesce for Command {
    /// A newer cursor position for the same press replaces the pending one:
    /// only the latest matters, and it must never pile up behind a stalled
    /// worker. Press, release and cancel are lifecycle: delivered always,
    /// on the lock-free path (see [`crate::mailbox`]).
    fn supersedes(&self, earlier: &Self) -> bool {
        matches!(
            (self, earlier),
            (Command::Drag { generation: g, .. }, Command::Drag { generation: h, .. }) if g == h
        )
    }

    fn sheddable(&self) -> bool {
        matches!(self, Command::Drag { .. })
    }
}

/// What the worker knows about the newest press, independent of whether a
/// window has been resolved for it yet.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Intent {
    generation: u64,
    press: (f64, f64),
    /// When the button went down, so a press the worker could not look at in
    /// time is dropped rather than hit-tested against stale coordinates.
    pressed_at: Instant,
    /// The newest cursor position; the press location until it moves.
    cursor: (f64, f64),
    /// `cursor` has not been turned into a preview yet.
    cursor_fresh: bool,
    /// Whether the drag-to-move chord has been held at any point during this
    /// press. Sticky: a chord seen once has already handed the press over, and
    /// letting go of it again must not hand it back (only the unarmed branch of
    /// [`next_step`] reads this, so a chord pressed *after* the drag armed still
    /// changes nothing).
    chord: bool,
    released: bool,
    cancelled: bool,
}

impl Intent {
    /// Whether the cursor has moved far enough from the press for the window to
    /// plausibly be moving with it (see [`CURSOR_MOVE_EPSILON`]).
    fn cursor_moved(&self) -> bool {
        (self.cursor.0 - self.press.0).abs() > CURSOR_MOVE_EPSILON
            || (self.cursor.1 - self.press.1).abs() > CURSOR_MOVE_EPSILON
    }
}

/// Fold one command into the worker's picture of the newest press.
///
/// A `Press` always replaces: generations only increase, so the newest one is
/// the only press that can still be snapped. Everything else is matched on
/// generation, which is what makes a command left over from a superseded press
/// a no-op instead of a stray preview or write.
fn absorb(intent: &mut Option<Intent>, command: Command) {
    match command {
        Command::Press {
            generation,
            location,
            at,
            chord,
        } => {
            *intent = Some(Intent {
                generation,
                press: location,
                pressed_at: at,
                cursor: location,
                cursor_fresh: false,
                chord,
                released: false,
                cancelled: false,
            });
        }
        Command::Drag {
            generation,
            location,
            chord,
        } => {
            if let Some(current) = intent
                .as_mut()
                .filter(|i| i.generation == generation && !i.released && !i.cancelled)
            {
                current.cursor = location;
                current.cursor_fresh = true;
                current.chord |= chord;
            }
        }
        Command::Release { generation } => {
            if let Some(current) = intent
                .as_mut()
                .filter(|i| i.generation == generation && !i.cancelled)
            {
                current.released = true;
            }
        }
        Command::Cancel { generation } => {
            if let Some(current) = intent.as_mut().filter(|i| i.generation == generation) {
                current.cancelled = true;
            }
        }
    }
}

/// An in-flight drag the worker has resolved a window for: the window grabbed
/// on mouse-down plus the live preview state. The window is not snapped until
/// release; until then this only reads the cursor and shows the preview.
struct Session {
    generation: u64,
    /// The window under the cursor when the button went down.
    window: DragWindow,
    /// Its frame at that moment, used to confirm the OS is actually *moving* it
    /// (a title-bar drag) before arming — text selection and other drags leave
    /// the frame put and never arm.
    start_frame: Rect,
    /// When `start_frame` was last confirmed against a fresh AX read while
    /// unarmed, so a run of drag events throttles to at most one AX call per
    /// [`FRAME_CHECK_INTERVAL`].
    last_frame_check: Instant,
    /// Whether the one frame read a release is allowed outside the throttle has
    /// already been spent. Without it a release over a window that never moved
    /// would ask again immediately, forever.
    final_check: bool,
    armed: bool,
    /// Every display's `(full_frame, work_area)` (CG), snapshotted on arm so
    /// edge detection on later moves stays a pure computation — and re-read
    /// whenever the cache's generation has moved on (a display unplugged,
    /// rearranged or resized, the Dock changing the work area), so a preview
    /// or a drop is never laid out against geometry that no longer exists.
    screens: Vec<(Rect, Rect)>,
    /// The cache generation `screens` was read at (see
    /// `AppState::screen_geometry_snapshot`).
    screens_generation: u64,
    /// The newest cursor position the preview was computed from, so the drop
    /// can be re-targeted against fresh geometry if the displays changed.
    cursor: (f64, f64),
    /// The preset the newest cursor position selects together with the work area
    /// to lay it out in. Stored as one value (not just the preset) so that
    /// dragging between displays whose edges map to the *same* preset — e.g. the
    /// top of display A to the top of display B, both `Maximize` — still
    /// re-targets the preview.
    ///
    /// This is the zone the drop snaps to, and normally the one on screen: it is
    /// recomputed from every cursor position the worker sees, and the preview
    /// follows it. The exception is a release folded in with the drag before it
    /// (the worker was held up): the newest position is still what decides the
    /// snap, but its `overlay::show` is superseded by the release's own `hide`
    /// before it can render, so the last zone the user actually *saw* may be the
    /// one before it. Following the cursor is the better of the two — it is
    /// where the user let go — but it is not the same promise.
    active: Option<(WindowPreset, Rect)>,
}

/// The parts of a [`Session`] the decision in [`next_step`] is allowed to see,
/// so that decision stays pure and testable — a `Session` itself owns an
/// `AXUIElement` and cannot be built without a live window.
#[derive(Clone, Copy, PartialEq, Debug)]
struct SessionView {
    generation: u64,
    armed: bool,
    last_frame_check: Instant,
    final_check: bool,
    /// Whether a snap zone is currently previewed, i.e. whether a release now
    /// would have somewhere to snap to.
    has_target: bool,
}

/// The one thing the worker should do next, given what the callback has told it
/// and what it has resolved so far. Pure, so which of these an ended or
/// abandoned drag can still reach is settled by tests rather than by scheduling.
#[derive(PartialEq, Debug)]
enum Next {
    /// Hit-test the press to find the window under it. Several Accessibility
    /// round trips.
    Resolve { generation: u64, press: (f64, f64) },
    /// Read the window's frame to see whether the OS is really moving it.
    /// One Accessibility round trip.
    CheckMoving,
    /// Re-target the preview from the newest cursor position. No IPC.
    Preview,
    /// The drag was released over a zone: snap the window to it.
    Apply,
    /// This press is over (or was never ours): let go of it, clearing any
    /// preview it left on screen.
    Forget,
    /// Nothing to do until the callback says more.
    Wait,
}

fn next_step(intent: Option<Intent>, session: Option<SessionView>, now: Instant) -> Next {
    let Some(current) = intent else {
        return Next::Forget;
    };
    if current.cancelled {
        return Next::Forget;
    }

    let Some(live) = session.filter(|s| s.generation == current.generation) else {
        // Nothing resolved for this press yet.
        if current.chord {
            // Drag-to-move owns this press and is driving the window itself; a
            // competing edge preview would flash against the move.
            return Next::Forget;
        }
        if current.released && !current.cursor_moved() {
            // An ordinary click, already over: it could not have snapped
            // anything, so spend no Accessibility calls establishing that.
            return Next::Forget;
        }
        if now.duration_since(current.pressed_at) > PRESS_FRESHNESS {
            // Too late to hit-test honestly (see `PRESS_FRESHNESS`).
            return Next::Forget;
        }
        return Next::Resolve {
            generation: current.generation,
            press: current.press,
        };
    };

    if !live.armed {
        // A chord that came down before the drag armed hands the press to
        // drag-to-move, which is driving the window itself.
        if current.chord {
            return Next::Forget;
        }
        // Cheap prefilter: the cursor itself must have moved before the window
        // could plausibly have moved past `MOVE_EPSILON`. Most drag traffic
        // while unarmed is a text selection whose cursor wanders even though
        // nothing about the window changes, so this only skips the frame read
        // when the cursor has barely left the press at all.
        if !current.cursor_moved() {
            return if current.released {
                Next::Forget
            } else {
                Next::Wait
            };
        }
        // A release is exempt from the throttle below — there is no run left to
        // pace, and this is the last chance to notice that the window did move,
        // so a drag quick enough to finish inside one interval can still arm and
        // snap. Exactly once, though: the answer cannot change after the button
        // is up, so a window that had not moved is done being asked.
        if current.released {
            return if live.final_check {
                Next::Forget
            } else {
                Next::CheckMoving
            };
        }
        // Throttle the AX round trip itself, so a heavy run of drag events
        // never turns into a matching run of IPC calls against whatever app is
        // under the cursor.
        if now.duration_since(live.last_frame_check) < FRAME_CHECK_INTERVAL {
            return Next::Wait;
        }
        return Next::CheckMoving;
    }

    // Armed: a cursor position the preview has not seen yet re-targets it
    // first, including one that arrived folded together with the release, so the
    // drop always follows the newest cursor rather than a stale zone. The
    // release itself carries no position of its own.
    if current.cursor_fresh {
        return Next::Preview;
    }
    if current.released {
        return if live.has_target {
            Next::Apply
        } else {
            Next::Forget
        };
    }
    Next::Wait
}

/// Whether [`step`] did something, and so should be preceded by a fresh look at
/// the queue before the next one.
#[derive(PartialEq, Debug)]
enum Step {
    /// Work was done; the queue may have moved on while it ran.
    Progressed,
    /// Nothing left to do for the press as it currently stands.
    Idle,
}

/// Hit-test the window under `press`. The several-round-trip part: the
/// hit-test, the walk to the owning window, then the frame read — each call
/// bounded by the element's AX messaging timeout, but plural. It runs here, on
/// the worker thread, and never on the callback.
fn resolve(app: &AppHandle, generation: u64, press: (f64, f64)) -> Option<Session> {
    let app_state = app.try_state::<AppState>()?;
    if !app_state.windows.permission_granted() {
        return None;
    }
    let window = match tomari_window::window_at_point(press.0, press.1) {
        Ok(window) => window,
        Err(e) => {
            // No permission, nothing draggable under the pointer, or the app did
            // not answer in time. Debug, not warn: an ordinary click on anything
            // undraggable comes through here.
            tracing::debug!(error = %e, "drag-to-snap found nothing draggable under the pointer");
            return None;
        }
    };
    let start_frame = window.frame().ok()?;
    Some(Session {
        generation,
        window,
        start_frame,
        // Backdated by a full interval, not `Instant::now()`: the first drag
        // whose cursor has already cleared `CURSOR_MOVE_EPSILON` must be allowed
        // an immediate AX read. A `now()` baseline would instead make that very
        // first check appear recent and throttle it away, so a quick drag (grab,
        // move past the edge, release, all within `FRAME_CHECK_INTERVAL`) could
        // complete without ever reading the frame once and would never arm.
        last_frame_check: Instant::now() - FRAME_CHECK_INTERVAL,
        final_check: false,
        armed: false,
        screens: Vec::new(),
        screens_generation: 0,
        cursor: press,
        active: None,
    })
}

/// Drop a session, taking down any preview it had on screen.
fn forget(app: &AppHandle, session: &mut Option<Session>) {
    if let Some(previous) = session.take()
        && previous.active.is_some()
    {
        overlay::hide(app);
    }
}

/// Whether the display geometry has been refreshed since `live` last read it.
fn screens_stale(app: &AppHandle, live: &Session) -> bool {
    app.try_state::<AppState>()
        .is_some_and(|s| s.screen_geometry_generation() != live.screens_generation)
}

/// Re-read the cached display geometry (and its generation) into `live`.
fn refresh_screens(app: &AppHandle, live: &mut Session) {
    if let Some(state) = app.try_state::<AppState>() {
        let (generation, screens) = state.screen_geometry_snapshot();
        live.screens = screens;
        live.screens_generation = generation;
    }
}

/// What a release does, decided against the display geometry current at that
/// moment rather than the one the preview was drawn against.
#[derive(Debug, Clone, Copy, PartialEq)]
enum DropDecision {
    /// Snap to this zone — the previewed one when the displays are unchanged,
    /// the re-targeted one when they moved but the cursor still selects a zone.
    Apply((WindowPreset, Rect)),
    /// The cursor selects no zone on the current displays (the previewed one
    /// is gone); leave the window where the OS dropped it.
    Abort,
}

/// Decide the drop for `cursor` on the current `screens`, given what was
/// `previewed`. Pure; the preview only breaks ties in the log, never the
/// decision — geometry the user cannot see any more must not place a window.
fn drop_target(
    screens: &[(Rect, Rect)],
    cursor: (f64, f64),
    previewed: (WindowPreset, Rect),
) -> DropDecision {
    match target_for(screens, cursor) {
        Some(target) => {
            if target != previewed {
                tracing::debug!("drag-to-snap re-targeted the drop after a display change");
            }
            DropDecision::Apply(target)
        }
        None => DropDecision::Abort,
    }
}

/// The snap zone `cursor` selects on `screens`: the preset its edge band maps
/// to, with the work area to lay it out in. Pure.
fn target_for(screens: &[(Rect, Rect)], cursor: (f64, f64)) -> Option<(WindowPreset, Rect)> {
    let (x, y) = cursor;
    screen_at_cursor(screens, x, y)
        .and_then(|(full, visible)| edge_snap_preset((x, y), full).map(|preset| (preset, visible)))
}

/// Carry out at most *one* step towards the current intent.
fn step(app: &AppHandle, intent: &mut Option<Intent>, session: &mut Option<Session>) -> Step {
    let view = session.as_ref().map(|s| SessionView {
        generation: s.generation,
        armed: s.armed,
        last_frame_check: s.last_frame_check,
        final_check: s.final_check,
        has_target: s.active.is_some(),
    });
    match next_step(*intent, view, Instant::now()) {
        Next::Wait => Step::Idle,
        Next::Forget => {
            forget(app, session);
            // Also drop the intent, not just the session: every path that
            // reaches here is done with this press, and a `Drag` still queued
            // behind a `Cancel` (or behind the chord that handed the press to
            // drag-to-move) must not resolve a window and start snapping after
            // the fact.
            *intent = None;
            Step::Idle
        }
        Next::Resolve { generation, press } => {
            // A press that superseded an older one takes its preview down
            // before the round trips, not after.
            forget(app, session);
            *session = resolve(app, generation, press);
            if session.is_none() {
                // No permission, nothing draggable under the cursor, or the app
                // did not answer in time: this press is not ours.
                *intent = None;
            }
            Step::Progressed
        }
        Next::CheckMoving => {
            let Some(live) = session.as_mut() else {
                return Step::Idle;
            };
            live.last_frame_check = Instant::now();
            live.final_check |= intent.is_some_and(|i| i.released);
            match live.window.frame() {
                Ok(frame) => {
                    let moved = (frame.x - live.start_frame.x).abs() > MOVE_EPSILON
                        || (frame.y - live.start_frame.y).abs() > MOVE_EPSILON;
                    if moved {
                        live.armed = true;
                        // Read the geometry cached on the main thread (kept
                        // current by the display-change observer in `displays`);
                        // never block on the window server for it.
                        refresh_screens(app, live);
                    }
                    // Not moving: dragging something that is not the window
                    // (text selection, a control). Leave it alone and look
                    // again after the throttle interval.
                }
                Err(e) => {
                    // The window went away or stopped answering; abandon this
                    // drag rather than retry it for the rest of the press.
                    tracing::debug!(error = %e, "drag-to-snap stopped: window frame unreadable");
                    forget(app, session);
                    *intent = None;
                }
            }
            Step::Progressed
        }
        Next::Preview => {
            let Some(live) = session.as_mut() else {
                return Step::Idle;
            };
            let Some(current) = intent.as_mut() else {
                return Step::Idle;
            };
            // Consumed: the same position must not be re-targeted twice.
            current.cursor_fresh = false;
            live.cursor = current.cursor;
            // Displays may have changed since the snapshot on arm; a preview
            // against a display that is gone would promise a drop nowhere.
            if screens_stale(app, live) {
                refresh_screens(app, live);
            }
            let target = target_for(&live.screens, live.cursor);
            // Compare the whole target, not just the preset: moving between
            // displays whose edges share a preset must still re-target.
            if target != live.active {
                live.active = target;
                match target {
                    Some((preset, visible)) => overlay::show(app, compute_frame(preset, visible)),
                    None => overlay::hide(app),
                }
            }
            Step::Progressed
        }
        Next::Apply => {
            *intent = None;
            let Some(mut live) = session.take() else {
                return Step::Idle;
            };
            overlay::hide(app);
            let Some(active) = live.active else {
                return Step::Progressed;
            };
            let Some(app_state) = app.try_state::<AppState>() else {
                return Step::Progressed;
            };
            // Decide the drop from geometry read *now* — under the window
            // lock, right before the write — not from the preview: a display
            // change since the preview (unplugged, rearranged, resized, Dock
            // moved), or while waiting for the lock, can leave the previewed
            // work area pointing off-screen. Re-target from the last cursor
            // position against the fresh snapshot; if it selects no zone any
            // more, decline the snap — a window moved to nowhere is worse than
            // a drag that did nothing. A change landing during the AX write
            // itself cannot be caught from here; that window is the write's own
            // duration.
            let window = live.window.clone();
            let start_frame = live.start_frame;
            let changed = crate::window_ops::apply_dragged(
                app_state.inner(),
                &window,
                start_frame,
                || {
                    refresh_screens(app, &mut live);
                    match drop_target(&live.screens, live.cursor, active) {
                        DropDecision::Apply((preset, visible)) => {
                            Some(compute_frame(preset, visible))
                        }
                        DropDecision::Abort => {
                            tracing::debug!(
                                "drag-to-snap aborted: the displays changed and the drop selects no zone"
                            );
                            None
                        }
                    }
                },
            );
            if changed {
                // The tray/menu APIs are main-thread-only, and this runs on the
                // worker thread.
                let handle = app.clone();
                let _ = app.run_on_main_thread(move || crate::tray::refresh(&handle));
            }
            Step::Progressed
        }
    }
}

/// Follow presses until the callback's sender is dropped (the tap is being torn
/// down).
///
/// The queue is drained before *every* step, not once per batch: resolving a
/// window takes several round trips, and a drag that ended while one was in
/// flight must not then get a preview from the cursor position it had
/// beforehand.
fn worker_loop(app: &AppHandle, rx: &mut Receiver<Command>) {
    let mut intent: Option<Intent> = None;
    let mut session: Option<Session> = None;

    while let Some(first) = rx.recv() {
        absorb(&mut intent, first);
        loop {
            while let Some(next) = rx.try_recv() {
                absorb(&mut intent, next);
            }
            if step(app, &mut intent, &mut session) == Step::Idle {
                break;
            }
        }
    }
    // The tap is going away; whatever is still on screen belongs to it.
    forget(app, &mut session);
}

/// State the tap keeps across events. Every field is either an atomic or only
/// touched through `&mut self` in [`Drop`], because the callback is `Fn`; none
/// of it needs a lock of its own, and none of its methods blocks: a cursor
/// sample that finds the queue's slot contended is dropped, and lifecycle
/// commands take a lock-free path (see [`crate::mailbox`]).
struct SnapTapState {
    /// Commands to the worker. `None` when its thread could not be started, in
    /// which case the tap simply observes nothing.
    tx: Option<Sender<Command>>,
    worker: Option<JoinHandle<()>>,
    /// Bumped for every press, so the worker can tell a command from this drag
    /// from one left over from the last.
    generation: AtomicU64,
}

impl SnapTapState {
    fn new(app: AppHandle) -> Self {
        let (tx, rx) = mailbox::channel::<Command>("drag-to-snap");
        match std::thread::Builder::new()
            .name("tomari-dragsnap-apply".into())
            .spawn(move || {
                let mut rx = rx;
                worker_loop(&app, &mut rx)
            }) {
            Ok(worker) => Self {
                tx: Some(tx),
                worker: Some(worker),
                generation: AtomicU64::new(0),
            },
            Err(e) => {
                tracing::warn!(error = %e, "could not start the drag-to-snap worker thread");
                Self {
                    tx: None,
                    worker: None,
                    generation: AtomicU64::new(0),
                }
            }
        }
    }

    /// Post a command. Non-blocking; folded into the bounded queue (see
    /// [`crate::mailbox`]).
    fn send(&self, command: Command) {
        if let Some(tx) = self.tx.as_ref() {
            let _ = tx.send(command);
        }
    }

    fn press(&self, location: (f64, f64), at: Instant, chord: bool) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.send(Command::Press {
            generation,
            location,
            at,
            chord,
        });
    }

    fn drag(&self, location: (f64, f64), chord: bool) {
        self.send(Command::Drag {
            generation: self.generation.load(Ordering::SeqCst),
            location,
            chord,
        });
    }

    fn release(&self) {
        self.send(Command::Release {
            generation: self.generation.load(Ordering::SeqCst),
        });
    }

    fn cancel(&self) {
        self.send(Command::Cancel {
            generation: self.generation.load(Ordering::SeqCst),
        });
    }
}

impl Drop for SnapTapState {
    /// Join the worker before the callback itself is torn down, the same
    /// join-order care [`crate::drag_to_move`] takes: `restart_result` drops the
    /// previous `RunningTap` (joining the tap *thread*) before starting the next
    /// one, and without this the new tap could see a fresh press while the old
    /// worker's last snap was still in flight.
    ///
    /// This runs when the callback closure is dropped, which happens on the tap
    /// thread right after `CFRunLoopRun` returns (see `tap::run_tap`) — strictly
    /// *before* `RunningTap::drop`'s `handle.join()` returns, so normally the
    /// worker has exited and its preview is down by the time `restart_result`
    /// can start the next tap. The wait is short because a call already in
    /// flight is bounded by the AX messaging timeout, but that bound is
    /// best-effort (see `tomari_window`): a target app that is both wedged and
    /// had its timeout rejected can hold this past `RunningTap`'s own bounded
    /// join, which then detaches the tap thread — so the guarantee is "normally,
    /// not always". Unlike the callback, a restart is not holding up anyone's
    /// pointer events while it waits.
    fn drop(&mut self) {
        self.cancel();
        self.tx = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn start(app: AppHandle) -> Result<RunningTap, String> {
    tap::spawn(
        "tomari-dragtosnap",
        "drag-to-snap tap",
        // Modifier remap and Hyper flags must be normalized before this tap
        // decides whether the pointer chord belongs to a gesture.
        CGEventTapPlacement::TailAppendEventTap,
        CGEventTapOptions::ListenOnly,
        vec![
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseDragged,
            CGEventType::LeftMouseUp,
        ],
        move |port_holder| {
            // The state never leaves this thread: the callback runs only on this
            // run loop, and the worker it owns is joined when it drops.
            let state = SnapTapState::new(app);
            // The generation this tap is started under; its health reports
            // are dropped once a later start retires it.
            let generation = HEALTH.generation();
            Box::new(move |_proxy, etype, event: &CGEvent| {
                handle_event(&state, &port_holder, generation, etype, event)
            })
        },
    )
}

fn handle_event(
    state: &SnapTapState,
    port_holder: &AtomicUsize,
    generation: u64,
    etype: CGEventType,
    event: &CGEvent,
) -> CallbackResult {
    // The system disabled the tap (timeout / heavy input): re-enable it, or
    // drag-to-snap would silently stop working until the next settings change.
    if matches!(
        etype,
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
    ) {
        // While disabled we may have missed the matching mouse-up, which would
        // otherwise leave a snap preview stranded.
        state.cancel();
        HEALTH.record_disabled(generation);
        HEALTH.record_reenable(generation, tap::reenable(port_holder));
        return CallbackResult::Keep;
    }

    // Settings-driven menu bar arrangement is a synthesized pointer gesture,
    // not a window drag. Let it reach macOS without arming a snap candidate.
    if crate::eventtap::is_synthetic(event) {
        return CallbackResult::Keep;
    }

    if !ENABLED.load(Ordering::SeqCst) {
        return CallbackResult::Keep;
    }

    // Everything below is a flag read, an atomic and a channel send — nothing
    // that can block the pointer stream. See the module doc comment.
    match etype {
        CGEventType::LeftMouseDown => {
            let location = event.location();
            state.press(
                (location.x, location.y),
                Instant::now(),
                crate::drag_to_move::gesture_for_flags(event.get_flags()).is_some(),
            );
        }
        CGEventType::LeftMouseDragged => {
            let location = event.location();
            state.drag(
                (location.x, location.y),
                crate::drag_to_move::gesture_for_flags(event.get_flags()).is_some(),
            );
        }
        CGEventType::LeftMouseUp => state.release(),
        _ => {}
    }

    CallbackResult::Keep
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two side-by-side displays, each with a menu-bar-sized work-area inset.
    fn two_displays() -> Vec<(Rect, Rect)> {
        vec![
            (
                Rect::new(0.0, 0.0, 1000.0, 800.0),
                Rect::new(0.0, 25.0, 1000.0, 775.0),
            ),
            (
                Rect::new(1000.0, 0.0, 1000.0, 800.0),
                Rect::new(1000.0, 25.0, 1000.0, 775.0),
            ),
        ]
    }

    #[test]
    fn a_drop_on_unchanged_displays_applies_the_previewed_zone() {
        let screens = two_displays();
        // Cursor at the very top edge of the second display.
        let cursor = (1500.0, 0.0);
        let previewed = target_for(&screens, cursor).expect("the top edge selects a zone");
        assert_eq!(
            drop_target(&screens, cursor, previewed),
            DropDecision::Apply(previewed)
        );
    }

    #[test]
    fn a_drop_after_the_previewed_display_vanished_is_aborted() {
        let screens = two_displays();
        let cursor = (1500.0, 0.0);
        let previewed = target_for(&screens, cursor).expect("the top edge selects a zone");
        // The second display was unplugged before the release.
        let remaining = vec![screens[0]];
        assert_eq!(
            drop_target(&remaining, cursor, previewed),
            DropDecision::Abort
        );
    }

    #[test]
    fn a_drop_after_the_work_area_changed_is_re_targeted_to_the_fresh_geometry() {
        let screens = two_displays();
        let cursor = (1500.0, 0.0);
        let previewed = target_for(&screens, cursor).expect("the top edge selects a zone");
        // The Dock moved: the second display's work area shrank.
        let mut changed = screens.clone();
        changed[1].1 = Rect::new(1000.0, 25.0, 1000.0, 700.0);
        match drop_target(&changed, cursor, previewed) {
            DropDecision::Apply((preset, visible)) => {
                assert_eq!(preset, previewed.0);
                assert_eq!(visible, changed[1].1, "laid out in the current work area");
                assert_ne!(visible, previewed.1);
            }
            DropDecision::Abort => panic!("the zone still exists; the drop must apply"),
        }
    }

    const PRESS: (f64, f64) = (100.0, 200.0);
    /// Past `CURSOR_MOVE_EPSILON` from [`PRESS`].
    const MOVED: (f64, f64) = (140.0, 200.0);

    fn pressed_at(generation: u64, chord: bool, at: Instant) -> Option<Intent> {
        let mut intent = None;
        absorb(
            &mut intent,
            Command::Press {
                generation,
                location: PRESS,
                at,
                chord,
            },
        );
        intent
    }

    fn view(armed: bool, has_target: bool, last_frame_check: Instant) -> Option<SessionView> {
        Some(SessionView {
            generation: 1,
            armed,
            last_frame_check,
            final_check: false,
            has_target,
        })
    }

    /// Long enough ago that the frame-read throttle has elapsed.
    fn stale_check(now: Instant) -> Instant {
        now - FRAME_CHECK_INTERVAL - Duration::from_millis(1)
    }

    #[test]
    fn a_press_resolves_the_window_under_it() {
        let now = Instant::now();
        assert_eq!(
            next_step(pressed_at(1, false, now), None, now),
            Next::Resolve {
                generation: 1,
                press: PRESS
            }
        );
    }

    #[test]
    fn a_press_carrying_the_drag_to_move_chord_is_left_alone() {
        // Drag-to-move drives the window itself; a competing edge preview would
        // flash against the move, and the hit-test would be spent for nothing.
        let now = Instant::now();
        assert_eq!(next_step(pressed_at(1, true, now), None, now), Next::Forget);
    }

    #[test]
    fn a_click_folded_before_its_first_step_costs_no_accessibility_call() {
        // Press and release at the same spot, both already absorbed when the
        // worker first looks: there is nothing to snap, so it must not hit-test
        // the window. (A click whose press the worker reaches first is resolved
        // like any other — the saving is opportunistic, not guaranteed.)
        let now = Instant::now();
        let mut intent = pressed_at(1, false, now);
        absorb(&mut intent, Command::Release { generation: 1 });
        assert_eq!(next_step(intent, None, now), Next::Forget);
    }

    #[test]
    fn a_click_that_dragged_still_resolves_after_its_release() {
        // The cursor moved, so this may have been a window drag that ended
        // before the worker got to it; it is worth resolving.
        let now = Instant::now();
        let mut intent = pressed_at(1, false, now);
        absorb(
            &mut intent,
            Command::Drag {
                generation: 1,
                location: MOVED,
                chord: false,
            },
        );
        absorb(&mut intent, Command::Release { generation: 1 });
        assert_eq!(
            next_step(intent, None, now),
            Next::Resolve {
                generation: 1,
                press: PRESS
            }
        );
    }

    #[test]
    fn an_unarmed_drag_reads_the_frame_once_the_cursor_has_moved() {
        let now = Instant::now();
        let mut intent = pressed_at(1, false, now);
        absorb(
            &mut intent,
            Command::Drag {
                generation: 1,
                location: MOVED,
                chord: false,
            },
        );
        assert_eq!(
            next_step(intent, view(false, false, stale_check(now)), now),
            Next::CheckMoving
        );
    }

    #[test]
    fn an_unarmed_drag_waits_while_the_cursor_has_barely_moved() {
        let now = Instant::now();
        let mut intent = pressed_at(1, false, now);
        absorb(
            &mut intent,
            Command::Drag {
                generation: 1,
                location: (PRESS.0 + 1.0, PRESS.1),
                chord: false,
            },
        );
        assert_eq!(
            next_step(intent, view(false, false, stale_check(now)), now),
            Next::Wait
        );
    }

    #[test]
    fn an_unarmed_drag_waits_out_the_frame_read_throttle() {
        let now = Instant::now();
        let mut intent = pressed_at(1, false, now);
        absorb(
            &mut intent,
            Command::Drag {
                generation: 1,
                location: MOVED,
                chord: false,
            },
        );
        assert_eq!(next_step(intent, view(false, false, now), now), Next::Wait);
    }

    #[test]
    fn a_release_before_the_drag_armed_takes_one_last_frame_read() {
        // A drag quick enough to finish inside one throttle interval must still
        // be able to notice that the window moved, and snap.
        let now = Instant::now();
        let mut intent = pressed_at(1, false, now);
        absorb(
            &mut intent,
            Command::Drag {
                generation: 1,
                location: MOVED,
                chord: false,
            },
        );
        absorb(&mut intent, Command::Release { generation: 1 });
        assert_eq!(
            next_step(intent, view(false, false, now), now),
            Next::CheckMoving
        );
    }

    #[test]
    fn a_release_that_never_left_the_press_reads_nothing() {
        // The cursor did not move, so the window cannot have: no final frame
        // read, no snap.
        let now = Instant::now();
        let mut intent = pressed_at(1, false, now);
        absorb(&mut intent, Command::Release { generation: 1 });
        assert_eq!(
            next_step(intent, view(false, false, stale_check(now)), now),
            Next::Forget
        );
    }

    #[test]
    fn an_armed_drag_previews_the_newest_position_before_it_snaps() {
        // The drag moved and the release followed before the worker looked at
        // either: the zone that position selects is previewed first, so the drop
        // never lands somewhere the user was not shown.
        let now = Instant::now();
        let mut intent = pressed_at(1, false, now);
        absorb(
            &mut intent,
            Command::Drag {
                generation: 1,
                location: MOVED,
                chord: false,
            },
        );
        absorb(&mut intent, Command::Release { generation: 1 });
        assert_eq!(
            next_step(intent, view(true, true, stale_check(now)), now),
            Next::Preview
        );
    }

    #[test]
    fn a_release_snaps_to_the_zone_that_was_last_previewed() {
        // The release carries no position of its own, so a mouse-up that drifts
        // out of the edge band still snaps to the zone that was on screen.
        let now = Instant::now();
        let mut intent = pressed_at(1, false, now);
        absorb(&mut intent, Command::Release { generation: 1 });
        assert_eq!(
            next_step(intent, view(true, true, stale_check(now)), now),
            Next::Apply
        );
    }

    #[test]
    fn an_armed_release_over_no_zone_applies_nothing() {
        let now = Instant::now();
        let mut intent = pressed_at(1, false, now);
        absorb(&mut intent, Command::Release { generation: 1 });
        assert_eq!(
            next_step(intent, view(true, false, stale_check(now)), now),
            Next::Forget
        );
    }

    #[test]
    fn an_armed_drag_keeps_going_when_the_chord_comes_down() {
        // Once the window is following the drag, a modifier pressed mid-drag
        // does not hand it to drag-to-move — that gesture only claims a press.
        let now = Instant::now();
        let mut intent = pressed_at(1, false, now);
        absorb(
            &mut intent,
            Command::Drag {
                generation: 1,
                location: MOVED,
                chord: true,
            },
        );
        assert_eq!(
            next_step(intent, view(true, true, stale_check(now)), now),
            Next::Preview
        );
    }

    #[test]
    fn an_unarmed_drag_is_handed_over_when_the_chord_comes_down() {
        let now = Instant::now();
        let mut intent = pressed_at(1, false, now);
        absorb(
            &mut intent,
            Command::Drag {
                generation: 1,
                location: MOVED,
                chord: true,
            },
        );
        assert_eq!(
            next_step(intent, view(false, false, stale_check(now)), now),
            Next::Forget
        );
    }

    #[test]
    fn a_cancelled_press_is_dropped_whatever_it_had_reached() {
        let now = Instant::now();
        let mut intent = pressed_at(1, false, now);
        absorb(
            &mut intent,
            Command::Drag {
                generation: 1,
                location: MOVED,
                chord: false,
            },
        );
        absorb(&mut intent, Command::Cancel { generation: 1 });
        assert_eq!(
            next_step(intent, view(true, true, stale_check(now)), now),
            Next::Forget
        );
    }

    #[test]
    fn a_stale_session_is_replaced_by_the_newest_press() {
        // The previous drag's session is still around when a new press arrives:
        // the new one must be resolved, not driven from the old window.
        let now = Instant::now();
        assert_eq!(
            next_step(
                pressed_at(2, false, now),
                view(true, true, stale_check(now)),
                now
            ),
            Next::Resolve {
                generation: 2,
                press: PRESS
            }
        );
    }

    #[test]
    fn commands_from_a_superseded_press_are_ignored() {
        let mut intent = pressed_at(2, false, Instant::now());
        absorb(
            &mut intent,
            Command::Drag {
                generation: 1,
                location: MOVED,
                chord: false,
            },
        );
        absorb(&mut intent, Command::Cancel { generation: 1 });
        let current = intent.expect("the newest press stays");
        assert_eq!(current.generation, 2);
        assert_eq!(current.cursor, PRESS);
        assert!(!current.cursor_fresh);
        assert!(!current.cancelled);
    }

    #[test]
    fn a_drag_after_the_release_cannot_revive_the_press() {
        // The release is what ends a press; a stray drag behind it (a lost
        // mouse-up's leftovers) must not re-target anything.
        let mut intent = pressed_at(1, false, Instant::now());
        absorb(&mut intent, Command::Release { generation: 1 });
        absorb(
            &mut intent,
            Command::Drag {
                generation: 1,
                location: MOVED,
                chord: false,
            },
        );
        let current = intent.expect("the press is still the newest");
        assert!(current.released);
        assert_eq!(current.cursor, PRESS);
    }

    #[test]
    fn the_releases_final_frame_read_happens_only_once() {
        // Otherwise a release over a window that never moved would ask again
        // immediately — the throttle does not apply to a release — and spin on
        // Accessibility IPC forever.
        let now = Instant::now();
        let mut intent = pressed_at(1, false, now);
        absorb(
            &mut intent,
            Command::Drag {
                generation: 1,
                location: MOVED,
                chord: false,
            },
        );
        absorb(&mut intent, Command::Release { generation: 1 });
        let spent = Some(SessionView {
            generation: 1,
            armed: false,
            last_frame_check: now,
            final_check: true,
            has_target: false,
        });
        assert_eq!(next_step(intent, spent, now), Next::Forget);
    }

    #[test]
    fn a_press_the_worker_could_not_reach_in_time_is_dropped() {
        // Its coordinates may no longer sit over the window that was pressed, so
        // hit-testing them could grab (and snap) the wrong window.
        let now = Instant::now();
        let mut intent = pressed_at(1, false, now - PRESS_FRESHNESS - Duration::from_millis(1));
        absorb(
            &mut intent,
            Command::Drag {
                generation: 1,
                location: MOVED,
                chord: false,
            },
        );
        assert_eq!(next_step(intent, None, now), Next::Forget);
    }

    #[test]
    fn a_chord_held_once_keeps_the_press_handed_over() {
        // Folded as Press(chord) + Drag(no chord): the press was already
        // drag-to-move's, and letting the modifiers go must not claim it back.
        let now = Instant::now();
        let mut intent = pressed_at(1, true, now);
        absorb(
            &mut intent,
            Command::Drag {
                generation: 1,
                location: MOVED,
                chord: false,
            },
        );
        assert_eq!(next_step(intent, None, now), Next::Forget);
    }

    #[test]
    fn nothing_in_flight_is_nothing_to_do() {
        let now = Instant::now();
        assert_eq!(next_step(None, None, now), Next::Forget);
    }
}
