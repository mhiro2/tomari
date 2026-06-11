//! The [`WindowManager`] abstraction. Concrete implementations resolve the
//! focused window to a [`WindowHandle`] and report display work areas; the
//! snapping/cycling orchestration lives in the application layer and the pure
//! placement math in [`geometry`](crate::geometry).

use std::sync::{Arc, Mutex};

use tomari_core::domain::window::{DisplayDirection, Rect};

use crate::error::Result;

/// A handle to one specific window that can be read and repositioned later,
/// even after focus has moved elsewhere — the unit the undo history stores.
pub trait WindowHandle: Send {
    /// The window's current frame (points, top-left origin). Fails when the
    /// window no longer exists.
    fn frame(&self) -> Result<Rect>;

    /// Move and resize this window to exactly `frame`.
    fn set_frame(&self, frame: Rect) -> Result<()>;

    /// A hash that is stable across handles to the same window, so two
    /// resolutions of the focused window can be compared for identity.
    fn stable_hash(&self) -> u64;
}

/// Something that can resolve the focused window and describe the displays.
pub trait WindowManager {
    /// Whether the OS-level permission required to move windows is granted.
    fn permission_granted(&self) -> bool;

    /// A handle to the currently focused window.
    fn focused_window(&self) -> Result<Box<dyn WindowHandle>>;

    /// The usable area of the display containing `window_frame` (the full
    /// display minus the menu bar / Dock / notch).
    fn work_area(&self, window_frame: Rect) -> Result<Rect>;

    /// The usable area of every display, in the same coordinate space as
    /// [`work_area`](Self::work_area).
    fn screen_work_areas(&self) -> Result<Vec<Rect>>;

    /// Every display's `(full_frame, work_area)` pair, in the same coordinate
    /// space as [`work_area`](Self::work_area). Drag-to-snap detects edge
    /// contact against the full frame (where the cursor actually stops) and
    /// lays the window out inside the work area.
    fn screens_cg(&self) -> Result<Vec<(Rect, Rect)>>;
}

/// Pick the work area a window should move to for `direction`, given every
/// display's work area. Areas are ordered left-to-right (then top-to-bottom)
/// and the walk wraps around. Returns the (from, to) pair, or `None` when no
/// areas are available.
pub fn adjacent_work_area(
    areas: &[Rect],
    window: Rect,
    direction: DisplayDirection,
) -> Option<(Rect, Rect)> {
    if areas.is_empty() {
        return None;
    }

    let center = |r: Rect| (r.x + r.width / 2.0, r.y + r.height / 2.0);
    let (wx, wy) = center(window);
    let contains = |r: &Rect| wx >= r.x && wx < r.x + r.width && wy >= r.y && wy < r.y + r.height;
    // The display the window is on: the one containing its center, or failing
    // that (the window was dragged mostly off screen) the nearest by center.
    let current = areas.iter().position(contains).unwrap_or_else(|| {
        let dist = |r: &Rect| {
            let (cx, cy) = center(*r);
            (cx - wx).powi(2) + (cy - wy).powi(2)
        };
        areas
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| dist(a).total_cmp(&dist(b)))
            .map(|(i, _)| i)
            .unwrap_or(0)
    });

    let mut order: Vec<usize> = (0..areas.len()).collect();
    order.sort_by(|&a, &b| {
        areas[a]
            .x
            .total_cmp(&areas[b].x)
            .then(areas[a].y.total_cmp(&areas[b].y))
    });
    let pos = order.iter().position(|&i| i == current).unwrap_or(0);
    let step = match direction {
        DisplayDirection::Next => 1,
        DisplayDirection::Prev => order.len() - 1,
    };
    let target = order[(pos + step) % order.len()];
    Some((areas[current], areas[target]))
}

/// An in-memory [`WindowManager`] used for tests and as a fallback on platforms
/// without a native implementation. Handles share the manager's window state,
/// so moves through a handle are visible to later reads.
#[derive(Debug)]
pub struct MockWindowManager {
    pub granted: bool,
    /// Work areas of the simulated displays; the first is the primary.
    pub areas: Vec<Rect>,
    /// The simulated focused window's frame, shared with handed-out handles.
    pub window: Arc<Mutex<Rect>>,
    /// The most recent frame applied through any handle.
    pub last_frame: Arc<Mutex<Option<Rect>>>,
}

