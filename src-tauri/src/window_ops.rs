//! Window actions shared by every input path (hotkey, tray, UI).
//!
//! Each operation honors the window-management master switch, and records the
//! moved window (as a handle) with its previous frame in the undo history —
//! but only when something actually moved, so Undo never burns an entry on a
//! no-op. Snaps additionally track what the last press did, so repeating the
//! same half-snap cycles 1/2 → 1/3 → 2/3.

use serde::{Deserialize, Serialize};
use tomari_core::{
    DisplayDirection, NormalizedRect, PlacementSlot, Rect, WindowApplication, WindowPlacement,
    WindowPreset,
};
use tomari_window::{FocusedWindow, WindowHandle, adjacent_work_area, geometry};

use crate::error::CmdError;
use crate::locks::MutexExt;
use crate::state::{AppState, LastPlacement, LastSnap, PlacementEdit, WindowChange};

/// Opaque identity of the window represented by a [`PlacementContext`]. UI
/// commands send it back so a focus change cannot make an action silently land
/// on a different application than the one still shown in the panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowTarget {
    pub bundle_id: String,
    pub window_id: String,
}

/// Everything the Window settings view needs about the focused application.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlacementContext {
    pub target: WindowTarget,
    pub application: WindowApplication,
    pub current_frame: NormalizedRect,
    pub placements: Vec<WindowPlacement>,
    pub can_move_to_display: bool,
}

/// Whether undo/redo is currently available, independent of focused-window
/// resolution so recovery remains reachable even with the desktop focused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowHistoryStatus {
    pub can_undo: bool,
    pub can_redo: bool,
}

/// What an undo/redo request actually accomplished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HistoryActionResult {
    Applied,
    Empty,
    StaleEntriesDiscarded,
}

/// Result of moving to a neighboring display and restoring a remembered home.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum MoveRecallResult {
    Moved { slot: PlacementSlot },
    NoAdjacentDisplay,
}

/// Whether a remembered-home capture/delete changed persistent data and can
/// therefore be recovered with [`undo_placement_edit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlacementEditResult {
    pub changed: bool,
    /// Whether the edit's previous logical state can be restored safely.
    /// Cleaning up an undecodable database row changes persistent data but is
    /// deliberately not undoable because reintroducing corruption is unsafe.
    pub undoable: bool,
}

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

fn window_target(focused: &FocusedWindow) -> WindowTarget {
    WindowTarget {
        bundle_id: focused.application.bundle_id.clone(),
        // Keep the platform-sized hash opaque across the JSON boundary; a JS
        // number cannot represent every u64 without losing identity bits.
        window_id: format!("{:016x}", focused.handle.stable_hash()),
    }
}

fn resolve_focused_context(state: &AppState) -> tomari_window::Result<(FocusedWindow, Rect, Rect)> {
    if !state.windows.permission_granted() {
        return Err(tomari_window::Error::PermissionDenied);
    }
    let focused = state.windows.focused_window_context()?;
    let frame = focused.handle.frame()?;
    let area = state.windows.work_area(frame)?;
    Ok((focused, frame, area))
}

fn retry_ax_read_once<T>(
    mut read: impl FnMut() -> tomari_window::Result<T>,
) -> tomari_window::Result<T> {
    match read() {
        Err(error) if error.retryable() => read(),
        result => result,
    }
}

fn focused_context(
    state: &AppState,
    expected: Option<&WindowTarget>,
) -> Result<(FocusedWindow, Rect, Rect), CmdError> {
    // kAXErrorCannotComplete is a transient messaging failure (often the
    // bounded timeout). Apple explicitly recommends repeating such a read.
    // Resolution is read-only, so one immediate retry is safe; mutations still
    // happen only after the exact window identity has been checked.
    let (focused, frame, area) = retry_ax_read_once(|| resolve_focused_context(state))?;
    if expected.is_some_and(|expected| *expected != window_target(&focused)) {
        return Err(CmdError::window_target_changed(
            "the focused window changed since the panel was last refreshed",
        ));
    }
    Ok((focused, frame, area))
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
        state.push_window_change(WindowChange {
            window,
            before: previous,
            after,
        });
    }
    Ok(after)
}

