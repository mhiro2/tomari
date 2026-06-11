//! Pure geometry: turn a [`WindowPreset`] into a concrete target frame within a
//! screen's work area. Coordinates are points with a top-left origin (y grows
//! downward), matching both `CGDisplay` bounds and the Accessibility API.

use tomari_core::domain::window::{Rect, WindowPreset};

/// How far (points) two frames may drift apart per edge and still count as the
/// same placement. Some windows clamp to minimum sizes or size increments, so
/// an exact comparison would never re-detect a snapped window.
const MATCH_TOLERANCE: f64 = 2.0;

/// Compute the target frame for `preset` inside the given `work_area`.
pub fn compute_frame(preset: WindowPreset, work_area: Rect) -> Rect {
    let Rect {
        x,
        y,
        width: w,
        height: h,
    } = work_area;
    let half_w = w / 2.0;
    let half_h = h / 2.0;
    let third_w = w / 3.0;

    match preset {
        WindowPreset::LeftHalf => Rect::new(x, y, half_w, h),
        WindowPreset::RightHalf => Rect::new(x + half_w, y, half_w, h),
        WindowPreset::TopHalf => Rect::new(x, y, w, half_h),
        WindowPreset::BottomHalf => Rect::new(x, y + half_h, w, half_h),
        WindowPreset::TopLeftQuarter => Rect::new(x, y, half_w, half_h),
        WindowPreset::TopRightQuarter => Rect::new(x + half_w, y, half_w, half_h),
        WindowPreset::BottomLeftQuarter => Rect::new(x, y + half_h, half_w, half_h),
        WindowPreset::BottomRightQuarter => Rect::new(x + half_w, y + half_h, half_w, half_h),
        WindowPreset::LeftThird => Rect::new(x, y, third_w, h),
        WindowPreset::CenterThird => Rect::new(x + third_w, y, third_w, h),
        WindowPreset::RightThird => Rect::new(x + 2.0 * third_w, y, third_w, h),
        WindowPreset::LeftTwoThirds => Rect::new(x, y, 2.0 * third_w, h),
        WindowPreset::RightTwoThirds => Rect::new(x + third_w, y, 2.0 * third_w, h),
        WindowPreset::Center => {
            let cw = w * 0.6;
            let ch = h * 0.7;
            Rect::new(x + (w - cw) / 2.0, y + (h - ch) / 2.0, cw, ch)
        }
        WindowPreset::Maximize => Rect::new(x, y, w, h),
    }
}

/// Map `frame` from one work area to another, keeping its position and size
/// proportional (a window filling the left half of one display fills the left
/// half of the other).
pub fn remap_frame(frame: Rect, from: Rect, to: Rect) -> Rect {
    if from.width <= 0.0 || from.height <= 0.0 {
        return to;
    }
    let fx = (frame.x - from.x) / from.width;
    let fy = (frame.y - from.y) / from.height;
    let fw = frame.width / from.width;
    let fh = frame.height / from.height;
    Rect::new(
        to.x + fx * to.width,
        to.y + fy * to.height,
        (fw * to.width).min(to.width),
        (fh * to.height).min(to.height),
    )
}

/// The presets a half-snap cycles through on repeated activation:
/// 1/2 → 1/3 → 2/3 → back to 1/2.
fn cycle_group(preset: WindowPreset) -> Option<[WindowPreset; 3]> {
    match preset {
        WindowPreset::LeftHalf => Some([
            WindowPreset::LeftHalf,
            WindowPreset::LeftThird,
            WindowPreset::LeftTwoThirds,
        ]),
        WindowPreset::RightHalf => Some([
            WindowPreset::RightHalf,
            WindowPreset::RightThird,
            WindowPreset::RightTwoThirds,
        ]),
        _ => None,
    }
}

/// Whether two frames describe the same placement, within [`MATCH_TOLERANCE`].
pub fn frames_match(a: Rect, b: Rect) -> bool {
    (a.x - b.x).abs() <= MATCH_TOLERANCE
        && (a.y - b.y).abs() <= MATCH_TOLERANCE
        && (a.width - b.width).abs() <= MATCH_TOLERANCE
        && (a.height - b.height).abs() <= MATCH_TOLERANCE
}

