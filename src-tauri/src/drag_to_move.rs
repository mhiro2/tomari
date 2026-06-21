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

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use core_foundation::base::TCFType;
use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes};
use core_foundation_sys::mach_port::CFMachPortRef;
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventType, CallbackResult,
};
use tauri::{AppHandle, Manager};
use tomari_core::Rect;
use tomari_window::{
    DragWindow, WindowHandle, drag_move_frame, drag_resize_frame, window_at_point,
};

use crate::locks::MutexExt;
use crate::state::AppState;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
}

/// The single live drag-to-move tap, owned globally like the other taps.
static MOVE_TAP: Mutex<Option<MoveTap>> = Mutex::new(None);

pub struct MoveTap {
    run_loop: CFRunLoop,
    thread: Option<JoinHandle<()>>,
}

impl Drop for MoveTap {
    fn drop(&mut self) {
        self.run_loop.stop();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

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
/// and, if drag-to-move is enabled, start a fresh one.
pub fn restart(app: &AppHandle) {
    let mut guard = MOVE_TAP.lock_safe();
    *guard = None; // Drop stops the previous tap.

    if !drag_to_move_enabled_for(app) {
        return;
    }

    match start(app.clone()) {
        Ok(tap) => {
            *guard = Some(tap);
            tracing::info!("drag-to-move event tap started");
        }
        Err(e) => {
            tracing::warn!(error = %e, "drag-to-move event tap not started (grant Input Monitoring?)")
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
/// window. The anchor (frame and cursor at press time) lives in the worker;
/// dropping this ends the gesture (closes the worker's channel and joins it).
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
}

impl Drop for MoveWorker {
    fn drop(&mut self) {
        // Drop the sender first so the worker's `recv` returns and the loop
        // exits, then join so a final in-flight (timeout-bounded) AX write
        // finishes before the next gesture can start on the same window.
        self.tx = None;
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
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

fn start(app: AppHandle) -> Result<MoveTap, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("tomari-dragtomove".into())
        .spawn(move || run_tap(app, tx))
        .map_err(|e| e.to_string())?;

    match rx.recv() {
        Ok(Ok(run_loop)) => Ok(MoveTap {
            run_loop,
            thread: Some(thread),
        }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(e) => Err(format!(
            "drag-to-move tap thread exited before signalling: {e}"
        )),
    }
}

fn run_tap(app: AppHandle, tx: Sender<Result<CFRunLoop, String>>) {
    // The drag state never leaves this thread: the callback runs only on this
    // run loop. The mutex only satisfies the `Fn` bound.
    let drag: Mutex<Option<MoveDrag>> = Mutex::new(None);
    let port_holder = Arc::new(AtomicUsize::new(0));

    let callback = {
        let port_holder = port_holder.clone();
        move |_proxy, etype, event: &CGEvent| handle_event(&app, &drag, &port_holder, etype, event)
    };

    // An active tap (not listen-only): a gesture in flight returns `Drop` to
    // swallow the mouse events so the app underneath stays inert.
    let tap = match CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        vec![
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseDragged,
            CGEventType::LeftMouseUp,
        ],
        callback,
    ) {
        Ok(tap) => tap,
        Err(()) => {
            let _ = tx.send(Err(
                "failed to create drag-to-move tap — Input Monitoring permission required".into(),
            ));
            return;
        }
    };

    // Publish the mach port so the callback can re-arm the tap if the system
    // disables it (AX updates inside the callback can be slow enough).
    port_holder.store(
        tap.mach_port().as_concrete_TypeRef() as usize,
        Ordering::SeqCst,
    );

    let source = match tap.mach_port().create_runloop_source(0) {
        Ok(source) => source,
        Err(()) => {
            let _ = tx.send(Err(
                "failed to create run-loop source for drag-to-move tap".into()
            ));
            return;
        }
    };

    let run_loop = CFRunLoop::get_current();
    unsafe {
        run_loop.add_source(&source, kCFRunLoopCommonModes);
    }
    tap.enable();

    let _ = tx.send(Ok(run_loop));
    CFRunLoop::run_current();
    // Run loop stopped: returning here drops `tap`, invalidating the port.
}

fn handle_event(
    app: &AppHandle,
    drag: &Mutex<Option<MoveDrag>>,
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
        // later press starts clean rather than resuming a stale gesture. Take it
        // out before dropping (off-lock) so the worker join is not held under the
        // mutex.
        let stale = drag.lock_safe().take();
        drop(stale);
        let port = port_holder.load(Ordering::SeqCst) as CFMachPortRef;
        if !port.is_null() {
            unsafe { CGEventTapEnable(port, true) };
        }
        return CallbackResult::Keep;
    }

    let Some(app_state) = app.try_state::<AppState>() else {
        return CallbackResult::Keep;
    };
    let app_state = app_state.inner();

    handle_drag_to_move(app_state, drag, etype, event)
}

/// Grab the window under the cursor when a gesture chord is held on mouse-down,
/// drive it on each drag, and release on mouse-up — consuming the mouse events
/// while a gesture is in flight so the app underneath never sees them.
fn handle_drag_to_move(
    app_state: &AppState,
    drag: &Mutex<Option<MoveDrag>>,
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
        // Take it out before dropping (off-lock) so the worker join never runs
        // while the mutex is held.
        let stale = drag.lock_safe().take();
        drop(stale);
        return CallbackResult::Keep;
    }

    match etype {
        CGEventType::LeftMouseDown => {
            // End any stale gesture first — off-lock, since dropping its worker
            // joins (a missed mouse-up could leave one in flight).
            let stale = drag.lock_safe().take();
            drop(stale);

            // A gesture engages only when its exact chord is held; otherwise this
            // is an ordinary click and must pass through untouched.
            let Some(gesture) = gesture_for_flags(event.get_flags()) else {
                return CallbackResult::Keep;
            };
            let location = event.location();
            match grab(app_state, gesture, location.x, location.y) {
                Some(grabbed) => {
                    *drag.lock_safe() = Some(grabbed);
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
            let guard = drag.lock_safe();
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
            // `ended` drops at the end of this arm (off-lock), joining the worker.
            let ended = drag.lock_safe().take();
            if ended.is_some() {
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
