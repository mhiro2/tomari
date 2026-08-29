//! Drag-to-move / drag-to-resize: hold a modifier chord and drag anywhere
//! inside the window under the pointer to move it (⌃⌥) or resize it (⌃⌥⌘) — no
//! need to grab the title bar or click to focus first.
//!
//! Unlike [`drag_to_snap`](crate::drag_to_snap), which only watches the OS move
//! a window and snaps on release, this tap *drives* the window itself, so it is
//! an **active** CGEventTap: while a gesture is in flight it consumes the mouse
//! events so the app underneath never sees the drag (no text selection, no
//! stray secondary-click from the held Control). A plain drag with none of the
//! gesture modifiers passes straight through, untouched.
//!
//! Listening requires *Input Monitoring*; moving the window requires
//! *Accessibility*.
//!
//! ## Nothing slow on the callback
//!
//! An active tap's callback holds up *all* input while it runs, Tomari's own or
//! not. So this one calls into no other process, starts no thread, joins none
//! and waits on nothing unbounded: it reads the event's own modifier flags and
//! two atomics ([`ENABLED`], [`ACCESSIBILITY`] — both mirrored for exactly this
//! reason), posts a [`Command`] into a bounded, coalescing queue
//! ([`crate::mailbox`]: lifecycle commands go lock-free, a cursor sample that
//! finds the slot contended is dropped rather than waited for), and returns.
//!
//! Everything that messages another process happens on the single
//! [applier thread](applier_loop) started with the tap — the hit-test that
//! finds the window, the frame read that anchors the drag, and every write
//! after it. One thread for the whole tap, not one per gesture, is what keeps
//! two gestures' Accessibility calls from overlapping: an ended gesture's last
//! write cannot still be landing while the next gesture reads its anchor,
//! because both are steps on the same thread. Each command carries the
//! generation of the gesture it belongs to, and the queue is drained before
//! *every* call, so anything superseded while it sat there is discarded before a
//! call is made rather than after. What that buys is precisely "no call is
//! *started* for a gesture already known to be over" — a release that arrives
//! after the drain, or while a call is already running, cannot cancel it.
//!
//! Consuming the press therefore commits before the hit-test has run, and a
//! chord press over nothing draggable is swallowed rather than passed on. The
//! alternative — pass the press through and start consuming once the hit-test
//! answers — would let the app underneath act on the press first, which is the
//! very thing the gesture exists to prevent. Ownership of the press is tracked
//! separately from whether a gesture is still being driven, so a press we
//! consumed still has its release consumed after a gesture is cut short — a
//! tap the system disabled mid-drag, say. The one case that cannot be covered
//! is the tap itself going away mid-press (the feature switched off while the
//! button is held): the callback is gone, so the release passes through with no
//! press behind it. Deferring teardown until the user lets go would block a
//! settings save on a human, which is worse than a stray release the app has no
//! tracking loop to hand it to.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTapOptions, CGEventTapPlacement, CGEventType, CallbackResult,
};
use tauri::{AppHandle, Manager};
use tomari_core::Rect;
use tomari_window::{
    DragWindow, WindowHandle, drag_move_frame, drag_resize_frame, window_at_point,
};

use crate::locks::MutexExt;
use crate::mailbox::{self, Coalesce, Receiver, Sender};
use crate::state::AppState;
use crate::tap::{self, RunningTap, TapHealth, TapHealthCell};

/// The single live drag-to-move tap, owned globally like the other taps.
static MOVE_TAP: Mutex<Option<RunningTap>> = Mutex::new(None);

/// Where this tap stands (see [`TapHealth`]); logged on every change.
static HEALTH: TapHealthCell = TapHealthCell::new("drag-to-move");

/// Whether the feature is on, mirrored out of the settings by
/// [`restart_result`] so the callback can check it with one atomic load instead
/// of taking the settings lock on every mouse event. Authoritative for the
/// callback only: [`restart_result`] reads the settings themselves, and every
/// path that can change either the master switch or this feature's own toggle
/// goes through it.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Whether the *Accessibility* permission is granted, mirrored so the callback
/// does not call into TCC. Refreshed by [`restart_result`] and by the permission
/// poller in `main`, whose interval bounds how stale it can be: within that
/// window a chord press may pass through when it could have been claimed, or be
/// claimed when nothing can be moved. Both are preferable to an OS call from a
/// callback that holds up all input.
static ACCESSIBILITY: AtomicBool = AtomicBool::new(false);