/// Snap a window the user dragged to a screen edge to the frame `decide`
/// chooses, recording its mouse-down frame as the undo target. Reading "before"
/// at release would only capture the temporary edge position the OS dragged it
/// through. Best-effort: it runs inside the listen-only gesture tap, where there
/// is no caller to surface an error to, so failures return `false`.
///
/// `decide` runs *under* the window-mutation lock, immediately before the
/// write: the drop's target depends on the display geometry current at that
/// instant, and a decision taken before waiting for the lock could be overtaken
/// by a display change while waiting. `None` declines the snap.
pub fn apply_dragged<H>(
    state: &AppState,
    window: &H,
    before: Rect,
    decide: impl FnOnce() -> Option<Rect>,
) -> bool
where
    H: WindowHandle + Clone + 'static,
{
    let _op = state.lock_window_mutation();
    let Some(frame) = decide() else {
        return false;
    };
    if window.set_frame(frame).is_err() {
        return false;
    }
    if let Ok(after) = window.frame()
        && !geometry::frames_match(before, after)
    {
        state.push_window_change(WindowChange {
            window: Box::new(window.clone()),
            before,
            after,
        });
        return true;
    }
    false
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
    let _op = state.lock_window_mutation();
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
    let _op = state.lock_window_mutation();
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
    state.clear_last_placement();
    Ok(())
}

/// Describe the frontmost non-Tomari application's current and remembered
/// positions. The current frame is normalized with the same geometry used by
/// capture so the UI preview is exactly what would be saved.
pub fn placement_context(state: &AppState) -> Result<PlacementContext, CmdError> {
    let (focused, frame, area) = focused_context(state, None)?;
    let current_frame = geometry::normalize_frame(frame, area)
        .ok_or_else(|| CmdError::other("the focused window has an invalid frame"))?;
    let placements = state
        .db
        .list_window_placements(&focused.application.bundle_id)?;
    let can_move_to_display = state.windows.screen_work_areas()?.len() > 1;
    Ok(PlacementContext {
        target: window_target(&focused),
        application: focused.application,
        current_frame,
        placements,
        can_move_to_display,
    })
}

pub fn window_history_status(state: &AppState) -> WindowHistoryStatus {
    let (can_undo, can_redo) = state.window_history_status();
    WindowHistoryStatus { can_undo, can_redo }
}

/// Remember the focused window's safe, display-relative position in `slot`.
pub fn capture_placement(
    state: &AppState,
    target: &WindowTarget,
    slot: PlacementSlot,
) -> Result<PlacementEditResult, CmdError> {
    let _op = state.lock_window_mutation();
    let (focused, frame, area) = focused_context(state, Some(target))?;
    let frame = geometry::normalize_frame(frame, area)
        .ok_or_else(|| CmdError::other("the focused window has an invalid frame"))?;
    let placement = WindowPlacement {
        application: focused.application,
        slot,
        frame,
    };
    // Placement edits are serialized by `window_mutation` (held above), not
    // by `config_mutation`: that lock is held across a main-thread hop by the
    // shortcut commands (the global-shortcut plugin registers on the main
    // thread and waits), and this runs *on* the main thread — waiting for it
    // here would deadlock the app. Nothing under `config_mutation` touches the
    // placements table, so no ordering is lost.
    let before = state
        .db
        .get_window_placement(&placement.application.bundle_id, slot)?;
    if before.as_ref() == Some(&placement) {
        return Ok(PlacementEditResult {
            changed: false,
            undoable: false,
        });
    }
    state.db.save_window_placement(&placement)?;
    state.push_placement_edit(PlacementEdit {
        before,
        after: Some(placement),
    });
    state.clear_last_placement();
    Ok(PlacementEditResult {
        changed: true,
        undoable: true,
    })
}