impl MockWindowManager {
    pub fn new(area: Rect) -> Self {
        Self {
            granted: true,
            areas: vec![area],
            window: Arc::new(Mutex::new(Rect::new(0.0, 0.0, 100.0, 100.0))),
            last_frame: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set_window(&self, frame: Rect) {
        *self.window.lock().unwrap() = frame;
    }
}

/// The handle [`MockWindowManager`] hands out: writes back into the shared
/// window state and records the applied frame.
pub struct MockWindowHandle {
    window: Arc<Mutex<Rect>>,
    last_frame: Arc<Mutex<Option<Rect>>>,
}

impl WindowHandle for MockWindowHandle {
    fn frame(&self) -> Result<Rect> {
        Ok(*self.window.lock().unwrap())
    }

    fn set_frame(&self, frame: Rect) -> Result<()> {
        *self.window.lock().unwrap() = frame;
        *self.last_frame.lock().unwrap() = Some(frame);
        Ok(())
    }

    fn stable_hash(&self) -> u64 {
        // Handles to the same mock window share the same allocation.
        Arc::as_ptr(&self.window) as u64
    }
}

impl WindowManager for MockWindowManager {
    fn permission_granted(&self) -> bool {
        self.granted
    }

    fn focused_window(&self) -> Result<Box<dyn WindowHandle>> {
        Ok(Box::new(MockWindowHandle {
            window: self.window.clone(),
            last_frame: self.last_frame.clone(),
        }))
    }

    fn work_area(&self, window_frame: Rect) -> Result<Rect> {
        // Mirror the macOS behavior: the display containing the window's
        // center, falling back to the primary.
        let (cx, cy) = (
            window_frame.x + window_frame.width / 2.0,
            window_frame.y + window_frame.height / 2.0,
        );
        Ok(self
            .areas
            .iter()
            .find(|a| cx >= a.x && cx < a.x + a.width && cy >= a.y && cy < a.y + a.height)
            .copied()
            .unwrap_or(self.areas[0]))
    }

    fn screen_work_areas(&self) -> Result<Vec<Rect>> {
        Ok(self.areas.clone())
    }

    fn screens_cg(&self) -> Result<Vec<(Rect, Rect)>> {
        // The mock has no separate full frame; the work area doubles as both.
        Ok(self.areas.iter().map(|a| (*a, *a)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_handle_moves_are_visible_to_later_reads() {
        let m = MockWindowManager::new(Rect::new(0.0, 25.0, 1600.0, 975.0));
        let handle = m.focused_window().unwrap();
        let target = Rect::new(10.0, 30.0, 800.0, 600.0);
        handle.set_frame(target).unwrap();
        assert_eq!(handle.frame().unwrap(), target);
        assert_eq!(*m.last_frame.lock().unwrap(), Some(target));
        // A fresh handle sees the same window state.
        assert_eq!(m.focused_window().unwrap().frame().unwrap(), target);
    }

    #[test]
    fn adjacent_area_picks_nearest_when_window_is_off_screen() {
        let a = Rect::new(0.0, 25.0, 1600.0, 975.0);
        let b = Rect::new(1600.0, 0.0, 1200.0, 800.0);
        // Window center far to the right of both: nearest is `b`.
        let window = Rect::new(5000.0, 100.0, 200.0, 200.0);
        let (from, to) = adjacent_work_area(&[a, b], window, DisplayDirection::Next).unwrap();
        assert_eq!(from, b);
        assert_eq!(to, a, "Next from the rightmost wraps to the leftmost");
        assert!(adjacent_work_area(&[], window, DisplayDirection::Next).is_none());
    }

    #[test]
    fn adjacent_area_wraps_in_both_directions() {
        let a = Rect::new(0.0, 25.0, 1600.0, 975.0);
        let b = Rect::new(1600.0, 0.0, 1200.0, 800.0);
        let window = Rect::new(100.0, 100.0, 400.0, 300.0); // on `a`

        let (_, next) = adjacent_work_area(&[a, b], window, DisplayDirection::Next).unwrap();
        assert_eq!(next, b);
        let (_, prev) = adjacent_work_area(&[a, b], window, DisplayDirection::Prev).unwrap();
        assert_eq!(prev, b, "Prev from the leftmost wraps to the rightmost");

        // Single display: from == to (callers treat that as a no-op).
        let (from, to) = adjacent_work_area(&[a], window, DisplayDirection::Next).unwrap();
        assert_eq!(from, to);
    }
}
