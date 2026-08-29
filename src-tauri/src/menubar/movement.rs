//! Safe, best-effort movement of a foreign menu bar item.
//!
//! Accessibility exposes the item's frame but no move action. macOS does,
//! however, honor the public user gesture for rearranging status items: a drag
//! with Command held. This module synthesizes that gesture and restores the
//! cursor through a drop guard on every exit path.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use core_graphics::display::CGDisplay;
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
    CGMouseButton, CallbackResult, EventField,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;

use super::status::ScanContext;
use super::{MenuBarItem, MenuBarItemZone};
use crate::eventtap::{SYNTHETIC_MARKER, is_synthetic};
use crate::tap::{self, RunningTap};

const DROP_GAP: f64 = 3.0;
const DRAG_STEPS: usize = 16;
const PRESS_SETTLE: Duration = Duration::from_millis(40);
const DRAG_STEP_DELAY: Duration = Duration::from_millis(10);
const RELEASE_SETTLE: Duration = Duration::from_millis(180);

#[derive(Debug)]
pub(super) enum CommandDragError {
    TargetChanged,
    Unavailable(String),
}

impl std::fmt::Display for CommandDragError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TargetChanged => formatter.write_str("menu bar item changed before mouse-down"),
            Self::Unavailable(message) => formatter.write_str(message),
        }
    }
}

impl From<String> for CommandDragError {
    fn from(message: String) -> Self {
        Self::Unavailable(message)
    }
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceButtonState(state_id: CGEventSourceStateID, button: CGMouseButton) -> bool;
}

/// Owns cursor restoration and, while a gesture is active, the matching
/// mouse-up. The caller drops it immediately after the short post-release
/// settle and before the slower Accessibility verification scan.
pub(super) struct CursorRestore {
    original: CGPoint,
    last: CGPoint,
    /// The display the cursor started on, or `None` if none reported it (a
    /// display unplugged as the position was read) — a fallback case too.
    origin_display: Option<CGDisplay>,
    /// The display used to hide/show the cursor (the origin one, else main).
    display: CGDisplay,
    cleanup_up: CGEvent,
    cursor_hidden: bool,
    cursor_moved: bool,
    mouse_down: bool,
    /// Kept alive until the cursor is back where it belongs: `Drop::drop` runs
    /// the warp/show first and the fields drop after it, so physical input is
    /// watched for as long as the pointer is not the user's own.
    _interference: InterferenceGuard,
}

impl CursorRestore {
    fn new(original: CGPoint, cleanup_up: CGEvent, interference: InterferenceGuard) -> Self {
        let origin_display = CGDisplay::displays_with_point(original, 1)
            .ok()
            .and_then(|(ids, count)| (count > 0).then(|| ids.into_iter().next()).flatten())
            .map(CGDisplay::new);
        let display = origin_display
            .as_ref()
            .map(|d| CGDisplay::new(d.id))
            .unwrap_or_else(CGDisplay::main);
        let cursor_hidden = match display.hide_cursor() {
            Ok(()) => true,
            Err(code) => {
                tracing::warn!(code, "could not hide cursor during menu bar drag");
                false
            }
        };
        Self {
            original,
            last: original,
            origin_display,
            display,
            cleanup_up,
            cursor_hidden,
            cursor_moved: false,
            mouse_down: false,
            _interference: interference,
        }
    }

    fn interrupted(&self) -> bool {
        self._interference.interrupted()
    }

    fn post(&mut self, event: &CGEvent, point: CGPoint) {
        event.post(CGEventTapLocation::HID);
        self.last = point;
        self.cursor_moved = true;
    }

    fn press(&mut self, event: &CGEvent, point: CGPoint) {
        // Mark the press owned before posting. Even an unwind or future early
        // return immediately after the post must emit the prepared cleanup up.
        self.mouse_down = true;
        self.post(event, point);
    }

    fn release(&mut self, event: &CGEvent, point: CGPoint) {
        self.post(event, point);
        self.mouse_down = false;
    }
}

