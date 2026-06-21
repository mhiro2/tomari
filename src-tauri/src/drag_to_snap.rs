//! Drag-to-snap: drag a window (no modifier) to a screen edge or corner and it
//! snaps when you let go. A listen-only CGEventTap on its own thread watches the
//! left button's press/drag/release; once the window is actually moving the drag
//! arms, previews the target zone while the cursor hugs a border, and applies it
//! on release.
//!
//! Listening requires *Input Monitoring*; moving the window requires
//! *Accessibility*.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use core_foundation::base::TCFType;
use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes};
use core_foundation_sys::mach_port::CFMachPortRef;
use core_graphics::event::{
    CGEvent, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
    CallbackResult,
};
use tauri::{AppHandle, Manager};
use tomari_core::{Rect, WindowPreset};
use tomari_window::{DragWindow, WindowHandle, compute_frame, edge_snap_preset, screen_at_cursor};

use crate::locks::MutexExt;
use crate::overlay;
use crate::state::AppState;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
}

/// The single live drag-to-snap tap, owned globally like the keyboard tap.
static DRAG_TAP: Mutex<Option<DragTap>> = Mutex::new(None);

pub struct DragTap {
    run_loop: CFRunLoop,
    thread: Option<JoinHandle<()>>,
}

impl Drop for DragTap {
    fn drop(&mut self) {
        self.run_loop.stop();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

/// (Re)start the tap to match the current settings: tear down any existing tap
/// and, if drag-to-snap is enabled, start a fresh one.
pub fn restart(app: &AppHandle) {
    let mut guard = DRAG_TAP.lock_safe();
    *guard = None; // Drop stops the previous tap.
    // Any snap preview on screen belongs to the tap we just dropped; a restart
    // (settings change, wake, permission grant) must not leave it stuck.
    overlay::hide(app);

    let enabled = app
        .try_state::<AppState>()
        .map(|s| {
            let settings = s.settings.lock_safe();
            settings.window_management_enabled && settings.drag_to_snap_enabled
        })
        .unwrap_or(false);
    if !enabled {
        return;
    }

    match start(app.clone()) {
        Ok(tap) => {
            *guard = Some(tap);
            tracing::info!("drag-to-snap event tap started");
        }
        Err(e) => {
            tracing::warn!(error = %e, "drag-to-snap event tap not started (grant Input Monitoring?)")
        }
    }
}

/// An in-flight drag-to-snap: the window grabbed on mouse-down plus the live
/// preview state. The window is not snapped until release; until then this only
/// reads the cursor and shows the preview.
struct DragSnap {
    /// The window under the cursor when the button went down.
    window: DragWindow,
    /// Its frame at that moment, used to confirm the OS is actually *moving* it
    /// (a title-bar drag) before arming — text selection and other drags leave
    /// the frame put and never arm.
    start_frame: Rect,
    armed: bool,
    /// Every display's `(full_frame, work_area)` (CG), snapshotted once on arm
    /// so edge detection on later moves stays a pure, lock-free computation.
    screens: Vec<(Rect, Rect)>,
    /// The preset the cursor currently selects together with the work area to
    /// lay it out in. Stored as one value (not just the preset) so that dragging
    /// between displays whose edges map to the *same* preset — e.g. the top of
    /// display A to the top of display B, both `Maximize` — still re-targets the
    /// preview, and so the drop snaps to the last previewed zone even if the
    /// cursor slips off every display at the moment of release.
    active: Option<(WindowPreset, Rect)>,
}

/// How far (points) the dragged window's origin must move from its mouse-down
/// frame before a drag counts as a real window move and arms drag-to-snap.
const MOVE_EPSILON: f64 = 1.0;

fn start(app: AppHandle) -> Result<DragTap, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("tomari-dragtosnap".into())
        .spawn(move || run_tap(app, tx))
        .map_err(|e| e.to_string())?;

    match rx.recv() {
        Ok(Ok(run_loop)) => Ok(DragTap {
            run_loop,
            thread: Some(thread),
        }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(e) => Err(format!(
            "drag-to-snap tap thread exited before signalling: {e}"
        )),
    }
}

fn run_tap(app: AppHandle, tx: Sender<Result<CFRunLoop, String>>) {
    // The drag state never leaves this thread: the callback runs only on this
    // run loop. The mutex only satisfies the `Fn` bound.
    let drag: Mutex<Option<DragSnap>> = Mutex::new(None);
    let port_holder = Arc::new(AtomicUsize::new(0));

    let callback = {
        let port_holder = port_holder.clone();
        move |_proxy, etype, event: &CGEvent| handle_event(&app, &drag, &port_holder, etype, event)
    };

    let tap = match CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::ListenOnly,
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
                "failed to create drag-to-snap tap — Input Monitoring permission required".into(),
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
                "failed to create run-loop source for drag-to-snap tap".into()
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
    drag: &Mutex<Option<DragSnap>>,
    port_holder: &Arc<AtomicUsize>,
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
        // otherwise leave a snap preview stranded. Drop any in-flight state and
        // clear the preview before re-arming.
        if drag.lock_safe().take().is_some() {
            overlay::hide(app);
        }
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

    handle_drag_to_snap(app, app_state, drag, etype, event)
}

/// Watch a plain drag and snap the window to a screen edge or corner on
/// release: grab on mouse-down, arm once the window is actually moving, preview
/// the target while the cursor hugs a border, and apply it on mouse-up.
fn handle_drag_to_snap(
    app: &AppHandle,
    app_state: &AppState,
    drag: &Mutex<Option<DragSnap>>,
    etype: CGEventType,
    event: &CGEvent,
) -> CallbackResult {
    // Only the press/drag/release of the left button drive snapping.
    if !matches!(
        etype,
        CGEventType::LeftMouseDown | CGEventType::LeftMouseDragged | CGEventType::LeftMouseUp
    ) {
        return CallbackResult::Keep;
    }

    if !drag_to_snap_enabled(app_state) {
        // Defensive: drop any state left over if the feature was just disabled.
        if drag.lock_safe().take().is_some() {
            overlay::hide(app);
        }
        return CallbackResult::Keep;
    }

    match etype {
        CGEventType::LeftMouseDown => {
            // A new press starts fresh: if a previous drag was abandoned with a
            // preview still up (e.g. a mouse-up lost while the tap was off),
            // clear it before overwriting the state.
            let stale_preview = {
                let mut guard = drag.lock_safe();
                let stale = guard.as_ref().is_some_and(|d| d.active.is_some());
                // A drag-to-move/resize chord (⌃⌥ / ⌃⌥⌘) is held: that gesture
                // owns this drag and is driving the window itself, so do not also
                // arm an edge snap, which would flash a preview against the move.
                if crate::drag_to_move::gesture_for_flags(event.get_flags()).is_some() {
                    *guard = None;
                } else {
                    let location = event.location();
                    *guard = grab_drag_candidate(app_state, location.x, location.y);
                }
                stale
            };
            if stale_preview {
                overlay::hide(app);
            }
        }
        CGEventType::LeftMouseDragged => {
            let location = event.location();
            let (x, y) = (location.x, location.y);
            let mut guard = drag.lock_safe();
            let Some(d) = guard.as_mut() else {
                return CallbackResult::Keep;
            };

            if !d.armed {
                // The press carried no gesture chord, but one came down before
                // this drag armed: it now belongs to drag-to-move, so abandon
                // the snap candidate rather than arm a competing preview.
                if crate::drag_to_move::gesture_for_flags(event.get_flags()).is_some() {
                    *guard = None;
                    return CallbackResult::Keep;
                }
                match d.window.frame() {
                    Ok(now) => {
                        let moved = (now.x - d.start_frame.x).abs() > MOVE_EPSILON
                            || (now.y - d.start_frame.y).abs() > MOVE_EPSILON;
                        if !moved {
                            // Dragging something that is not moving the window
                            // (text selection, a control): leave it alone.
                            return CallbackResult::Keep;
                        }
                        d.armed = true;
                        // Read the geometry cached on the main thread (kept
                        // current by the display-change observer in `displays`);
                        // never block the tap thread for it.
                        d.screens = app_state.screen_geometry();
                    }
                    Err(_) => {
                        // The window went away; abandon this drag.
                        *guard = None;
                        return CallbackResult::Keep;
                    }
                }
            }

            // Armed: resolve the target purely from the cursor and the snapshot.
            let target = screen_at_cursor(&d.screens, x, y).and_then(|(full, visible)| {
                edge_snap_preset((x, y), full).map(|preset| (preset, visible))
            });
            // Compare the whole target, not just the preset: moving between
            // displays whose edges share a preset must still re-target.
            if target != d.active {
                d.active = target;
                match target {
                    Some((preset, visible)) => overlay::show(app, compute_frame(preset, visible)),
                    None => overlay::hide(app),
                }
            }
        }
        CGEventType::LeftMouseUp => {
            let dropped = drag.lock_safe().take();
            if let Some(d) = dropped {
                overlay::hide(app);
                if d.armed
                    && let Some((preset, visible)) = d.active
                {
                    crate::window_ops::apply_dragged(
                        app_state,
                        &d.window,
                        compute_frame(preset, visible),
                    );
                }
            }
        }
        _ => {}
    }

    CallbackResult::Keep
}

/// Whether drag-to-snap should run: it shares the window-management master
/// switch and has its own opt-in toggle.
fn drag_to_snap_enabled(app_state: &AppState) -> bool {
    let settings = app_state.settings.lock_safe();
    settings.window_management_enabled && settings.drag_to_snap_enabled
}

/// Hit-test the window under the cursor on mouse-down for a possible
/// drag-to-snap, recording its frame so a later move can tell a real window
/// drag from text selection. Quiet `None` when the permission is missing or
/// nothing draggable is under the cursor.
fn grab_drag_candidate(app_state: &AppState, x: f64, y: f64) -> Option<DragSnap> {
    if !app_state.windows.permission_granted() {
        return None;
    }
    let window = tomari_window::window_at_point(x, y).ok()?;
    let start_frame = window.frame().ok()?;
    Some(DragSnap {
        window,
        start_frame,
        armed: false,
        screens: Vec::new(),
        active: None,
    })
}
