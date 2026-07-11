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

use std::sync::atomic::AtomicUsize;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapOptions, CGEventType, CallbackResult};
use tauri::{AppHandle, Manager};
use tomari_core::Rect;
use tomari_window::{
    DragWindow, WindowHandle, drag_move_frame, drag_resize_frame, window_at_point,
};

use crate::locks::MutexExt;
use crate::state::AppState;
use crate::tap::{self, RunningTap};

/// The single live drag-to-move tap, owned globally like the other taps.
static MOVE_TAP: Mutex<Option<RunningTap>> = Mutex::new(None);

/// Which gesture a modifier chord selects.
#[derive(Clone, Copy, PartialEq, Eq)]
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
    *guard = None; // Drop stops the previous tap.

    if !drag_to_move_enabled_for(app) {
        return true;
    }

    match start(app.clone()) {
        Ok(tap) => {
            *guard = Some(tap);
            tracing::info!("drag-to-move event tap started");
            true
        }
        Err(e) => {
            tracing::warn!(error = %e, "drag-to-move event tap not started (grant Input Monitoring?)");
            false
        }
    }
}

fn drag_to_move_enabled_for(app: &AppHandle) -> bool {
    app.try_state::<AppState>()
        .map(|s| {
            let settings = s.settings.lock_safe();
            settings.window_management_enabled && settings.drag_to_move_enabled
        })
        .unwrap_or(false)
}

/// An in-flight move/resize: a handle to the [`MoveWorker`] driving the grabbed
/// window. The anchor (frame and cursor at press time) lives in the worker.
/// Ending the gesture closes the worker's channel (see [`MoveWorker::end`]) —
/// it does *not* join here; see the module doc comment on why joining is
/// deferred to the next gesture's mouse-down instead of this drag's teardown.
struct MoveDrag {
    worker: MoveWorker,
}

/// Off-thread applier for an in-flight move/resize.
///
/// The event-tap callback must never make an Accessibility call itself: this is
/// an active tap, so a synchronous AX write to a wedged target app would block
/// the callback and stall input system-wide until the OS disables the tap.
/// Instead the callback posts the latest cursor position here and returns at
/// once; this worker thread owns the window and applies the moves. It coalesces
/// any queued positions to the most recent before each AX write (so a slow
/// write never makes the window chase a backlog), and stops on the first failure
/// rather than hammering a dead or unresponsive target. Each write is itself
/// bounded by the AX messaging timeout set on the window element.
///
/// The *join*, however, is not bounded by anything: a wedged target app can
/// make the final in-flight write block for as long as the AX messaging
/// timeout allows. Doing that join synchronously — as a plain `Drop` impl
/// would, from inside the active tap's mouse-up callback — would stall input
/// system-wide for up to that long, exactly the failure mode this design
/// otherwise avoids. So ending a gesture ([`end`](MoveWorker::end)) only closes
/// the channel and hands back the bare `JoinHandle`; the caller
/// ([`DragToMoveState::end_drag`]) parks it in `pending_join` and only
/// actually joins it when the *next* gesture begins (mouse-down), which is
/// naturally off the hot path of the gesture that just ended and is not
/// itself time-critical the way a mouse-up ack is.
struct MoveWorker {
    /// Channel for the newest cursor position. `None` once dropped, which ends
    /// the worker's receive loop.
    tx: Option<Sender<(f64, f64)>>,
    thread: Option<JoinHandle<()>>,
}

impl MoveWorker {
    fn spawn(
        window: DragWindow,
        gesture: Gesture,
        start_frame: Rect,
        start_cursor: (f64, f64),
    ) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<(f64, f64)>();
        match std::thread::Builder::new()
            .name("tomari-dragmove-apply".into())
            .spawn(move || apply_loop(&window, gesture, start_frame, start_cursor, &rx))
        {
            Ok(thread) => Self {
                tx: Some(tx),
                thread: Some(thread),
            },
            // Spawn failed (resource exhaustion): keep no sender, so `post`
            // becomes a no-op rather than queueing into a channel nothing drains.
            // The window simply won't follow this drag.
            Err(e) => {
                tracing::warn!(error = %e, "could not start drag-to-move worker thread");
                Self {
                    tx: None,
                    thread: None,
                }
            }
        }
    }

    /// Hand the worker the newest cursor position. Non-blocking: if the worker
    /// has already stopped (window gone), the send simply fails and is ignored.
    fn post(&self, location: (f64, f64)) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(location);
        }
    }

    /// End the gesture: drop the sender so the worker's `recv` returns and its
    /// loop exits, and hand back its `JoinHandle` (if it started) for the
    /// caller to park and join later — never here, and never blocking. See the
    /// struct doc comment for why the join itself must not happen synchronously
    /// with the gesture ending.
    fn end(mut self) -> Option<JoinHandle<()>> {
        self.tx = None;
        self.thread.take()
    }
}