/// Where to put the cursor back. The point it started from — unless the display
/// it was on has gone away since (unplugged mid-drag), in which case warping
/// there would park the cursor off every screen or on whatever display now
/// happens to cover those coordinates; the main display's centre is the
/// fallback. Judged by the *identity* of the original display, not by whether
/// some display covers the point.
fn restore_target(origin_display: Option<&CGDisplay>, original: CGPoint) -> CGPoint {
    if origin_display.is_some_and(|display| display.is_active()) {
        return original;
    }
    let bounds = CGDisplay::main().bounds();
    CGPoint::new(
        bounds.origin.x + bounds.size.width / 2.0,
        bounds.origin.y + bounds.size.height / 2.0,
    )
}

impl Drop for CursorRestore {
    fn drop(&mut self) {
        if self.mouse_down {
            self.cleanup_up.set_location(self.last);
            self.cleanup_up.post(CGEventTapLocation::HID);
            self.mouse_down = false;
        }
        if self.cursor_moved {
            if let Err(code) = CGDisplay::warp_mouse_cursor_position(restore_target(
                self.origin_display.as_ref(),
                self.original,
            )) {
                tracing::warn!(
                    code,
                    "could not restore cursor position after menu bar drag"
                );
            }
            self.cursor_moved = false;
        }
        if self.cursor_hidden {
            if let Err(code) = self.display.show_cursor() {
                tracing::warn!(code, "could not show cursor after menu bar drag");
            }
            self.cursor_hidden = false;
        }
    }
}

/// Watches for *physical* input while the synthetic gesture runs. The gesture
/// takes a few hundred milliseconds during which the pointer is parked over a
/// foreign item; a real click landing then would go to that item at the warp
/// destination, and a real mouse-down would tangle with the synthetic release.
/// Checking the button state between steps only catches a press that is
/// still held, so a short-lived listen-only tap records any event that does
/// not carry Tomari's own marker — a click, a drag, a key — and the gesture
/// cancels at the next step, letting the drop guard release and restore.
///
/// Best-effort: the tap needs Input Monitoring; without it (or if the tap
/// fails to start) the gesture runs with the button-state checks alone, as it
/// always did.
struct InterferenceGuard {
    _tap: Option<RunningTap>,
    seen: Arc<AtomicBool>,
}

impl InterferenceGuard {
    fn start() -> Self {
        let seen = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&seen);
        let tap = tap::spawn(
            "tomari-menubar-guard",
            "menu bar drag guard",
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::ListenOnly,
            vec![
                CGEventType::LeftMouseDown,
                CGEventType::LeftMouseUp,
                CGEventType::LeftMouseDragged,
                CGEventType::RightMouseDown,
                CGEventType::RightMouseUp,
                CGEventType::RightMouseDragged,
                CGEventType::OtherMouseDown,
                CGEventType::OtherMouseUp,
                CGEventType::OtherMouseDragged,
                CGEventType::MouseMoved,
                CGEventType::ScrollWheel,
                CGEventType::KeyDown,
                CGEventType::KeyUp,
                CGEventType::FlagsChanged,
            ],
            move |_port_holder| {
                Box::new(move |_proxy, _etype, event: &CGEvent| {
                    // Nothing slow here: one marker read and one atomic store.
                    if !is_synthetic(event) {
                        flag.store(true, Ordering::SeqCst);
                    }
                    CallbackResult::Keep
                })
            },
        );
        let tap = match tap {
            Ok(tap) => Some(tap),
            Err(e) => {
                tracing::debug!(error = %e, "menu bar drag runs without the physical-input guard");
                None
            }
        };
        Self { _tap: tap, seen }
    }

    fn interrupted(&self) -> bool {
        self.seen.load(Ordering::SeqCst)
    }
}

fn interrupted_error() -> CommandDragError {
    CommandDragError::Unavailable(
        "physical input arrived during the menu bar drag; the move was cancelled".into(),
    )
}