/// The preset a repeated half-snap advances to: when `applied` (what the
/// previous press of the same request placed) is a member of `requested`'s
/// cycle, step to the next member; otherwise start over at the request.
pub fn next_in_cycle(requested: WindowPreset, applied: WindowPreset) -> WindowPreset {
    let Some(group) = cycle_group(requested) else {
        return requested;
    };
    match group.iter().position(|p| *p == applied) {
        Some(i) => group[(i + 1) % group.len()],
        None => requested,
    }
}

/// How close (points) the cursor must come to a screen edge to start a
/// drag-to-snap preview. The OS stops the cursor a pixel shy of the boundary,
/// so this only needs to clear that gap without reaching into the screen.
const EDGE_THRESHOLD: f64 = 6.0;

/// How far (points) along an edge from a corner still counts as that corner
/// rather than the edge's middle. The end stretches of each edge map to the
/// adjacent quarter; the middle maps to a half (or maximize, for the top).
const CORNER_SIZE: f64 = 120.0;

/// The preset a drag-to-snap should apply for a cursor that has reached the
/// border of `screen_full_frame` (CG coordinates, top-left origin). The cursor
/// stops at the *full* display bounds, so detection uses those — the resulting
/// preset is later laid out within the work area. `None` means the cursor is
/// not on a snap-triggering border (including the bottom edge's middle, which
/// is intentionally left unassigned).
///
/// Mapping (Rectangle's default): top edge → maximize, left/right edge → half,
/// the ends of every edge (within [`CORNER_SIZE`]) → the adjacent quarter.
pub fn edge_snap_preset(cursor: (f64, f64), screen_full_frame: Rect) -> Option<WindowPreset> {
    let (cx, cy) = cursor;
    let Rect {
        x,
        y,
        width: w,
        height: h,
    } = screen_full_frame;
    if cx < x || cx >= x + w || cy < y || cy >= y + h {
        return None;
    }

    let from_left = cx - x;
    let from_right = (x + w) - cx;
    let from_top = cy - y;
    let from_bottom = (y + h) - cy;

    let on_left = from_left <= EDGE_THRESHOLD;
    let on_right = from_right <= EDGE_THRESHOLD;
    let on_top = from_top <= EDGE_THRESHOLD;
    let on_bottom = from_bottom <= EDGE_THRESHOLD;

    // A corner is reached either by sliding along a horizontal edge toward a
    // side, or up/down a vertical edge toward the top/bottom — so detection is
    // symmetric regardless of which edge the cursor is hugging.
    let near_left = from_left <= CORNER_SIZE;
    let near_right = from_right <= CORNER_SIZE;
    let near_top = from_top <= CORNER_SIZE;
    let near_bottom = from_bottom <= CORNER_SIZE;

    if (on_top && near_left) || (on_left && near_top) {
        Some(WindowPreset::TopLeftQuarter)
    } else if (on_top && near_right) || (on_right && near_top) {
        Some(WindowPreset::TopRightQuarter)
    } else if (on_bottom && near_left) || (on_left && near_bottom) {
        Some(WindowPreset::BottomLeftQuarter)
    } else if (on_bottom && near_right) || (on_right && near_bottom) {
        Some(WindowPreset::BottomRightQuarter)
    } else if on_top {
        Some(WindowPreset::Maximize)
    } else if on_left {
        Some(WindowPreset::LeftHalf)
    } else if on_right {
        Some(WindowPreset::RightHalf)
    } else {
        // The bottom edge's middle has no half assigned: a window dragged there
        // simply drops where it is.
        None
    }
}