/// Apply coalesced cursor positions to the grabbed window until the channel
/// closes (the gesture ended) or an AX write fails (the window vanished or the
/// app stopped answering within the messaging timeout).
fn apply_loop(
    window: &DragWindow,
    gesture: Gesture,
    start_frame: Rect,
    start_cursor: (f64, f64),
    rx: &Receiver<(f64, f64)>,
) {
    while let Ok(mut location) = rx.recv() {
        // Coalesce: jump to the most recent queued position so the window never
        // chases a backlog of stale points behind a slow AX server.
        while let Ok(newer) = rx.try_recv() {
            location = newer;
        }
        let delta = (location.0 - start_cursor.0, location.1 - start_cursor.1);
        let result = match gesture {
            Gesture::Move => {
                let frame = drag_move_frame(start_frame, delta);
                window.set_origin(frame.x, frame.y)
            }
            Gesture::Resize => {
                let frame = drag_resize_frame(start_frame, delta);
                window.set_size(frame.width, frame.height)
            }
        };
        if let Err(e) = result {
            tracing::debug!(error = %e, "drag-to-move stopped: window no longer writable");
            break;
        }
    }
}

/// State the tap thread keeps across events: the in-flight gesture (if any) and
/// the previous gesture's worker thread, kept around only to be joined —
/// never blocking — at the start of the *next* gesture. See the [`MoveWorker`]
/// doc comment for why the join is deferred this way.
#[derive(Default)]
struct DragToMoveState {
    drag: Mutex<Option<MoveDrag>>,
    pending_join: Mutex<Option<JoinHandle<()>>>,
}

impl DragToMoveState {
    /// End the in-flight gesture (if any), parking its worker's `JoinHandle`
    /// to be reaped by the next call to [`Self::reap_pending`] rather than
    /// joined here. Called off the gesture's own critical path (mouse-up,
    /// tap-disabled recovery, a mid-drag feature toggle) so ending a gesture
    /// is always non-blocking.
    fn end_drag(&self) -> bool {
        let ended = self.drag.lock_safe().take();
        let Some(drag) = ended else {
            return false;
        };
        if let Some(handle) = drag.worker.end() {
            // Replace whatever was parked before. Dropping a `JoinHandle`
            // without joining it merely detaches the thread — it does not
            // block — so this stays non-blocking even if the previous
            // gesture's worker somehow has not been reaped yet (e.g. two
            // gestures ended back to back with no mouse-down in between to
            // reap the first). At most one handle is ever parked; the
            // detached thread still runs to completion on its own.
            *self.pending_join.lock_safe() = Some(handle);
        }
        true
    }

    /// Join a previously parked worker thread, if any. Called at the start of
    /// a new gesture (mouse-down) — naturally off the hot path of the gesture
    /// that just ended, and not itself time-critical the way acking a
    /// mouse-up promptly is. The join is still bounded: the worker's last
    /// write is timeout-bounded by the AX messaging timeout on the window
    /// element, so this cannot hang, only briefly delay the *new* gesture's
    /// own grab — never stall unrelated input system-wide the way joining
    /// from inside mouse-up would.
    fn reap_pending(&self) {
        if let Some(handle) = self.pending_join.lock_safe().take() {
            let _ = handle.join();
        }
    }
}

fn start(app: AppHandle) -> Result<RunningTap, String> {
    // An active tap (not listen-only): a gesture in flight returns `Drop` to
    // swallow the mouse events so the app underneath stays inert.
    tap::spawn(
        "tomari-dragtomove",
        "drag-to-move tap",
        CGEventTapOptions::Default,
        vec![
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseDragged,
            CGEventType::LeftMouseUp,
        ],
        move |port_holder| {
            // The drag state never leaves this thread: the callback runs only on
            // this run loop. The mutexes only satisfy the `Fn` bound.
            let state = DragToMoveState::default();
            Box::new(move |_proxy, etype, event: &CGEvent| {
                handle_event(&app, &state, &port_holder, etype, event)
            })
        },
    )
}