/// Publish the current *Accessibility* grant for the tap callback to read.
pub fn set_accessibility_granted(granted: bool) {
    ACCESSIBILITY.store(granted, Ordering::SeqCst);
}

/// Which gesture a modifier chord selects.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Gesture {
    /// ⌃⌥ + drag: move the window, keeping its size.
    Move,
    /// ⌃⌥⌘ + drag: resize the window from its bottom-right, top-left anchored.
    Resize,
}

/// The gesture an event's held modifiers select, or `None` when they are not
/// one of our exact chords. ⌃⌥ → move, ⌃⌥⌘ → resize; Shift must be released so
/// the chords stay distinct from other Shift-bearing shortcuts. Caps Lock and
/// fn are ignored so neither blocks a gesture.
pub fn gesture_for_flags(flags: CGEventFlags) -> Option<Gesture> {
    let ctrl = flags.contains(CGEventFlags::CGEventFlagControl);
    let alt = flags.contains(CGEventFlags::CGEventFlagAlternate);
    let cmd = flags.contains(CGEventFlags::CGEventFlagCommand);
    let shift = flags.contains(CGEventFlags::CGEventFlagShift);
    match (ctrl, alt, cmd, shift) {
        (true, true, false, false) => Some(Gesture::Move),
        (true, true, true, false) => Some(Gesture::Resize),
        _ => None,
    }
}

/// (Re)start the tap to match the current settings: tear down any existing tap
/// and, if drag-to-move is enabled, start a fresh one. Callers that do not need
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
    let mut guard = MOVE_TAP.lock_safe();
    // Published before the old tap goes down, so no reader sees `Healthy` over
    // a tap being torn down; also retires the old callback's generation.
    HEALTH.begin_start();
    *guard = None; // Drop stops the previous tap.

    if let Some(state) = app.try_state::<AppState>() {
        set_accessibility_granted(state.windows.permission_granted());
    }
    let enabled = drag_to_move_enabled_for(app);
    ENABLED.store(enabled, Ordering::SeqCst);
    if !enabled {
        HEALTH.set(TapHealth::Stopped);
        return true;
    }

    match start() {
        Ok(tap) => {
            *guard = Some(tap);
            // `Healthy` only once the handle is in place, so the state never
            // says "running" ahead of it.
            HEALTH.set(TapHealth::Healthy);
            tracing::info!("drag-to-move event tap started");
            true
        }
        Err(e) => {
            HEALTH.record_start_failure(crate::eventtap::input_monitoring_granted());
            tracing::warn!(error = %e, "drag-to-move event tap not started (grant Input Monitoring?)");
            false
        }
    }
}

/// Whether the drag-to-move tap is currently running. A cheap state read
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
    let _serialized = MOVE_TAP.lock_safe();
    matches!(
        HEALTH.state(),
        TapHealth::Healthy | TapHealth::DisabledByTimeout
    )
}

fn drag_to_move_enabled_for(app: &AppHandle) -> bool {
    app.try_state::<AppState>()
        .map(|s| {
            let settings = s.settings.lock_safe();
            settings.window_management_enabled && settings.drag_to_move_enabled
        })
        .unwrap_or(false)
}

/// One instruction from the callback to the applier thread. Every variant
/// carries the generation of the gesture it belongs to, so the applier can drop
/// anything belonging to a gesture that has since been superseded without
/// making an Accessibility call for it.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Command {
    /// A gesture began: find the window under `press` and start driving it.
    Begin {
        generation: u64,
        gesture: Gesture,
        press: (f64, f64),
    },
    /// The cursor moved while the gesture was in flight.
    Move {
        generation: u64,
        location: (f64, f64),
    },
    /// The gesture is over — released, interrupted, or torn down.
    End { generation: u64 },
}