/// The display whose full frame contains the cursor, returned as its
/// `(full_frame, work_area)` pair (both CG coordinates). `None` when the cursor
/// is off every display (e.g. mid-transition between mirrored arrangements).
pub fn screen_at_cursor(screens: &[(Rect, Rect)], cx: f64, cy: f64) -> Option<(Rect, Rect)> {
    screens.iter().copied().find(|(full, _)| {
        cx >= full.x && cx < full.x + full.width && cy >= full.y && cy < full.y + full.height
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect {
        x: 0.0,
        y: 25.0,
        width: 1600.0,
        height: 975.0,
    };

    #[test]
    fn left_half() {
        assert_eq!(
            compute_frame(WindowPreset::LeftHalf, AREA),
            Rect::new(0.0, 25.0, 800.0, 975.0)
        );
    }

    #[test]
    fn right_half_starts_at_midpoint() {
        assert_eq!(
            compute_frame(WindowPreset::RightHalf, AREA),
            Rect::new(800.0, 25.0, 800.0, 975.0)
        );
    }

    #[test]
    fn maximize_fills_work_area() {
        assert_eq!(compute_frame(WindowPreset::Maximize, AREA), AREA);
    }

    #[test]
    fn quarters_tile_the_screen() {
        let tl = compute_frame(WindowPreset::TopLeftQuarter, AREA);
        let br = compute_frame(WindowPreset::BottomRightQuarter, AREA);
        // The bottom-right quarter begins exactly where the top-left ends.
        assert_eq!(br.x, tl.x + tl.width);
        assert_eq!(br.y, tl.y + tl.height);
        assert_eq!(tl.width, 800.0);
        assert_eq!(tl.height, 487.5);
    }

    #[test]
    fn thirds_partition_width() {
        let l = compute_frame(WindowPreset::LeftThird, AREA);
        let c = compute_frame(WindowPreset::CenterThird, AREA);
        let r = compute_frame(WindowPreset::RightThird, AREA);
        assert!((l.width - c.width).abs() < 1e-9);
        assert!((c.width - r.width).abs() < 1e-9);
        assert!((l.width * 3.0 - AREA.width).abs() < 1e-9);
        assert_eq!(c.x, l.x + l.width);
        assert_eq!(r.x, c.x + c.width);
    }

    #[test]
    fn center_is_centered_and_smaller() {
        let c = compute_frame(WindowPreset::Center, AREA);
        assert!(c.width < AREA.width && c.height < AREA.height);
        // Equal margins on left/right.
        let left_margin = c.x - AREA.x;
        let right_margin = (AREA.x + AREA.width) - (c.x + c.width);
        assert!((left_margin - right_margin).abs() < 1e-9);
    }

    #[test]
    fn repeated_half_snap_cycles_through_thirds() {
        // 1/2 → 1/3 → 2/3 → back to 1/2 on repeated presses.
        let req = WindowPreset::LeftHalf;
        assert_eq!(
            next_in_cycle(req, WindowPreset::LeftHalf),
            WindowPreset::LeftThird
        );
        assert_eq!(
            next_in_cycle(req, WindowPreset::LeftThird),
            WindowPreset::LeftTwoThirds
        );
        assert_eq!(
            next_in_cycle(req, WindowPreset::LeftTwoThirds),
            WindowPreset::LeftHalf
        );
    }

    #[test]
    fn cycle_restarts_when_the_applied_preset_is_unrelated() {
        // The previous press applied something outside this request's cycle
        // (e.g. the request changed sides): start over at the request.
        assert_eq!(
            next_in_cycle(WindowPreset::RightHalf, WindowPreset::LeftThird),
            WindowPreset::RightHalf
        );
    }

    #[test]
    fn non_cycling_presets_pass_through() {
        assert_eq!(
            next_in_cycle(WindowPreset::Maximize, WindowPreset::Maximize),
            WindowPreset::Maximize
        );
    }

    #[test]
    fn frames_match_uses_a_small_tolerance() {
        let a = Rect::new(0.0, 25.0, 800.0, 975.0);
        let drifted = Rect::new(1.0, 25.0, 798.5, 975.0);
        let off = Rect::new(10.0, 25.0, 800.0, 975.0);
        assert!(frames_match(a, drifted));
        assert!(!frames_match(a, off));
    }

    /// A 1920×1080 display at the origin, in CG coordinates.
    const FULL: Rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 1920.0,
        height: 1080.0,
    };

    #[test]
    fn edge_middles_map_to_halves_and_maximize() {
        // Mid-left edge → left half; mid-right → right half; mid-top → maximize.
        assert_eq!(
            edge_snap_preset((1.0, 540.0), FULL),
            Some(WindowPreset::LeftHalf)
        );
        assert_eq!(
            edge_snap_preset((1919.0, 540.0), FULL),
            Some(WindowPreset::RightHalf)
        );
        assert_eq!(
            edge_snap_preset((960.0, 1.0), FULL),
            Some(WindowPreset::Maximize)
        );
    }

    #[test]
    fn bottom_edge_middle_has_no_snap() {
        assert_eq!(edge_snap_preset((960.0, 1079.0), FULL), None);
    }

    #[test]
    fn corners_map_to_quarters_from_either_edge() {
        // Touching the top edge near the left → top-left quarter.
        assert_eq!(
            edge_snap_preset((10.0, 1.0), FULL),
            Some(WindowPreset::TopLeftQuarter)
        );
        // Touching the left edge near the top → also top-left quarter.
        assert_eq!(
            edge_snap_preset((1.0, 10.0), FULL),
            Some(WindowPreset::TopLeftQuarter)
        );
        assert_eq!(
            edge_snap_preset((1919.0, 10.0), FULL),
            Some(WindowPreset::TopRightQuarter)
        );
        // Bottom corners are reachable even though the bottom middle is not.
        assert_eq!(
            edge_snap_preset((1.0, 1070.0), FULL),
            Some(WindowPreset::BottomLeftQuarter)
        );
        assert_eq!(
            edge_snap_preset((1919.0, 1070.0), FULL),
            Some(WindowPreset::BottomRightQuarter)
        );
    }

    #[test]
    fn corner_zone_gives_way_to_the_edge_middle() {
        // Just outside the corner stretch on the left edge → left half.
        assert_eq!(
            edge_snap_preset((1.0, CORNER_SIZE + 1.0), FULL),
            Some(WindowPreset::LeftHalf)
        );
        // Just inside it → quarter.
        assert_eq!(
            edge_snap_preset((1.0, CORNER_SIZE - 1.0), FULL),
            Some(WindowPreset::TopLeftQuarter)
        );
    }

    #[test]
    fn interior_and_outside_yield_no_snap() {
        assert_eq!(edge_snap_preset((960.0, 540.0), FULL), None);
        assert_eq!(edge_snap_preset((-5.0, 540.0), FULL), None);
        assert_eq!(edge_snap_preset((960.0, 2000.0), FULL), None);
    }

    #[test]
    fn screen_at_cursor_selects_the_containing_display() {
        let main = (
            Rect::new(0.0, 0.0, 1920.0, 1080.0),
            Rect::new(0.0, 25.0, 1920.0, 1055.0),
        );
        // A secondary display to the left (negative x), as macOS reports it.
        let left = (
            Rect::new(-1440.0, 0.0, 1440.0, 900.0),
            Rect::new(-1440.0, 25.0, 1440.0, 875.0),
        );
        let screens = [main, left];
        assert_eq!(screen_at_cursor(&screens, 100.0, 100.0), Some(main));
        assert_eq!(screen_at_cursor(&screens, -1200.0, 100.0), Some(left));
        // Off every display.
        assert_eq!(screen_at_cursor(&screens, -5000.0, 100.0), None);
    }

    #[test]
    fn edge_snap_uses_each_displays_own_bounds() {
        // The left edge of a display whose origin is negative is at its own x,
        // not the global origin.
        let left = Rect::new(-1440.0, 0.0, 1440.0, 900.0);
        assert_eq!(
            edge_snap_preset((-1439.0, 450.0), left),
            Some(WindowPreset::LeftHalf)
        );
    }

    #[test]
    fn every_preset_stays_within_work_area() {
        for preset in WindowPreset::ALL {
            let f = compute_frame(preset, AREA);
            assert!(f.x >= AREA.x - 1e-9, "{preset:?} x");
            assert!(f.y >= AREA.y - 1e-9, "{preset:?} y");
            assert!(
                f.x + f.width <= AREA.x + AREA.width + 1e-9,
                "{preset:?} right"
            );
            assert!(
                f.y + f.height <= AREA.y + AREA.height + 1e-9,
                "{preset:?} bottom"
            );
        }
    }
}