fn handle_event(
    app: &AppHandle,
    state: &DragToMoveState,
    port_holder: &Arc<AtomicUsize>,
    etype: CGEventType,
    event: &CGEvent,
) -> CallbackResult {
    // The system disabled the tap (timeout / heavy input): re-enable it, or
    // drag-to-move would silently stop working until the next settings change.
    if matches!(
        etype,
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
    ) {
        // We may have missed the matching mouse-up; end any in-flight grab so a
        // later press starts clean rather than resuming a stale gesture. This
        // parks the worker to be joined at the next gesture rather than here.
        state.end_drag();
        tap::reenable(port_holder);
        return CallbackResult::Keep;
    }

    let Some(app_state) = app.try_state::<AppState>() else {
        return CallbackResult::Keep;
    };
    let app_state = app_state.inner();

    handle_drag_to_move(app_state, state, etype, event)
}

/// Grab the window under the cursor when a gesture chord is held on mouse-down,
/// drive it on each drag, and release on mouse-up — consuming the mouse events
/// while a gesture is in flight so the app underneath never sees them.
fn handle_drag_to_move(
    app_state: &AppState,
    state: &DragToMoveState,
    etype: CGEventType,
    event: &CGEvent,
) -> CallbackResult {
    if !matches!(
        etype,
        CGEventType::LeftMouseDown | CGEventType::LeftMouseDragged | CGEventType::LeftMouseUp
    ) {
        return CallbackResult::Keep;
    }

    if !drag_to_move_enabled(app_state) {
        // Defensive: end any gesture left over if the feature was just disabled.
        state.end_drag();
        return CallbackResult::Keep;
    }

    match etype {
        CGEventType::LeftMouseDown => {
            // End any stale gesture first (a missed mouse-up could leave one in
            // flight) and reap whatever the *previous* gesture parked — bounded
            // by the AX messaging timeout, and off the time-critical path of
            // ending a gesture (see `DragToMoveState`), so it belongs here, not
            // at mouse-up.
            state.end_drag();
            state.reap_pending();

            // A gesture engages only when its exact chord is held; otherwise this
            // is an ordinary click and must pass through untouched.
            let Some(gesture) = gesture_for_flags(event.get_flags()) else {
                return CallbackResult::Keep;
            };
            let location = event.location();
            match grab(app_state, gesture, location.x, location.y) {
                Some(grabbed) => {
                    *state.drag.lock_safe() = Some(grabbed);
                    // Consume the press: the app underneath must not act on it.
                    CallbackResult::Drop
                }
                // Modifiers held but nothing draggable under the cursor (or no
                // permission): leave the click alone.
                None => CallbackResult::Keep,
            }
        }
        CGEventType::LeftMouseDragged => {
            let location = event.location();
            // Just hand the worker the latest cursor position: the AX write
            // happens off this thread, so a wedged target app can never stall
            // the active tap (which would delay input system-wide).
            let guard = state.drag.lock_safe();
            match guard.as_ref() {
                Some(d) => {
                    d.worker.post((location.x, location.y));
                    CallbackResult::Drop
                }
                None => CallbackResult::Keep,
            }
        }
        CGEventType::LeftMouseUp => {
            // End the gesture. If one was in flight we own the matching up, so
            // consume it; otherwise it belongs to a normal click — pass it on.
            // This only parks the worker's `JoinHandle`; the actual join happens
            // at the next gesture's mouse-down, never here, so a wedged target
            // app can never stall this ack (and, transitively, input
            // system-wide) for up to the AX messaging timeout.
            if state.end_drag() {
                CallbackResult::Drop
            } else {
                CallbackResult::Keep
            }
        }
        _ => CallbackResult::Keep,
    }
}

/// Whether drag-to-move should run: it shares the window-management master
/// switch and has its own opt-in toggle.
fn drag_to_move_enabled(app_state: &AppState) -> bool {
    let settings = app_state.settings.lock_safe();
    settings.window_management_enabled && settings.drag_to_move_enabled
}

/// Hit-test the window under the cursor on mouse-down and capture the anchor
/// state for the gesture. Quiet `None` when the permission is missing or there
/// is nothing draggable under the cursor.
fn grab(app_state: &AppState, gesture: Gesture, x: f64, y: f64) -> Option<MoveDrag> {
    if !app_state.windows.permission_granted() {
        return None;
    }
    // The hit-test and frame read run here on the tap thread, but both are now
    // bounded by the window element's AX messaging timeout, so even a wedged app
    // releases the press promptly. Every *subsequent* write is off-thread.
    let window = window_at_point(x, y).ok()?;
    let start_frame = window.frame().ok()?;
    Some(MoveDrag {
        worker: MoveWorker::spawn(window, gesture, start_frame, (x, y)),
    })
}