/// Forget one remembered position for the focused application.
pub fn forget_placement(
    state: &AppState,
    target: &WindowTarget,
    slot: PlacementSlot,
) -> Result<PlacementEditResult, CmdError> {
    let _op = state.lock_window_mutation();
    let (focused, _, _) = focused_context(state, Some(target))?;
    // Placement edits are serialized by `window_mutation` (held above), not
    // by `config_mutation`: that lock is held across a main-thread hop by the
    // shortcut commands (the global-shortcut plugin registers on the main
    // thread and waits), and this runs *on* the main thread — waiting for it
    // here would deadlock the app. Nothing under `config_mutation` touches the
    // placements table, so no ordering is lost.
    let before = state
        .db
        .get_window_placement(&focused.application.bundle_id, slot)?;
    let deleted = state
        .db
        .delete_window_placement(&focused.application.bundle_id, slot)?;
    if !deleted {
        return Ok(PlacementEditResult {
            changed: false,
            undoable: false,
        });
    }
    let undoable = if let Some(before) = before {
        state.push_placement_edit(PlacementEdit {
            before: Some(before),
            after: None,
        });
        true
    } else {
        // The row existed but could not be decoded. Removing it repairs the
        // slot, but an invalid value must never be restored into undo history.
        false
    };
    state.clear_last_placement();
    Ok(PlacementEditResult {
        changed: true,
        undoable,
    })
}

/// Restore the remembered-home data that existed before the most recent
/// capture or forget. The current row must still match that edit's result;
/// otherwise a newer/outside write wins rather than being overwritten.
pub fn undo_placement_edit(state: &AppState) -> Result<HistoryActionResult, CmdError> {
    let _op = state.lock_window_mutation();
    // Placement edits are serialized by `window_mutation` (held above), not
    // by `config_mutation`: that lock is held across a main-thread hop by the
    // shortcut commands (the global-shortcut plugin registers on the main
    // thread and waits), and this runs *on* the main thread — waiting for it
    // here would deadlock the app. Nothing under `config_mutation` touches the
    // placements table, so no ordering is lost.
    let Some(edit) = state.pop_placement_edit() else {
        return Ok(HistoryActionResult::Empty);
    };
    let Some(identity) = edit.before.as_ref().or(edit.after.as_ref()) else {
        state.restore_placement_edit(edit);
        return Err(CmdError::other(
            "remembered-position undo data is incomplete",
        ));
    };
    let current = state
        .db
        .get_window_placement(&identity.application.bundle_id, identity.slot)?;
    if current != edit.after {
        state.restore_placement_edit(edit);
        return Err(CmdError::other(
            "the remembered position changed again and was not overwritten",
        ));
    }
    let result = match &edit.before {
        Some(before) => state.db.save_window_placement(before),
        None => state
            .db
            .delete_window_placement(&identity.application.bundle_id, identity.slot)
            .map(|_| ()),
    };
    if let Err(error) = result {
        state.restore_placement_edit(edit);
        return Err(error.into());
    }
    state.clear_last_placement();
    Ok(HistoryActionResult::Applied)
}

fn placement_to_recall(
    placements: &[WindowPlacement],
    last: Option<&LastPlacement>,
    bundle_id: &str,
    window_hash: u64,
    current: Rect,
) -> Option<WindowPlacement> {
    let primary = placements
        .iter()
        .find(|placement| placement.slot == PlacementSlot::Primary);
    let secondary = placements
        .iter()
        .find(|placement| placement.slot == PlacementSlot::Secondary);
    let repeat_primary = matches!(
        last,
        Some(last)
            if last.bundle_id == bundle_id
                && last.window_hash == window_hash
                && last.slot == PlacementSlot::Primary
                && geometry::frames_match(last.after, current)
    );
    if repeat_primary {
        secondary.or(primary).cloned()
    } else {
        primary.or(secondary).cloned()
    }
}

/// Restore the focused application's remembered position on its current
/// display. A repeated activation cycles Primary → Secondary when both exist.
pub fn recall_placement(state: &AppState) -> Result<PlacementSlot, CmdError> {
    recall_placement_impl(state, None)
}

/// Context-checked variant used by the settings panel.
pub fn recall_placement_for(
    state: &AppState,
    target: &WindowTarget,
) -> Result<PlacementSlot, CmdError> {
    recall_placement_impl(state, Some(target))
}