impl Coalesce for Command {
    /// A newer cursor position for the same gesture replaces the pending one:
    /// only the latest matters, and it must never pile up behind a stalled
    /// applier. Begin and End are lifecycle: delivered always, on the
    /// lock-free path (see [`crate::mailbox`]).
    fn supersedes(&self, earlier: &Self) -> bool {
        matches!(
            (self, earlier),
            (Command::Move { generation: g, .. }, Command::Move { generation: h, .. }) if g == h
        )
    }

    fn sheddable(&self) -> bool {
        matches!(self, Command::Move { .. })
    }
}

/// What the applier knows about the newest gesture: what the callback has told
/// it, independent of whether the window has been resolved yet.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Intent {
    generation: u64,
    gesture: Gesture,
    press: (f64, f64),
    /// The newest cursor position, or `None` while the press has not moved —
    /// there is nothing to apply yet, but the window can already be resolved.
    cursor: Option<(f64, f64)>,
    /// The gesture has ended; nothing further may be applied for it.
    ended: bool,
}

/// Fold one command into the applier's picture of the newest gesture.
///
/// A `Begin` always replaces: generations only increase, so the newest one is
/// the only gesture that can still be driven. `Move` and `End` are matched on
/// generation, which is what makes a command left over from a superseded
/// gesture a no-op instead of a stray write.
fn absorb(intent: &mut Option<Intent>, command: Command) {
    match command {
        Command::Begin {
            generation,
            gesture,
            press,
        } => {
            *intent = Some(Intent {
                generation,
                gesture,
                press,
                cursor: None,
                ended: false,
            });
        }
        Command::Move {
            generation,
            location,
        } => {
            if let Some(current) = intent.as_mut().filter(|i| i.generation == generation) {
                current.cursor = Some(location);
            }
        }
        Command::End { generation } => {
            if let Some(current) = intent.as_mut().filter(|i| i.generation == generation) {
                current.ended = true;
            }
        }
    }
}

/// The window a gesture resolved to, and the frame it started from.
struct Driving {
    generation: u64,
    window: DragWindow,
    start_frame: Rect,
}

/// Resolve the window under `press`. This is the several-round-trip part — the
/// hit-test plus the walk to the owning window, then the frame read — each call
/// bounded by the element's AX messaging timeout, but plural. It runs here, on
/// the applier thread, and never on the callback.
fn resolve(generation: u64, press: (f64, f64)) -> Option<Driving> {
    let window = match window_at_point(press.0, press.1) {
        Ok(window) => window,
        // No permission, nothing draggable under the cursor, or the app did not
        // answer in time. The press was already consumed, so this gesture just
        // does nothing.
        Err(e) => {
            tracing::debug!(error = %e, "drag-to-move found nothing to drag under the cursor");
            return None;
        }
    };
    let start_frame = window.frame().ok()?;
    Some(Driving {
        generation,
        window,
        start_frame,
    })
}

/// The one thing the applier should do next, given what the callback has told it
/// and which gesture (if any) it has already resolved a window for. Pure, so the
/// ordering that decides whether an ended gesture can still get a write is
/// settled by tests rather than by scheduling.
#[derive(PartialEq, Debug)]
enum Next {
    /// Find the window under this press. Several Accessibility round-trips.
    Resolve { generation: u64, press: (f64, f64) },
    /// Apply this delta to the resolved window.
    Apply { gesture: Gesture, delta: (f64, f64) },
    /// Nothing to do until the callback says more.
    Wait,
    /// This gesture is over (or gone): let go of its window.
    Forget,
}

fn next_step(intent: Option<Intent>, resolved: Option<u64>) -> Next {
    match intent {
        // Nothing in flight, or the gesture ended — either way its window is no
        // longer ours to hold. The *intent* stays where it is: a stray `Move`
        // for an ended generation must remain a no-op rather than restart it.
        None => Next::Forget,
        Some(current) if current.ended => Next::Forget,
        // Not resolved, or resolved for a gesture this one replaced.
        Some(current) if resolved != Some(current.generation) => Next::Resolve {
            generation: current.generation,
            press: current.press,
        },
        Some(Intent {
            gesture,
            press,
            cursor: Some(cursor),
            ..
        }) => Next::Apply {
            gesture,
            delta: (cursor.0 - press.0, cursor.1 - press.1),
        },
        // Pressed but not yet moved, or the newest position already applied.
        Some(_) => Next::Wait,
    }
}

