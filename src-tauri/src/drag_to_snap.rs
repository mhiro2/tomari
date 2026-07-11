//! Drag-to-snap: drag a window (no modifier) to a screen edge or corner and it
//! snaps when you let go. A listen-only CGEventTap on its own thread watches the
//! left button's press/drag/release; once the window is actually moving the drag
//! arms, previews the target zone while the cursor hugs a border, and applies it
//! on release.
//!
//! Listening requires *Input Monitoring*; moving the window requires
//! *Accessibility*.

use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use core_graphics::event::{CGEvent, CGEventTapOptions, CGEventType, CallbackResult};
use tauri::{AppHandle, Manager};
use tomari_core::{Rect, WindowPreset};
use tomari_window::{DragWindow, WindowHandle, compute_frame, edge_snap_preset, screen_at_cursor};

use crate::locks::MutexExt;
use crate::overlay;
use crate::state::AppState;
use crate::tap::{self, RunningTap};

/// The single live drag-to-snap tap, owned globally like the keyboard tap.
static DRAG_TAP: Mutex<Option<RunningTap>> = Mutex::new(None);

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
        return true;
    }

    match start(app.clone()) {
        Ok(tap) => {
            *guard = Some(tap);
            tracing::info!("drag-to-snap event tap started");
            true
        }
        Err(e) => {
            tracing::warn!(error = %e, "drag-to-snap event tap not started (grant Input Monitoring?)");
            false
        }
    }
}

/// Whether the drag-to-snap tap is currently running. A cheap lock-and-check
/// so `save_settings` can verify on *every* save that an enabled feature
/// actually has its tap alive — a warning must reflect the live state, not
/// just the last restart attempt, or it would vanish from the UI on the next
/// unrelated save while the tap is still dead.
pub fn is_running() -> bool {
    DRAG_TAP.lock_safe().is_some()
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
    /// The cursor location at mouse-down, so later drag events can tell whether
    /// the cursor itself has moved far enough to be worth an AX frame read at
    /// all — cheap and lock-free, unlike the frame read it gates.
    start_cursor: (f64, f64),
    /// When `start_frame` was last confirmed against a fresh AX read while
    /// unarmed, so repeated drag events (60-120 Hz) throttle to at most one AX
    /// call per [`FRAME_CHECK_INTERVAL`] instead of one per event — a slow
    /// or wedged target app must not be hammered with AX IPC just because the
    /// user is dragging a text selection over it.
    last_frame_check: Instant,
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

/// How far (points) the cursor must move from its mouse-down location before an
/// unarmed drag is even worth an AX frame read. Below this, the pointer is
/// essentially where it was pressed (a click, a tiny jitter) and the window
/// could not plausibly have moved past `MOVE_EPSILON` yet — skip the AX call
/// entirely rather than confirm a foregone conclusion.
const CURSOR_MOVE_EPSILON: f64 = 2.0;

/// Minimum time between AX frame reads while a drag is unarmed. Mouse-dragged
/// events arrive at 60-120 Hz; without this an unarmed drag (e.g. a text
/// selection over another app) would perform an AX IPC round trip on every
/// single one of them, which can trip that app's own responsiveness and, if it
/// is slow to answer, risk the tap's own timeout-disable. Once armed this no
/// longer applies — armed drags resolve purely from the cached cursor/screen
/// geometry and never read the window's frame again.
const FRAME_CHECK_INTERVAL: Duration = Duration::from_millis(50);

fn start(app: AppHandle) -> Result<RunningTap, String> {
    tap::spawn(
        "tomari-dragtosnap",
        "drag-to-snap tap",
        CGEventTapOptions::ListenOnly,
        vec![
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseDragged,
            CGEventType::LeftMouseUp,
        ],
        move |port_holder| {
            // The drag state never leaves this thread: the callback runs only on
            // this run loop. The mutex only satisfies the `Fn` bound.
            let drag: Mutex<Option<DragSnap>> = Mutex::new(None);
            Box::new(move |_proxy, etype, event: &CGEvent| {
                handle_event(&app, &drag, &port_holder, etype, event)
            })
        },
    )
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
        tap::reenable(port_holder);
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

                // Cheap, lock-free prefilter: the cursor itself must have moved
                // before the window could plausibly have moved past
                // `MOVE_EPSILON`. Most drag-tap traffic while unarmed is a text
                // selection or a control drag whose cursor still wanders even
                // though nothing about the window changes, so this alone does
                // not replace the AX check below — it only skips it when the
                // cursor has barely moved from mouse-down at all.
                let cursor_moved = (x - d.start_cursor.0).abs() > CURSOR_MOVE_EPSILON
                    || (y - d.start_cursor.1).abs() > CURSOR_MOVE_EPSILON;
                if !cursor_moved {
                    return CallbackResult::Keep;
                }

                // Time-throttle the AX round trip itself: even with the cursor
                // moving, do not read the frame more than once per
                // `FRAME_CHECK_INTERVAL` so a heavy run of drag events never
                // turns into a matching run of AX IPC calls against whatever app
                // is under the cursor.
                let now_instant = Instant::now();
                if now_instant.duration_since(d.last_frame_check) < FRAME_CHECK_INTERVAL {
                    return CallbackResult::Keep;
                }
                d.last_frame_check = now_instant;

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
        start_cursor: (x, y),
        // Backdated by a full interval, not `Instant::now()`: the first drag
        // event whose cursor has already cleared `CURSOR_MOVE_EPSILON` must be
        // allowed an immediate AX read. A `now()` baseline would instead make
        // that very first check appear recent and throttle it away, so a quick
        // drag (grab, move past the edge, release, all within
        // `FRAME_CHECK_INTERVAL`) could complete without ever reading the frame
        // once and would never arm.
        last_frame_check: Instant::now() - FRAME_CHECK_INTERVAL,
        armed: false,
        screens: Vec::new(),
        active: None,
    })
}
