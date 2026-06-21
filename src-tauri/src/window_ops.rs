//! Window actions shared by every input path (hotkey, tray, UI).
//!
//! Each operation honors the window-management master switch, and records the
//! moved window (as a handle) with its previous frame in the undo history —
//! but only when something actually moved, so Undo never burns an entry on a
//! no-op. Snaps additionally track what the last press did, so repeating the
//! same half-snap cycles 1/2 → 1/3 → 2/3.

use tomari_core::{DisplayDirection, Rect, WindowPreset};
use tomari_window::{WindowHandle, adjacent_work_area, geometry};

use crate::error::CmdError;
use crate::locks::MutexExt;
use crate::state::{AppState, LastSnap};

/// Whether a snap advances the 1/2 → 1/3 → 2/3 cycle on repeat, or always
/// lands exactly on the requested preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapBehavior {
    /// Hotkeys, modifier taps and the UI: repeating the same snap on an unmoved
    /// window cycles through the group.
    Cycle,
    /// The URL scheme: every invocation is idempotent, landing on exactly the
    /// requested preset regardless of history.
    Exact,
}

/// Whether window management is enabled; window ops silently no-op when off.
fn enabled(state: &AppState) -> bool {
    state.settings.lock_safe().window_management_enabled
}

/// Resolve the focused window, or fail like the platform implementations do
/// when the permission is missing.
fn focused(state: &AppState) -> Result<(Box<dyn WindowHandle>, Rect), CmdError> {
    if !state.windows.permission_granted() {
        return Err(tomari_window::Error::PermissionDenied.into());
    }
    let window = state.windows.focused_window()?;
    let frame = window.frame()?;
    Ok((window, frame))
}

/// Apply `frame` to `window` and, when that actually moved it, record the
/// previous frame in the undo history.
fn apply(
    state: &AppState,
    window: Box<dyn WindowHandle>,
    previous: Rect,
    frame: Rect,
) -> Result<Rect, CmdError> {
    window.set_frame(frame)?;
    // Read back what the window settled on (it may clamp to a minimum size).
    // A failed read means the window vanished mid-flight — surface that rather
    // than recording state we cannot know.
    let after = window.frame()?;
    if !geometry::frames_match(previous, after) {
        state.push_window_history(window, previous);
    }
    Ok(after)
}

/// Snap a window the user dragged to a screen edge to `frame`, recording the
/// move for undo when it took effect. Unlike [`snap`], the caller already holds
/// the window handle and target frame (drag-to-snap resolves both from the
/// cursor), so this skips the focused-window and work-area lookups. Best-effort:
/// it runs inside the listen-only gesture tap, where there is no caller to
/// surface an error to, so failures are simply dropped.
pub fn apply_dragged<H>(state: &AppState, window: &H, frame: Rect)
where
    H: WindowHandle + Clone + 'static,
{
    let Ok(previous) = window.frame() else {
        return;
    };
    if window.set_frame(frame).is_err() {
        return;
    }
    // Record the pre-snap frame only when the window actually moved, so Undo
    // does not burn an entry on a snap that changed nothing.
    if let Ok(after) = window.frame()
        && !geometry::frames_match(previous, after)
    {
        state.push_window_history(Box::new(window.clone()), previous);
    }
}