fn recall_placement_impl(
    state: &AppState,
    expected: Option<&WindowTarget>,
) -> Result<PlacementSlot, CmdError> {
    // Taken here, in the shared body, so the shortcut/menu path (`recall_*`)
    // and the panel path (`*_for`) are serialized alike.
    let _op = state.lock_window_mutation();
    if !enabled(state) {
        return Err(CmdError::other("window management is disabled"));
    }
    let (focused, previous, area) = focused_context(state, expected)?;
    let placements = state
        .db
        .list_window_placements(&focused.application.bundle_id)?;
    let window_hash = focused.handle.stable_hash();
    let placement = placement_to_recall(
        &placements,
        state.last_placement().as_ref(),
        &focused.application.bundle_id,
        window_hash,
        previous,
    )
    .ok_or_else(|| {
        CmdError::placement_not_found("the focused application has no remembered position")
    })?;
    let target = geometry::recall_frame(placement.frame, area)
        .ok_or_else(|| CmdError::other("the remembered position is invalid"))?;
    let after = apply(state, focused.handle, previous, target)?;
    state.set_last_placement(LastPlacement {
        bundle_id: focused.application.bundle_id,
        slot: placement.slot,
        window_hash,
        after,
    });
    state.clear_last_snap();
    Ok(placement.slot)
}

/// Move to a neighboring display and restore the application's Primary home
/// there as one history entry. Secondary is the fallback when Primary is not
/// configured.
pub fn move_to_display_and_recall(
    state: &AppState,
    direction: DisplayDirection,
) -> Result<MoveRecallResult, CmdError> {
    move_to_display_and_recall_impl(state, direction, None)
}

/// Context-checked variant used by the settings panel.
pub fn move_to_display_and_recall_for(
    state: &AppState,
    target: &WindowTarget,
    direction: DisplayDirection,
) -> Result<MoveRecallResult, CmdError> {
    move_to_display_and_recall_impl(state, direction, Some(target))
}

fn move_to_display_and_recall_impl(
    state: &AppState,
    direction: DisplayDirection,
    expected: Option<&WindowTarget>,
) -> Result<MoveRecallResult, CmdError> {
    // Taken here, in the shared body, so the shortcut/menu path (`recall_*`)
    // and the panel path (`*_for`) are serialized alike.
    let _op = state.lock_window_mutation();
    if !enabled(state) {
        return Err(CmdError::other("window management is disabled"));
    }
    let (focused, previous, _) = focused_context(state, expected)?;
    let placements = state
        .db
        .list_window_placements(&focused.application.bundle_id)?;
    let placement = placements
        .iter()
        .find(|placement| placement.slot == PlacementSlot::Primary)
        .or_else(|| placements.first())
        .cloned()
        .ok_or_else(|| {
            CmdError::placement_not_found("the focused application has no remembered position")
        })?;
    let areas = state.windows.screen_work_areas()?;
    let Some((from, to)) = adjacent_work_area(&areas, previous, direction) else {
        return Ok(MoveRecallResult::NoAdjacentDisplay);
    };
    if from == to {
        return Ok(MoveRecallResult::NoAdjacentDisplay);
    }
    let target = geometry::recall_frame(placement.frame, to)
        .ok_or_else(|| CmdError::other("the remembered position is invalid"))?;
    let window_hash = focused.handle.stable_hash();
    let after = apply(state, focused.handle, previous, target)?;
    state.set_last_placement(LastPlacement {
        bundle_id: focused.application.bundle_id,
        slot: placement.slot,
        window_hash,
        after,
    });
    state.clear_last_snap();
    Ok(MoveRecallResult::Moved {
        slot: placement.slot,
    })
}

/// Restore the most recently moved window to its recorded frame. Entries whose
/// window has since closed are discarded, falling through to the next one; a
/// transient failure keeps its entry so the user can simply retry.
pub fn undo(state: &AppState) -> Result<HistoryActionResult, CmdError> {
    let _op = state.lock_window_mutation();
    if !enabled(state) {
        return Ok(HistoryActionResult::Empty);
    }
    let mut discarded_stale = false;
    while let Some(change) = state.pop_undo() {
        match change.window.set_frame(change.before) {
            Ok(()) => {
                state.push_redo(change);
                state.clear_last_snap();
                state.clear_last_placement();
                return Ok(HistoryActionResult::Applied);
            }
            Err(e) if e.window_gone() => discarded_stale = true,
            Err(e) => {
                state.restore_undo(change);
                return Err(e.into());
            }
        }
    }
    Ok(if discarded_stale {
        HistoryActionResult::StaleEntriesDiscarded
    } else {
        HistoryActionResult::Empty
    })
}