/// Whether [`step`] did something slow, and so should be preceded by a fresh
/// look at the queue before the next one.
#[derive(PartialEq, Debug)]
enum Step {
    /// An Accessibility call was made; the queue may have moved on since it
    /// started.
    Progressed,
    /// Nothing left to do for the gesture as it currently stands.
    Idle,
}

/// Carry out at most *one* Accessibility call towards the current intent.
fn step(intent: &mut Option<Intent>, driving: &mut Option<Driving>) -> Step {
    match next_step(*intent, driving.as_ref().map(|d| d.generation)) {
        Next::Wait => Step::Idle,
        Next::Forget => {
            *driving = None;
            Step::Idle
        }
        Next::Resolve { generation, press } => {
            *driving = resolve(generation, press);
            if driving.is_none() {
                *intent = None;
            }
            Step::Progressed
        }
        Next::Apply { gesture, delta } => {
            // Consumed: the same position must never be written twice.
            if let Some(pending) = intent.as_mut() {
                pending.cursor = None;
            }
            let Some(target) = driving.as_ref() else {
                return Step::Idle;
            };
            let result = match gesture {
                Gesture::Move => {
                    let frame = drag_move_frame(target.start_frame, delta);
                    target.window.set_origin(frame.x, frame.y)
                }
                Gesture::Resize => {
                    let frame = drag_resize_frame(target.start_frame, delta);
                    target.window.set_size(frame.width, frame.height)
                }
            };
            if let Err(e) = result {
                // The window vanished or the app stopped answering: stop rather
                // than hammering it for the rest of the drag.
                tracing::debug!(error = %e, "drag-to-move stopped: window no longer writable");
                *intent = None;
                *driving = None;
            }
            Step::Progressed
        }
    }
}

/// Drive gestures until the callback's sender is dropped (the tap is being torn
/// down).
///
/// The queue is drained before *every* Accessibility call, not once per batch:
/// resolving a window takes several round-trips, and a gesture that ended while
/// one was in flight must not then get a write from the cursor position it had
/// beforehand. That is also what makes the generation check worth having — a
/// gesture that began and ended entirely while an earlier call was running is
/// folded into the intent and discarded without a call of its own.
fn applier_loop(rx: &mut Receiver<Command>) {
    let mut intent: Option<Intent> = None;
    let mut driving: Option<Driving> = None;

    while let Some(first) = rx.recv() {
        absorb(&mut intent, first);
        loop {
            while let Some(next) = rx.try_recv() {
                absorb(&mut intent, next);
            }
            if step(&mut intent, &mut driving) == Step::Idle {
                break;
            }
        }
    }
}

/// State the tap keeps across events. Every field is either an atomic or only
/// touched through `&mut self` in [`Drop`], because the callback is `Fn`; none
/// of it needs a lock of its own, and none of its methods blocks: a cursor
/// sample that finds the queue's slot contended is dropped, and lifecycle
/// commands take a lock-free path (see [`crate::mailbox`]).
struct DragToMoveState {
    /// Commands to the applier. `None` when its thread could not be started, in
    /// which case no press is claimed — nothing could act on it.
    tx: Option<Sender<Command>>,
    applier: Option<JoinHandle<()>>,
    /// Bumped for every press we claim, so the applier can tell a command from
    /// this gesture from one left over from the last.
    generation: AtomicU64,
    /// Whether the current press was consumed and therefore owes us every event
    /// up to its release. Deliberately separate from whether the gesture is
    /// still being driven: a gesture can be cut short (the tap was disabled
    /// mid-drag, the feature was switched off) while the press it consumed must
    /// still swallow its own mouse-up.
    owning: AtomicBool,
}

