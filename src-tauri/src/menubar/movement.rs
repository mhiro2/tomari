//! Safe, best-effort movement of a foreign menu bar item.
//!
//! Accessibility exposes the item's frame but no move action. macOS does,
//! however, honor the public user gesture for rearranging status items: a drag
//! with Command held. This module synthesizes that gesture and restores the
//! cursor through a drop guard on every exit path.

use std::time::Duration;

use core_graphics::display::CGDisplay;
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton, EventField,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;

use super::status::ScanContext;
use super::{MenuBarItem, MenuBarItemZone};
use crate::eventtap::SYNTHETIC_MARKER;

const DROP_GAP: f64 = 3.0;
const DRAG_STEPS: usize = 16;
const MOVE_SETTLE: Duration = Duration::from_millis(20);
const PRESS_SETTLE: Duration = Duration::from_millis(40);
const DRAG_STEP_DELAY: Duration = Duration::from_millis(10);
const RELEASE_SETTLE: Duration = Duration::from_millis(180);

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
    display: CGDisplay,
    cleanup_up: CGEvent,
    cursor_hidden: bool,
    mouse_down: bool,
}

impl CursorRestore {
    fn new(original: CGPoint, cleanup_up: CGEvent) -> Self {
        let display = CGDisplay::displays_with_point(original, 1)
            .ok()
            .and_then(|(ids, count)| (count > 0).then(|| ids.into_iter().next()).flatten())
            .map(CGDisplay::new)
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
            display,
            cleanup_up,
            cursor_hidden,
            mouse_down: false,
        }
    }

    fn post(&mut self, event: &CGEvent, point: CGPoint) {
        event.post(CGEventTapLocation::HID);
        self.last = point;
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

impl Drop for CursorRestore {
    fn drop(&mut self) {
        if self.mouse_down {
            self.cleanup_up.set_location(self.last);
            self.cleanup_up.post(CGEventTapLocation::HID);
            self.mouse_down = false;
        }
        if let Err(code) = CGDisplay::warp_mouse_cursor_position(self.original) {
            tracing::warn!(
                code,
                "could not restore cursor position after menu bar drag"
            );
        }
        if self.cursor_hidden {
            if let Err(code) = self.display.show_cursor() {
                tracing::warn!(code, "could not show cursor after menu bar drag");
            }
            self.cursor_hidden = false;
        }
    }
}

/// Perform the public Command-drag gesture and leave cursor restoration to the
/// returned guard. The function refuses to interfere with a real mouse press.
pub(super) fn command_drag(
    item: &MenuBarItem,
    context: ScanContext,
    target: MenuBarItemZone,
) -> Result<CursorRestore, String> {
    let destination = destination(item, context, target)
        .ok_or_else(|| "menu bar item has no safe drop point".to_string())?;
    if unsafe {
        CGEventSourceButtonState(
            CGEventSourceStateID::CombinedSessionState,
            CGMouseButton::Left,
        )
    } {
        return Err("left mouse button is already pressed".to_string());
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
    let original = CGEvent::new(source.clone())
        .map_err(|()| "failed to read cursor position".to_string())?
        .location();
    let mut restore = CursorRestore::new(original, cleanup_up);

    restore.post(&move_event, start);
    std::thread::sleep(MOVE_SETTLE);
    restore.press(&down_event, start);
    std::thread::sleep(PRESS_SETTLE);
    for (event, point) in &drag_events {
        restore.post(event, *point);
        std::thread::sleep(DRAG_STEP_DELAY);
    }
    restore.release(&up_event, destination);
    std::thread::sleep(RELEASE_SETTLE);
    Ok(restore)
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