/// Snap the focused window to `preset`. With [`SnapBehavior::Cycle`], repeating
/// the same request on a window that has not moved since cycles 1/2 → 1/3 →
/// 2/3; with [`SnapBehavior::Exact`] it always applies exactly `preset`. Returns
/// the preset actually applied (`None` when window management is disabled).
pub fn snap(
    state: &AppState,
    preset: WindowPreset,
    behavior: SnapBehavior,
) -> Result<Option<WindowPreset>, CmdError> {
    if !enabled(state) {
        return Ok(None);
    }
    let (window, previous) = focused(state)?;
    let area = state.windows.work_area(previous)?;

    // "Repeated press" means: same requested preset, on the same window, and
    // the window still sits where the previous snap left it (so refocusing a
    // different window or dragging it away restarts the cycle). Exact placement
    // never cycles, so it skips the lookup entirely.
    let window_hash = window.stable_hash();
    let applied = match behavior {
        SnapBehavior::Exact => preset,
        SnapBehavior::Cycle => match state.last_snap() {
            Some(last)
                if last.requested == preset
                    && last.window_hash == window_hash
                    && geometry::frames_match(previous, last.after) =>
            {
                geometry::next_in_cycle(preset, last.applied)
            }
            _ => preset,
        },
    };

    let frame = geometry::compute_frame(applied, area);
    let after = apply(state, window, previous, frame)?;
    match behavior {
        SnapBehavior::Cycle => state.set_last_snap(LastSnap {
            requested: preset,
            applied,
            window_hash,
            after,
        }),
        // Exact placement sits outside the cycle. It must not merely *skip*
        // updating the cycle state — it must clear it, or a prior Cycle snap of
        // the same preset would still be on record and the next Cycle snap
        // would treat the exact placement as a repeat and advance unexpectedly.
        SnapBehavior::Exact => state.clear_last_snap(),
    }
    Ok(Some(applied))
}

/// Move the focused window to the neighboring display, keeping its position
/// and size proportional. A no-op on a single display.
pub fn move_to_display(state: &AppState, direction: DisplayDirection) -> Result<(), CmdError> {
    if !enabled(state) {
        return Ok(());
    }
    let (window, previous) = focused(state)?;
    let areas = state.windows.screen_work_areas()?;
    let Some((from, to)) = adjacent_work_area(&areas, previous, direction) else {
        return Ok(());
    };
    if from == to {
        return Ok(());
    }
    let frame = geometry::remap_frame(previous, from, to);
    apply(state, window, previous, frame)?;
    Ok(())
}

/// Restore the most recently moved window to its recorded frame. Entries whose
/// window has since closed are discarded, falling through to the next one; a
/// transient failure keeps its entry so the user can simply retry.
pub fn undo(state: &AppState) -> Result<(), CmdError> {
    if !enabled(state) {
        return Ok(());
    }
    while let Some((window, frame)) = state.pop_window_history() {
        match window.set_frame(frame) {
            Ok(()) => return Ok(()),
            Err(e) if e.window_gone() => continue,
            Err(e) => {
                state.push_window_history(window, frame);
                return Err(e.into());
            }
        }
    }
    // An empty (or fully stale) history is a quiet no-op.
    Ok(())
}

#[cfg(test)]
mod tests {
    use tomari_core::{AppSettings, Database};
    use tomari_keyboard::ModifierEngine;
    use tomari_window::{MockWindowManager, compute_frame};

    use super::*;

    fn area() -> Rect {
        Rect::new(0.0, 25.0, 1600.0, 975.0)
    }

    fn state_with(windows: MockWindowManager, settings: AppSettings) -> AppState {
        AppState::new(
            Database::open_in_memory().unwrap(),
            ModifierEngine::new(vec![]),
            Box::new(windows),
            settings,
        )
    }

    fn default_state() -> AppState {
        state_with(MockWindowManager::new(area()), AppSettings::default())
    }

    fn focused_frame(state: &AppState) -> Rect {
        state.windows.focused_window().unwrap().frame().unwrap()
    }

    #[test]
    fn repeated_snap_cycles_through_the_group() {
        let state = default_state();

        assert_eq!(
            snap(&state, WindowPreset::LeftHalf, SnapBehavior::Cycle).unwrap(),
            Some(WindowPreset::LeftHalf)
        );
        assert_eq!(
            snap(&state, WindowPreset::LeftHalf, SnapBehavior::Cycle).unwrap(),
            Some(WindowPreset::LeftThird)
        );
        assert_eq!(
            snap(&state, WindowPreset::LeftHalf, SnapBehavior::Cycle).unwrap(),
            Some(WindowPreset::LeftTwoThirds)
        );
        assert_eq!(
            snap(&state, WindowPreset::LeftHalf, SnapBehavior::Cycle).unwrap(),
            Some(WindowPreset::LeftHalf)
        );
        assert_eq!(
            focused_frame(&state),
            compute_frame(WindowPreset::LeftHalf, area())
        );
    }