impl Default for DragToMoveState {
    fn default() -> Self {
        let (tx, rx) = mailbox::channel::<Command>("drag-to-move");
        match std::thread::Builder::new()
            .name("tomari-dragmove-apply".into())
            .spawn(move || {
                let mut rx = rx;
                applier_loop(&mut rx)
            }) {
            Ok(applier) => Self::new(Some(tx), Some(applier)),
            Err(e) => {
                tracing::warn!(error = %e, "could not start the drag-to-move applier thread");
                Self::new(None, None)
            }
        }
    }
}

impl DragToMoveState {
    fn new(tx: Option<Sender<Command>>, applier: Option<JoinHandle<()>>) -> Self {
        Self {
            tx,
            applier,
            generation: AtomicU64::new(0),
            owning: AtomicBool::new(false),
        }
    }

    /// Post a command. Non-blocking, folded into the bounded queue (see
    /// [`crate::mailbox`]); returns whether the applier is there to receive it.
    fn send(&self, command: Command) -> bool {
        self.tx.as_ref().is_some_and(|tx| tx.send(command))
    }

    /// Start a gesture, claiming the press only if the applier actually took it.
    /// Returns whether it did: an applier that never started, or has since
    /// stopped, means nothing could move the window, so the press must be left
    /// to the app rather than swallowed for nothing.
    fn begin(&self, gesture: Gesture, press: (f64, f64)) -> bool {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        if !self.send(Command::Begin {
            generation,
            gesture,
            press,
        }) {
            return false;
        }
        self.owning.store(true, Ordering::SeqCst);
        true
    }

    /// Feed the in-flight gesture the newest cursor position.
    fn moved(&self, location: (f64, f64)) {
        self.send(Command::Move {
            generation: self.generation.load(Ordering::SeqCst),
            location,
        });
    }

    /// Whether a gesture could be driven at all — a cheap pre-check so an
    /// ordinary click is not even counted as a generation once the applier is
    /// gone. [`begin`](Self::begin) is the authority; this only avoids the
    /// pointless work.
    fn can_drive(&self) -> bool {
        self.tx.is_some()
    }

    /// Stop driving the current gesture, without giving up ownership of the
    /// press. Used wherever a gesture is cut short but its release has still to
    /// be swallowed.
    fn stop_gesture(&self) {
        self.send(Command::End {
            generation: self.generation.load(Ordering::SeqCst),
        });
    }

    fn owning(&self) -> bool {
        self.owning.load(Ordering::SeqCst)
    }

    /// Give up the press: nothing until the next mouse-down is ours.
    fn release(&self) {
        self.owning.store(false, Ordering::SeqCst);
    }
}

impl Drop for DragToMoveState {
    /// Join the applier before the tap callback itself is torn down, closing a
    /// join-order gap a bare `restart` would otherwise open: `restart_result`
    /// drops the previous `RunningTap` (which joins the tap *thread*, i.e.
    /// `CFRunLoopRun` returning) before starting the next tap. But that thread
    /// join says nothing about the applier, so without this a restart could let
    /// the new tap start, and a fresh gesture begin, while the old applier's
    /// last write was still in flight — racing the new gesture's writes to the
    /// same window.
    ///
    /// This runs when the callback closure is dropped, which happens on the tap
    /// thread right after `CFRunLoopRun` returns (see `tap::run_tap`) —
    /// strictly *before* `RunningTap::drop`'s `handle.join()` on that thread
    /// returns. So by the time `restart_result` can start the next tap, the old
    /// applier has exited.
    ///
    /// Ending the gesture *before* closing the channel is what keeps the wait
    /// short: the applier folds the `End` in with whatever cursor positions are
    /// still queued and applies none of them. The remaining wait is for a call
    /// already in flight, normally bounded by the AX messaging timeout — though
    /// that bound is best-effort (see `tomari_window`), so a target app that is
    /// both wedged and had its timeout rejected can still hold up a restart.
    /// Unlike the callback, a restart is not holding up anyone's keystrokes.
    fn drop(&mut self) {
        self.stop_gesture();
        self.tx = None;
        if let Some(applier) = self.applier.take() {
            let _ = applier.join();
        }
    }
}