/// Perform the public Command-drag gesture and leave cursor restoration to the
/// returned guard. The function refuses to interfere with a real mouse press,
/// and cancels — releasing and restoring through the guard — the moment
/// physical input is seen while the gesture is in flight.
pub(super) fn command_drag(
    item: &MenuBarItem,
    context: ScanContext,
    target: MenuBarItemZone,
    validate_before_move: impl FnOnce() -> bool,
) -> Result<CursorRestore, CommandDragError> {
    let destination = destination(item, context, target).ok_or_else(|| {
        CommandDragError::Unavailable("menu bar item has no safe drop point".into())
    })?;
    if left_button_pressed() {
        return Err(CommandDragError::Unavailable(
            "left mouse button is already pressed".into(),
        ));
    }

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|()| "failed to create menu bar drag event source".to_string())?;
    let start = CGPoint::new(item.position, item.center_y);
    // Prepare every event, including a spare release for the drop guard,
    // before posting the press. Allocation failure can therefore never strand
    // the system in a synthetic mouse-down state.
    let move_event = mouse_event(&source, CGEventType::MouseMoved, start)?;
    let down_event = mouse_event(&source, CGEventType::LeftMouseDown, start)?;
    let drag_events = (1..=DRAG_STEPS)
        .map(|step| {
            let progress = step as f64 / DRAG_STEPS as f64;
            let point = CGPoint::new(
                start.x + (destination.x - start.x) * progress,
                start.y + (destination.y - start.y) * progress,
            );
            mouse_event(&source, CGEventType::LeftMouseDragged, point).map(|event| (event, point))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let up_event = mouse_event(&source, CGEventType::LeftMouseUp, destination)?;
    let cleanup_up = mouse_event(&source, CGEventType::LeftMouseUp, destination)?;
    // Resolve labels and retain the exact AX element before entering this
    // function. This first frame/state check therefore leaves the person's
    // cursor untouched even if the target application responds slowly.
    if !validate_before_move() {
        return Err(CommandDragError::TargetChanged);
    }
    if left_button_pressed() {
        return Err(CommandDragError::Unavailable(
            "left mouse button was pressed before the drag could start".into(),
        ));
    }
    // Armed before the cursor position is read and before the pointer is
    // touched: starting the tap can take a moment, and a mouse moved meanwhile
    // must both count as interference and define where the cursor goes back.
    let interference = InterferenceGuard::start();
    let original = CGEvent::new(source.clone())
        .map_err(|()| "failed to read cursor position".to_string())?
        .location();
    if interference.interrupted() {
        return Err(interrupted_error());
    }
    let mut restore = CursorRestore::new(original, cleanup_up, interference);

    // Creating the guard hides but does not move the pointer. Check once more
    // here so a physical press that began during setup is rejected in place,
    // before any event can turn it into a drag over the foreign item.
    if left_button_pressed() {
        return Err(CommandDragError::Unavailable(
            "left mouse button was pressed before the drag could start".into(),
        ));
    }
    if restore.interrupted() {
        return Err(interrupted_error());
    }
    restore.post(&move_event, start);
    // Do not perform Accessibility work while the pointer is parked over a
    // foreign item. Event locations are explicit, so the synthetic down can
    // follow the move immediately after this last real-button check.
    if left_button_pressed() {
        return Err(CommandDragError::Unavailable(
            "left mouse button was pressed before the drag could start".into(),
        ));
    }
    if restore.interrupted() {
        return Err(interrupted_error());
    }
    restore.press(&down_event, start);
    std::thread::sleep(PRESS_SETTLE);
    for (event, point) in &drag_events {
        // A cancel here leaves the synthetic press owned by `restore`, whose
        // drop posts the matching release at the last point and restores the
        // cursor — the item is left wherever the partial drag put it, which
        // the caller's verification scan then reports.
        if restore.interrupted() {
            return Err(interrupted_error());
        }
        restore.post(event, *point);
        std::thread::sleep(DRAG_STEP_DELAY);
    }
    if restore.interrupted() {
        return Err(interrupted_error());
    }
    restore.release(&up_event, destination);
    std::thread::sleep(RELEASE_SETTLE);
    // Input during the release or its settle (or one the tap had not yet
    // handled at the last check) still contaminates the result: report it
    // rather than verify an interrupted gesture as a success.
    if restore.interrupted() {
        return Err(interrupted_error());
    }
    Ok(restore)
}

fn left_button_pressed() -> bool {
    unsafe {
        CGEventSourceButtonState(
            CGEventSourceStateID::CombinedSessionState,
            CGMouseButton::Left,
        )
    }
}

fn mouse_event(
    source: &CGEventSource,
    event_type: CGEventType,
    point: CGPoint,
) -> Result<CGEvent, String> {
    let event = CGEvent::new_mouse_event(source.clone(), event_type, point, CGMouseButton::Left)
        .map_err(|()| format!("failed to create {event_type:?} event"))?;
    event.set_flags(CGEventFlags::CGEventFlagCommand);
    event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, SYNTHETIC_MARKER);
    Ok(event)
}