    #[test]
    fn moving_the_window_between_presses_restarts_the_cycle() {
        let state = default_state();
        snap(&state, WindowPreset::LeftHalf, SnapBehavior::Cycle).unwrap();

        // The user drags the window somewhere else before pressing again.
        state
            .windows
            .focused_window()
            .unwrap()
            .set_frame(Rect::new(300.0, 300.0, 500.0, 400.0))
            .unwrap();

        assert_eq!(
            snap(&state, WindowPreset::LeftHalf, SnapBehavior::Cycle).unwrap(),
            Some(WindowPreset::LeftHalf),
            "a moved window starts over instead of cycling"
        );
    }

    #[test]
    fn changing_the_request_restarts_at_that_preset() {
        let state = default_state();
        snap(&state, WindowPreset::LeftHalf, SnapBehavior::Cycle).unwrap();
        assert_eq!(
            snap(&state, WindowPreset::RightHalf, SnapBehavior::Cycle).unwrap(),
            Some(WindowPreset::RightHalf)
        );
    }

    #[test]
    fn exact_snap_never_cycles() {
        let state = default_state();
        // Repeating an exact snap is idempotent — it never advances 1/2 → 1/3.
        for _ in 0..3 {
            assert_eq!(
                snap(&state, WindowPreset::LeftHalf, SnapBehavior::Exact).unwrap(),
                Some(WindowPreset::LeftHalf)
            );
        }
    }

    #[test]
    fn exact_snap_clears_the_cycle() {
        let state = default_state();
        // A cycle is in progress on the left half...
        snap(&state, WindowPreset::LeftHalf, SnapBehavior::Cycle).unwrap();
        // ...an exact snap of the same preset lands and must clear that cycle...
        snap(&state, WindowPreset::LeftHalf, SnapBehavior::Exact).unwrap();
        // ...so the next cycle snap starts fresh at the half rather than
        // advancing to the third as if the exact placement had been a repeat.
        assert_eq!(
            snap(&state, WindowPreset::LeftHalf, SnapBehavior::Cycle).unwrap(),
            Some(WindowPreset::LeftHalf)
        );
    }

    #[test]
    fn undo_restores_the_window_that_was_moved() {
        let state = default_state();
        let original = Rect::new(10.0, 40.0, 800.0, 600.0);
        state
            .windows
            .focused_window()
            .unwrap()
            .set_frame(original)
            .unwrap();

        snap(&state, WindowPreset::Maximize, SnapBehavior::Cycle).unwrap();
        assert_ne!(focused_frame(&state), original);

        undo(&state).unwrap();
        assert_eq!(focused_frame(&state), original);
        assert!(state.pop_window_history().is_none());
    }

    #[test]
    fn noop_moves_do_not_pollute_the_history() {
        let state = default_state();
        snap(&state, WindowPreset::Maximize, SnapBehavior::Cycle).unwrap();
        // Already maximized: snapping again must not add a second entry.
        snap(&state, WindowPreset::Maximize, SnapBehavior::Cycle).unwrap();
        // Single display: moving to the next display is a no-op.
        move_to_display(&state, DisplayDirection::Next).unwrap();

        assert!(state.pop_window_history().is_some());
        assert!(state.pop_window_history().is_none());
    }