fn start() -> Result<RunningTap, String> {
    // An active tap (not listen-only): a gesture in flight returns `Drop` to
    // swallow the mouse events so the app underneath stays inert.
    tap::spawn(
        "tomari-dragtomove",
        "drag-to-move tap",
        // Modifier remap and Hyper flags must be normalized before this tap
        // decides whether to claim and consume a pointer chord.
        CGEventTapPlacement::TailAppendEventTap,
        CGEventTapOptions::Default,
        vec![
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseDragged,
            CGEventType::LeftMouseUp,
        ],
        |port_holder| {
            // The state never leaves this thread: the callback runs only on this
            // run loop, and the applier it owns is joined when it drops.
            let state = DragToMoveState::default();
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
    state: &DragToMoveState,
    port_holder: &Arc<AtomicUsize>,
    generation: u64,
    etype: CGEventType,
    event: &CGEvent,
) -> CallbackResult {
    // The system disabled the tap (timeout / heavy input): re-enable it, or
    // drag-to-move would silently stop working until the next settings change.
    if matches!(
        etype,
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
    ) {
        // Events were missed, so stop driving — but keep ownership of the press:
        // if its mouse-up does arrive after the re-enable, it is still ours to
        // swallow, and if it never does the next mouse-down clears it.
        state.stop_gesture();
        HEALTH.record_disabled(generation);
        HEALTH.record_reenable(generation, tap::reenable(port_holder));
        return CallbackResult::Keep;
    }

    // Menu bar arrangement posts a real Command-drag through the HID stream.
    // It is intended for macOS, not either of Tomari's pointer gesture taps.
    if crate::eventtap::is_synthetic(event) {
        return CallbackResult::Keep;
    }

    handle_drag_to_move(state, etype, event)
}

/// Start a gesture when a chord is held on mouse-down, feed it the cursor on
/// each drag, and end it on mouse-up — consuming every event of a press we
/// claimed so the app underneath sees neither the drag nor a stray release.
fn handle_drag_to_move(
    state: &DragToMoveState,
    etype: CGEventType,
    event: &CGEvent,
) -> CallbackResult {
    match etype {
        CGEventType::LeftMouseDown => {
            // Whatever the last press left behind, this one starts clean: a
            // missed mouse-up must not let a finished gesture swallow an
            // ordinary click's drag and release.
            state.stop_gesture();
            state.release();

            // Every check here is local — the event's own flags and three
            // atomics. Nothing calls into another process or takes a lock; see
            // the module doc comment.
            if !ENABLED.load(Ordering::SeqCst) || !state.can_drive() {
                return CallbackResult::Keep;
            }
            // Nothing could be moved even if we consumed this, so don't.
            if !ACCESSIBILITY.load(Ordering::SeqCst) {
                return CallbackResult::Keep;
            }
            let Some(gesture) = gesture_for_flags(event.get_flags()) else {
                // An ordinary click; it must pass through untouched.
                return CallbackResult::Keep;
            };
            let location = event.location();
            if !state.begin(gesture, (location.x, location.y)) {
                // The applier is gone, so nothing could move a window: leave the
                // press to the app rather than swallow it for nothing.
                return CallbackResult::Keep;
            }
            // Consume the press, and with it every event up to its release. The
            // hit-test has not run yet, so this commits before knowing whether
            // anything under the cursor is draggable — see the module doc
            // comment for why that is the better of the two trades.
            CallbackResult::Drop
        }
        CGEventType::LeftMouseDragged => {
            if !state.owning() {
                return CallbackResult::Keep;
            }
            let location = event.location();
            state.moved((location.x, location.y));
            CallbackResult::Drop
        }
        CGEventType::LeftMouseUp => {
            if !state.owning() {
                // Belongs to an ordinary click; pass it on.
                return CallbackResult::Keep;
            }
            state.release();
            state.stop_gesture();
            CallbackResult::Drop
        }
        _ => CallbackResult::Keep,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRESS: (f64, f64) = (100.0, 200.0);

    /// A state wired to a plain channel with no applier thread, so the commands
    /// the callback posts can be read back exactly.
    fn wired() -> (DragToMoveState, Receiver<Command>) {
        let (tx, rx) = mailbox::channel("test");
        (DragToMoveState::new(Some(tx), None), rx)
    }

    fn drain(rx: &mut Receiver<Command>) -> Vec<Command> {
        std::iter::from_fn(|| rx.try_recv()).collect()
    }

    #[test]
    fn only_the_exact_chords_select_a_gesture() {
        let ctrl = CGEventFlags::CGEventFlagControl;
        let alt = CGEventFlags::CGEventFlagAlternate;
        let cmd = CGEventFlags::CGEventFlagCommand;
        let shift = CGEventFlags::CGEventFlagShift;
        assert_eq!(gesture_for_flags(ctrl | alt), Some(Gesture::Move));
        assert_eq!(gesture_for_flags(ctrl | alt | cmd), Some(Gesture::Resize));
        // Shift is deliberately excluded so the chords stay distinct.
        assert_eq!(gesture_for_flags(ctrl | alt | shift), None);
        assert_eq!(gesture_for_flags(ctrl), None);
        assert_eq!(gesture_for_flags(CGEventFlags::empty()), None);
    }

    #[test]
    fn a_gesture_posts_begin_move_and_end_under_one_generation() {
        let (state, mut rx) = wired();
        assert!(state.begin(Gesture::Move, PRESS));
        state.moved((110.0, 200.0));
        state.release();
        state.stop_gesture();
        assert_eq!(
            drain(&mut rx),
            vec![
                Command::Begin {
                    generation: 1,
                    gesture: Gesture::Move,
                    press: PRESS
                },
                Command::Move {
                    generation: 1,
                    location: (110.0, 200.0)
                },
                Command::End { generation: 1 },
            ]
        );
    }

    #[test]
    fn each_gesture_gets_its_own_generation() {
        let (state, mut rx) = wired();
        state.begin(Gesture::Move, PRESS);
        state.stop_gesture();
        state.begin(Gesture::Resize, PRESS);
        let generations: Vec<_> = drain(&mut rx)
            .into_iter()
            .map(|c| match c {
                Command::Begin { generation, .. }
                | Command::Move { generation, .. }
                | Command::End { generation } => generation,
            })
            .collect();
        assert_eq!(generations, vec![1, 1, 2]);
    }

    #[test]
    fn ownership_outlives_a_gesture_that_was_cut_short() {
        // A tap disabled mid-drag stops the gesture but must not hand the app a
        // mouse-up with no matching down.
        let (state, _rx) = wired();
        state.begin(Gesture::Move, PRESS);
        state.stop_gesture();
        assert!(state.owning(), "the press is still ours to finish");
        state.release();
        assert!(!state.owning());
    }

    #[test]
    fn a_press_is_not_claimed_without_an_applier() {
        let state = DragToMoveState::new(None, None);
        assert!(!state.can_drive());
        assert!(!state.begin(Gesture::Move, PRESS));
        assert!(!state.owning(), "an unclaimable press is left to the app");
    }

    #[test]
    fn a_press_is_not_claimed_once_the_applier_has_stopped() {
        // `can_drive` cannot see this: the sender is still there, but nothing is
        // draining it, so claiming the press would swallow it for nothing.
        let (state, rx) = wired();
        drop(rx);
        assert!(state.can_drive());
        assert!(!state.begin(Gesture::Move, PRESS));
        assert!(!state.owning());
    }

    #[test]
    fn absorb_keeps_only_the_newest_gesture() {
        let mut intent = None;
        absorb(
            &mut intent,
            Command::Begin {
                generation: 1,
                gesture: Gesture::Move,
                press: PRESS,
            },
        );
        absorb(
            &mut intent,
            Command::Begin {
                generation: 2,
                gesture: Gesture::Resize,
                press: (5.0, 6.0),
            },
        );
        assert_eq!(
            intent,
            Some(Intent {
                generation: 2,
                gesture: Gesture::Resize,
                press: (5.0, 6.0),
                cursor: None,
                ended: false,
            })
        );
    }

    #[test]
    fn absorb_ignores_commands_from_a_superseded_gesture() {
        // The whole point of the generation: a move or release left over from
        // the previous gesture must not drive the current one, nor end it.
        let mut intent = None;
        absorb(
            &mut intent,
            Command::Begin {
                generation: 7,
                gesture: Gesture::Move,
                press: PRESS,
            },
        );
        absorb(
            &mut intent,
            Command::Move {
                generation: 6,
                location: (1.0, 1.0),
            },
        );
        absorb(&mut intent, Command::End { generation: 6 });
        let current = intent.expect("still the newest gesture");
        assert_eq!(current.cursor, None);
        assert!(!current.ended);
    }

    #[test]
    fn absorb_takes_the_newest_cursor_and_the_end() {
        let mut intent = None;
        absorb(
            &mut intent,
            Command::Begin {
                generation: 1,
                gesture: Gesture::Move,
                press: PRESS,
            },
        );
        for x in [101.0, 102.0, 103.0] {
            absorb(
                &mut intent,
                Command::Move {
                    generation: 1,
                    location: (x, 200.0),
                },
            );
        }
        absorb(&mut intent, Command::End { generation: 1 });
        let current = intent.expect("the gesture is still the newest");
        assert_eq!(current.cursor, Some((103.0, 200.0)));
        assert!(current.ended);
    }

    fn intent(cursor: Option<(f64, f64)>, ended: bool) -> Intent {
        Intent {
            generation: 4,
            gesture: Gesture::Move,
            press: PRESS,
            cursor,
            ended,
        }
    }

    #[test]
    fn nothing_is_driven_without_an_intent() {
        assert_eq!(next_step(None, None), Next::Forget);
        // A window resolved for a gesture that is gone must be let go of.
        assert_eq!(next_step(None, Some(4)), Next::Forget);
    }

    #[test]
    fn an_ended_gesture_is_never_resolved_or_applied() {
        // The case that made this a pure function: the release can land while
        // the resolve it would have preceded is still in flight, and the
        // position the user had let go of must not then be written.
        assert_eq!(next_step(Some(intent(None, true)), None), Next::Forget);
        assert_eq!(
            next_step(Some(intent(Some((150.0, 200.0)), true)), Some(4)),
            Next::Forget
        );
    }

    #[test]
    fn a_gesture_resolves_before_it_can_be_applied() {
        assert_eq!(
            next_step(Some(intent(Some((150.0, 200.0)), false)), None),
            Next::Resolve {
                generation: 4,
                press: PRESS
            }
        );
    }

    #[test]
    fn a_window_resolved_for_an_older_gesture_is_resolved_again() {
        assert_eq!(
            next_step(Some(intent(None, false)), Some(3)),
            Next::Resolve {
                generation: 4,
                press: PRESS
            }
        );
    }

    #[test]
    fn a_resolved_gesture_waits_until_the_cursor_moves() {
        assert_eq!(next_step(Some(intent(None, false)), Some(4)), Next::Wait);
    }

    #[test]
    fn a_moved_cursor_becomes_a_delta_from_the_press() {
        assert_eq!(
            next_step(Some(intent(Some((150.0, 260.0)), false)), Some(4)),
            Next::Apply {
                gesture: Gesture::Move,
                delta: (50.0, 60.0)
            }
        );
        let resize = Intent {
            gesture: Gesture::Resize,
            ..intent(Some((90.0, 180.0)), false)
        };
        assert_eq!(
            next_step(Some(resize), Some(4)),
            Next::Apply {
                gesture: Gesture::Resize,
                delta: (-10.0, -20.0)
            }
        );
    }

    #[test]
    fn absorb_ignores_everything_before_the_first_begin() {
        // Commands can outlive the tap that produced them; none of them may
        // conjure a gesture out of nothing.
        let mut intent = None;
        absorb(
            &mut intent,
            Command::Move {
                generation: 1,
                location: PRESS,
            },
        );
        absorb(&mut intent, Command::End { generation: 1 });
        assert_eq!(intent, None);
    }
}