fn destination(
    item: &MenuBarItem,
    context: ScanContext,
    target: MenuBarItemZone,
) -> Option<CGPoint> {
    if !item.width.is_finite() || item.width <= 0.0 || !item.center_y.is_finite() {
        return None;
    }
    let half_width = item.width / 2.0;
    let x = match target {
        MenuBarItemZone::Hidden => context.divider_left - DROP_GAP - half_width,
        MenuBarItemZone::Visible => context.divider_right + DROP_GAP + half_width,
    };
    (x - half_width >= context.screen_left && x + half_width <= context.screen_right)
        .then(|| CGPoint::new(x, item.center_y))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(width: f64) -> MenuBarItem {
        MenuBarItem {
            id: "1:0".to_string(),
            name: "Example".to_string(),
            owner_name: None,
            bundle_id: Some("com.example.item".to_string()),
            zone: MenuBarItemZone::Visible,
            position: 900.0,
            center_y: 12.0,
            width,
            owner_pid: 42,
            ax_identifier: Some("example".to_string()),
        }
    }

    fn context() -> ScanContext {
        ScanContext {
            divider_x: 800.0,
            divider_left: 794.0,
            divider_right: 806.0,
            screen_left: 0.0,
            screen_right: 1_000.0,
            menu_top: 0.0,
            menu_bottom: 24.0,
        }
    }

    #[test]
    fn destinations_clear_the_complete_divider_and_item_frames() {
        let item = item(20.0);
        let hidden = destination(&item, context(), MenuBarItemZone::Hidden).unwrap();
        let visible = destination(&item, context(), MenuBarItemZone::Visible).unwrap();

        assert_eq!(hidden.x, 781.0);
        assert!(hidden.x + item.width / 2.0 < context().divider_left);
        assert_eq!(visible.x, 819.0);
        assert!(visible.x - item.width / 2.0 > context().divider_right);
    }

    #[test]
    fn destination_rejects_invalid_or_offscreen_geometry() {
        assert!(destination(&item(0.0), context(), MenuBarItemZone::Hidden).is_none());

        let narrow = ScanContext {
            screen_left: 790.0,
            ..context()
        };
        assert!(destination(&item(20.0), narrow, MenuBarItemZone::Hidden).is_none());
    }

    #[test]
    fn synthesized_mouse_events_are_command_drags_marked_for_our_taps() {
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).unwrap();
        let event = mouse_event(
            &source,
            CGEventType::LeftMouseDragged,
            CGPoint::new(100.0, 12.0),
        )
        .unwrap();

        assert!(event.get_flags().contains(CGEventFlags::CGEventFlagCommand));
        assert_eq!(
            event.get_integer_value_field(EventField::EVENT_SOURCE_USER_DATA),
            SYNTHETIC_MARKER
        );
        assert!(crate::eventtap::is_synthetic(&event));
    }
}