/// Reapply the most recently undone window mutation.
pub fn redo(state: &AppState) -> Result<HistoryActionResult, CmdError> {
    let _op = state.lock_window_mutation();
    if !enabled(state) {
        return Ok(HistoryActionResult::Empty);
    }
    let mut discarded_stale = false;
    while let Some(change) = state.pop_redo() {
        match change.window.set_frame(change.after) {
            Ok(()) => {
                state.push_undo_from_redo(change);
                state.clear_last_snap();
                state.clear_last_placement();
                return Ok(HistoryActionResult::Applied);
            }
            Err(e) if e.window_gone() => discarded_stale = true,
            Err(e) => {
                state.restore_redo(change);
                return Err(e.into());
            }
        }
    }
    Ok(if discarded_stale {
        HistoryActionResult::StaleEntriesDiscarded
    } else {
        HistoryActionResult::Empty
    })
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::{Arc, Mutex};

    use rusqlite::params;
    use tomari_core::{AppSettings, Database};
    use tomari_keyboard::ModifierEngine;
    use tomari_window::{MockWindowManager, compute_frame};

    use super::*;

    fn area() -> Rect {
        Rect::new(0.0, 25.0, 1600.0, 975.0)
    }

    fn state_with(windows: MockWindowManager, settings: AppSettings) -> AppState {
        state_with_db(windows, settings, Database::open_in_memory().unwrap())
    }

    fn state_with_db(windows: MockWindowManager, settings: AppSettings, db: Database) -> AppState {
        AppState::new(
            db,
            ModifierEngine::new(vec![]),
            Box::new(windows),
            settings,
            false,
        )
    }

    fn default_state() -> AppState {
        state_with(MockWindowManager::new(area()), AppSettings::default())
    }

    fn remember(state: &AppState, slot: PlacementSlot, frame: NormalizedRect) {
        state
            .db
            .save_window_placement(&WindowPlacement {
                application: WindowApplication {
                    bundle_id: "com.example.Mock".into(),
                    name: "Mock App".into(),
                },
                slot,
                frame,
            })
            .unwrap();
    }

    fn focused_frame(state: &AppState) -> Rect {
        state.windows.focused_window().unwrap().frame().unwrap()
    }

    fn focused_target(state: &AppState) -> WindowTarget {
        window_target(&state.windows.focused_window_context().unwrap())
    }

    struct IdentitylessWindowManager(MockWindowManager);

    impl tomari_window::WindowManager for IdentitylessWindowManager {
        fn permission_granted(&self) -> bool {
            self.0.permission_granted()
        }

        fn focused_window_context(&self) -> tomari_window::Result<FocusedWindow> {
            Err(tomari_window::Error::Unsupported)
        }

        fn focused_window(&self) -> tomari_window::Result<Box<dyn WindowHandle>> {
            self.0.focused_window()
        }

        fn work_area(&self, window_frame: Rect) -> tomari_window::Result<Rect> {
            self.0.work_area(window_frame)
        }

        fn screen_work_areas(&self) -> tomari_window::Result<Vec<Rect>> {
            self.0.screen_work_areas()
        }

        fn screens_cg(&self) -> tomari_window::Result<Vec<(Rect, Rect)>> {
            self.0.screens_cg()
        }
    }

    #[test]
    fn ordinary_snaps_do_not_require_a_durable_application_identity() {
        let state = AppState::new(
            Database::open_in_memory().unwrap(),
            ModifierEngine::new(Vec::new()),
            Box::new(IdentitylessWindowManager(MockWindowManager::new(area()))),
            AppSettings::default(),
            false,
        );

        assert_eq!(
            snap(&state, WindowPreset::LeftHalf, SnapBehavior::Exact).unwrap(),
            Some(WindowPreset::LeftHalf)
        );
        assert_eq!(
            focused_frame(&state),
            compute_frame(WindowPreset::LeftHalf, area())
        );
        assert!(placement_context(&state).is_err());
    }

    #[test]
    fn focused_context_reads_retry_cannot_complete_once() {
        let attempts = Cell::new(0);
        let value = retry_ax_read_once(|| {
            attempts.set(attempts.get() + 1);
            if attempts.get() == 1 {
                Err(tomari_window::Error::Ax(-25204))
            } else {
                Ok(42)
            }
        });

        assert_eq!(value.unwrap(), 42);
        assert_eq!(attempts.get(), 2);
    }

    #[test]
    fn focused_context_reads_do_not_retry_other_errors() {
        let attempts = Cell::new(0);
        let value = retry_ax_read_once(|| {
            attempts.set(attempts.get() + 1);
            Err::<(), _>(tomari_window::Error::Ax(-25201))
        });

        assert!(matches!(value, Err(tomari_window::Error::Ax(-25201))));
        assert_eq!(attempts.get(), 1);
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

        assert_eq!(undo(&state).unwrap(), HistoryActionResult::Applied);
        assert_eq!(focused_frame(&state), original);
        assert_eq!(state.window_history_status(), (false, true));

        assert_eq!(redo(&state).unwrap(), HistoryActionResult::Applied);
        assert_ne!(focused_frame(&state), original);
        assert_eq!(state.window_history_status(), (true, false));
    }

    #[test]
    fn noop_moves_do_not_pollute_the_history() {
        let state = default_state();
        snap(&state, WindowPreset::Maximize, SnapBehavior::Cycle).unwrap();
        // Already maximized: snapping again must not add a second entry.
        snap(&state, WindowPreset::Maximize, SnapBehavior::Cycle).unwrap();
        // Single display: moving to the next display is a no-op.
        move_to_display(&state, DisplayDirection::Next).unwrap();

        assert!(state.pop_undo().is_some());
        assert!(state.pop_undo().is_none());
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
        assert_eq!(undo(&state).unwrap(), HistoryActionResult::Applied);
        assert_eq!(
            focused_frame(&state),
            compute_frame(WindowPreset::LeftHalf, area())
        );
    }

    #[test]
    fn capture_and_context_use_the_focused_app_and_normalized_work_area() {
        let state = default_state();
        state
            .windows
            .focused_window()
            .unwrap()
            .set_frame(Rect::new(800.0, 25.0, 800.0, 975.0))
            .unwrap();

        let captured =
            capture_placement(&state, &focused_target(&state), PlacementSlot::Primary).unwrap();
        assert!(captured.changed);

        let context = placement_context(&state).unwrap();
        assert_eq!(context.application.name, "Mock App");
        assert_eq!(context.placements.len(), 1);
        assert_eq!(
            context.placements[0].application.bundle_id,
            "com.example.Mock"
        );
        assert_eq!(
            context.placements[0].frame,
            NormalizedRect::new(0.5, 0.0, 0.5, 1.0)
        );
        assert!(!context.can_move_to_display);
    }

    #[test]
    fn repeated_recall_cycles_primary_then_secondary_for_the_same_window() {
        let state = default_state();
        remember(
            &state,
            PlacementSlot::Primary,
            NormalizedRect::new(0.0, 0.0, 0.5, 1.0),
        );
        remember(
            &state,
            PlacementSlot::Secondary,
            NormalizedRect::new(0.5, 0.0, 0.5, 1.0),
        );

        assert_eq!(recall_placement(&state).unwrap(), PlacementSlot::Primary);
        assert_eq!(focused_frame(&state), Rect::new(0.0, 25.0, 800.0, 975.0));
        assert_eq!(recall_placement(&state).unwrap(), PlacementSlot::Secondary);
        assert_eq!(focused_frame(&state), Rect::new(800.0, 25.0, 800.0, 975.0));
    }

    #[test]
    fn moving_and_recalling_applies_the_primary_home_on_the_destination() {
        let mut mock = MockWindowManager::new(area());
        let right = Rect::new(1600.0, 0.0, 1200.0, 800.0);
        mock.areas.push(right);
        let state = state_with(mock, AppSettings::default());
        remember(
            &state,
            PlacementSlot::Primary,
            NormalizedRect::new(0.5, 0.0, 0.5, 1.0),
        );

        assert_eq!(
            move_to_display_and_recall(&state, DisplayDirection::Next).unwrap(),
            MoveRecallResult::Moved {
                slot: PlacementSlot::Primary
            }
        );
        assert_eq!(focused_frame(&state), Rect::new(2200.0, 0.0, 600.0, 800.0));
        assert_eq!(undo(&state).unwrap(), HistoryActionResult::Applied);
        assert_eq!(focused_frame(&state), Rect::new(0.0, 0.0, 100.0, 100.0));
    }

    #[test]
    fn recall_without_a_remembered_position_is_actionable() {
        let state = default_state();
        let err = recall_placement(&state).unwrap_err();
        assert_eq!(err.code, crate::error::ErrorCode::PlacementNotFound);
    }

    #[test]
    fn moving_and_recalling_reports_no_adjacent_display() {
        let state = default_state();
        remember(
            &state,
            PlacementSlot::Primary,
            NormalizedRect::new(0.0, 0.0, 0.5, 1.0),
        );

        assert_eq!(
            move_to_display_and_recall(&state, DisplayDirection::Next).unwrap(),
            MoveRecallResult::NoAdjacentDisplay
        );
        assert_eq!(state.window_history_status(), (false, false));
    }

    #[test]
    fn context_checked_commands_reject_a_stale_window_target() {
        let state = default_state();
        let mut stale = focused_target(&state);
        stale.window_id = "closed-window".into();

        let error = capture_placement(&state, &stale, PlacementSlot::Primary).unwrap_err();
        assert_eq!(error.code, crate::error::ErrorCode::WindowTargetChanged);
        assert!(
            state
                .db
                .list_window_placements("com.example.Mock")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn remembered_position_replacement_and_forget_are_undoable() {
        let state = default_state();
        let old = NormalizedRect::new(0.0, 0.0, 0.5, 1.0);
        remember(&state, PlacementSlot::Primary, old);
        state
            .windows
            .focused_window()
            .unwrap()
            .set_frame(Rect::new(800.0, 25.0, 800.0, 975.0))
            .unwrap();
        let target = focused_target(&state);

        assert!(
            capture_placement(&state, &target, PlacementSlot::Primary)
                .unwrap()
                .changed
        );
        assert_eq!(
            undo_placement_edit(&state).unwrap(),
            HistoryActionResult::Applied
        );
        assert_eq!(
            state
                .db
                .get_window_placement("com.example.Mock", PlacementSlot::Primary)
                .unwrap()
                .unwrap()
                .frame,
            old
        );

        assert!(
            forget_placement(&state, &target, PlacementSlot::Primary)
                .unwrap()
                .changed
        );
        assert_eq!(
            undo_placement_edit(&state).unwrap(),
            HistoryActionResult::Applied
        );
        assert_eq!(
            state
                .db
                .get_window_placement("com.example.Mock", PlacementSlot::Primary)
                .unwrap()
                .unwrap()
                .frame,
            old
        );
    }

    #[test]
    fn malformed_remembered_positions_can_be_captured_or_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("placements.sqlite3");
        let db = Database::open(&path).unwrap();
        let raw = rusqlite::Connection::open(&path).unwrap();
        raw.execute(
            "INSERT INTO window_placements (bundle_id, app_name, slot, frame)
             VALUES (?1, ?2, ?3, ?4)",
            params!["com.example.Mock", "Mock App", "primary", "not-json"],
        )
        .unwrap();
        let state = state_with_db(MockWindowManager::new(area()), AppSettings::default(), db);
        let target = focused_target(&state);

        let captured = capture_placement(&state, &target, PlacementSlot::Primary).unwrap();
        assert_eq!(
            captured,
            PlacementEditResult {
                changed: true,
                undoable: true,
            }
        );
        assert!(
            state
                .db
                .get_window_placement("com.example.Mock", PlacementSlot::Primary)
                .unwrap()
                .is_some()
        );
        assert_eq!(
            undo_placement_edit(&state).unwrap(),
            HistoryActionResult::Applied
        );

        raw.execute(
            "INSERT INTO window_placements (bundle_id, app_name, slot, frame)
             VALUES (?1, ?2, ?3, ?4)",
            params!["com.example.Mock", "Mock App", "primary", "still-not-json"],
        )
        .unwrap();
        let forgotten = forget_placement(&state, &target, PlacementSlot::Primary).unwrap();
        assert_eq!(
            forgotten,
            PlacementEditResult {
                changed: true,
                undoable: false,
            }
        );
        assert!(
            state
                .db
                .get_window_placement("com.example.Mock", PlacementSlot::Primary)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            undo_placement_edit(&state).unwrap(),
            HistoryActionResult::Empty
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
        assert_eq!(undo(&state).unwrap(), HistoryActionResult::Empty);
        assert_eq!(state.window_history_status(), (false, false));
    }

    #[test]
    fn undo_with_empty_history_is_a_no_op() {
        let state = default_state();
        assert_eq!(undo(&state).unwrap(), HistoryActionResult::Empty);
    }

    #[derive(Clone)]
    struct SharedHandle(Arc<Mutex<Rect>>);

    impl WindowHandle for SharedHandle {
        fn frame(&self) -> tomari_window::Result<Rect> {
            Ok(*self.0.lock().unwrap())
        }

        fn set_frame(&self, frame: Rect) -> tomari_window::Result<()> {
            *self.0.lock().unwrap() = frame;
            Ok(())
        }

        fn stable_hash(&self) -> u64 {
            Arc::as_ptr(&self.0) as u64
        }
    }

    #[test]
    fn dragged_snap_undo_returns_to_the_mouse_down_frame() {
        let state = default_state();
        let start = Rect::new(240.0, 180.0, 700.0, 500.0);
        let released_at_edge = Rect::new(0.0, 25.0, 700.0, 500.0);
        let snapped = compute_frame(WindowPreset::LeftHalf, area());
        let handle = SharedHandle(Arc::new(Mutex::new(released_at_edge)));

        assert!(apply_dragged(&state, &handle, start, || Some(snapped)));
        assert_eq!(undo(&state).unwrap(), HistoryActionResult::Applied);
        assert_eq!(handle.frame().unwrap(), start);
    }

    #[test]
    fn a_declined_drop_decision_moves_nothing_and_records_nothing() {
        let state = default_state();
        let start = Rect::new(240.0, 180.0, 700.0, 500.0);
        let released_at_edge = Rect::new(0.0, 25.0, 700.0, 500.0);
        let handle = SharedHandle(Arc::new(Mutex::new(released_at_edge)));

        assert!(!apply_dragged(&state, &handle, start, || None));
        assert_eq!(handle.frame().unwrap(), released_at_edge);
        assert_eq!(state.window_history_status(), (false, false));
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
        state.push_window_change(WindowChange {
            window: Box::new(FailingHandle::gone()),
            before: Rect::new(0.0, 0.0, 1.0, 1.0),
            after: Rect::new(1.0, 1.0, 1.0, 1.0),
        });

        assert_eq!(undo(&state).unwrap(), HistoryActionResult::Applied);
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
        state.push_window_change(WindowChange {
            window: Box::new(FailingHandle::transient()),
            before: frame,
            after: Rect::new(10.0, 30.0, 300.0, 200.0),
        });

        assert!(undo(&state).is_err());
        let kept = state.pop_undo();
        assert_eq!(
            kept.map(|change| change.before),
            Some(frame),
            "entry stays for a retry"
        );
    }

    #[test]
    fn undo_reports_when_only_closed_window_entries_were_discarded() {
        let state = default_state();
        state.push_window_change(WindowChange {
            window: Box::new(FailingHandle::gone()),
            before: Rect::new(0.0, 0.0, 1.0, 1.0),
            after: Rect::new(1.0, 1.0, 1.0, 1.0),
        });

        assert_eq!(
            undo(&state).unwrap(),
            HistoryActionResult::StaleEntriesDiscarded
        );
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