    #[test]
    fn moves_to_the_adjacent_display_proportionally() {
        let mut mock = MockWindowManager::new(area());
        let right = Rect::new(1600.0, 0.0, 1200.0, 800.0);
        mock.areas.push(right);
        mock.set_window(compute_frame(WindowPreset::LeftHalf, area()));
        let state = state_with(mock, AppSettings::default());

        move_to_display(&state, DisplayDirection::Next).unwrap();
        assert_eq!(
            focused_frame(&state),
            compute_frame(WindowPreset::LeftHalf, right)
        );
        // The move is undoable.
        undo(&state).unwrap();
        assert_eq!(
            focused_frame(&state),
            compute_frame(WindowPreset::LeftHalf, area())
        );
    }

    #[test]
    fn disabled_window_management_skips_everything() {
        let settings = AppSettings {
            window_management_enabled: false,
            ..AppSettings::default()
        };
        let state = state_with(MockWindowManager::new(area()), settings);

        assert_eq!(
            snap(&state, WindowPreset::LeftHalf, SnapBehavior::Cycle).unwrap(),
            None
        );
        move_to_display(&state, DisplayDirection::Next).unwrap();
        undo(&state).unwrap();
        assert!(state.pop_window_history().is_none());
    }

    #[test]
    fn undo_with_empty_history_is_a_no_op() {
        let state = default_state();
        undo(&state).unwrap();
    }

    /// A handle whose window always fails with a configurable error, to drive
    /// the undo fall-through logic.
    struct FailingHandle(tomari_window::Error);

    impl FailingHandle {
        fn gone() -> Self {
            Self(tomari_window::Error::NoFocusedWindow)
        }

        fn transient() -> Self {
            // An AX error code that does not mean "window gone".
            Self(tomari_window::Error::Ax(-25201))
        }

        fn err(&self) -> tomari_window::Error {
            match self.0 {
                tomari_window::Error::NoFocusedWindow => tomari_window::Error::NoFocusedWindow,
                tomari_window::Error::Ax(code) => tomari_window::Error::Ax(code),
                _ => unreachable!(),
            }
        }
    }

    impl WindowHandle for FailingHandle {
        fn frame(&self) -> tomari_window::Result<Rect> {
            Err(self.err())
        }

        fn set_frame(&self, _frame: Rect) -> tomari_window::Result<()> {
            Err(self.err())
        }

        fn stable_hash(&self) -> u64 {
            0
        }
    }

    #[test]
    fn undo_skips_entries_whose_window_is_gone() {
        let state = default_state();
        let original = focused_frame(&state);
        snap(&state, WindowPreset::Maximize, SnapBehavior::Cycle).unwrap();
        // A later entry whose window has since closed.
        state.push_window_history(
            Box::new(FailingHandle::gone()),
            Rect::new(0.0, 0.0, 1.0, 1.0),
        );

        undo(&state).unwrap();
        assert_eq!(
            focused_frame(&state),
            original,
            "fell through to the live entry"
        );
    }

    #[test]
    fn undo_keeps_the_entry_on_a_transient_failure() {
        let state = default_state();
        let frame = Rect::new(5.0, 30.0, 300.0, 200.0);
        state.push_window_history(Box::new(FailingHandle::transient()), frame);

        assert!(undo(&state).is_err());
        let kept = state.pop_window_history();
        assert_eq!(kept.map(|(_, f)| f), Some(frame), "entry stays for a retry");
    }

    #[test]
    fn cycle_requires_the_same_window() {
        let state = default_state();
        snap(&state, WindowPreset::LeftHalf, SnapBehavior::Cycle).unwrap();

        // A different window happens to sit on the exact same frame.
        let other = MockWindowManager::new(area());
        other.set_window(focused_frame(&state));
        let state2 = state_with(other, AppSettings::default());
        // Carry the last-snap over to the fresh state to isolate the
        // window-identity check (hashes differ between the two mocks).
        if let Some(last) = state.last_snap() {
            state2.set_last_snap(last);
        }

        assert_eq!(
            snap(&state2, WindowPreset::LeftHalf, SnapBehavior::Cycle).unwrap(),
            Some(WindowPreset::LeftHalf),
            "a different window must not continue the cycle"
        );
    }
}
