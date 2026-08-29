//! Shared application state managed by Tauri and accessed from commands, the
//! tray, the global-shortcut handler and the keyboard event tap.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use tauri_plugin_global_shortcut::Shortcut;
use tomari_core::{
    AppAction, AppSettings, Database, PlacementSlot, Rect, WindowPlacement, WindowPreset,
};
use tomari_keyboard::ModifierEngine;
use tomari_window::{WindowHandle, WindowManager};

use crate::keepawake::KeepAwake;
use crate::lifecycle::AppLifecycle;
use crate::locks::MutexExt;
use crate::menubar::MenuBarState;

/// How many window frames the undo history keeps before dropping the oldest.
const WINDOW_HISTORY_CAP: usize = 50;

/// How many remembered-position edits can be recovered from the panel.
const PLACEMENT_EDIT_HISTORY_CAP: usize = 20;

/// What the most recent preset snap did, so a repeated press of the same
/// request can advance its cycle (1/2 → 1/3 → 2/3) instead of re-applying.
#[derive(Debug, Clone, Copy)]
pub struct LastSnap {
    /// The preset the user asked for.
    pub requested: WindowPreset,
    /// The preset the snap actually applied (a cycle member of `requested`).
    pub applied: WindowPreset,
    /// Identity of the snapped window ([`WindowHandle::stable_hash`]), so a
    /// different window that merely shares the frame does not continue the
    /// cycle.
    pub window_hash: u64,
    /// The frame the window ended up with, read back after the move — used to
    /// confirm the next press still targets the same, unmoved window.
    pub after: Rect,
}

/// What the most recent remembered-position action applied, so repeating it on
/// the same unmoved window can alternate between Primary and Secondary.
#[derive(Debug, Clone)]
pub struct LastPlacement {
    pub bundle_id: String,
    pub slot: PlacementSlot,
    pub window_hash: u64,
    pub after: Rect,
}

/// One reversible window mutation. The handle retargets the window even after
/// another app receives focus, so undo/redo acts on what actually moved.
pub struct WindowChange {
    pub window: Box<dyn WindowHandle>,
    pub before: Rect,
    pub after: Rect,
}

#[derive(Default)]
struct WindowHistory {
    undo: Vec<WindowChange>,
    redo: Vec<WindowChange>,
}

/// One reversible edit to the persistent remembered-home data. This is kept
/// separate from [`WindowChange`]: restoring a window frame and restoring a
/// deleted/replaced saved home are different user intentions.
#[derive(Debug, Clone)]
pub struct PlacementEdit {
    pub before: Option<WindowPlacement>,
    pub after: Option<WindowPlacement>,
}

/// The display-geometry cache and the generation it was published under (see
/// `AppState::screen_geometry_snapshot`).
#[derive(Default)]
struct ScreenGeometry {
    generation: u64,
    screens: Vec<(Rect, Rect)>,
}

pub struct AppState {
    /// One-way process lifecycle and gates for work that could recreate an OS
    /// side effect after shutdown has begun.
    pub lifecycle: Arc<AppLifecycle>,
    /// Persistent SQLite store.
    pub db: Database,
    /// The tap/hold modifier engine, kept in sync with the stored rules.
    pub engine: Mutex<ModifierEngine>,
    /// Platform window controller (Accessibility on macOS).
    pub windows: Box<dyn WindowManager + Send + Sync>,
    /// In-memory cache of the current settings.
    pub settings: Mutex<AppSettings>,
    /// Registered global shortcuts mapped to the action they fire.
    pub shortcuts: Mutex<HashMap<Shortcut, AppAction>>,
    /// Whether the most recent registration pass left the OS shortcut set out
    /// of step with the stored keyboard configuration. Kept separately from
    /// the dispatch map because an unregister failure can leave registrations
    /// live while dispatch is still blocked by the keyboard master switch.
    shortcut_registration_incomplete: AtomicBool,
    /// Reversible window mutations, in memory only because handles are
    /// meaningless across a relaunch.
    window_history: Mutex<WindowHistory>,
    /// Recent edits to remembered-home data, newest last. These values are
    /// meaningful across window focus changes but not across app relaunches.
    placement_edit_history: Mutex<Vec<PlacementEdit>>,
    /// The most recent preset snap, for hotkey-repeat cycling.
    last_snap: Mutex<Option<LastSnap>>,
    /// The most recent remembered position, for Primary/Secondary cycling.
    last_placement: Mutex<Option<LastPlacement>>,
    /// Cached display geometry — each display's `(full_frame, work_area)` in CG
    /// coordinates — for drag-to-snap, with the generation it was published
    /// under. Refreshed from the main thread (the only place AppKit's
    /// per-display frames are readable) so the drag-to-snap tap thread can
    /// resolve snap zones without a blocking main-thread round-trip. The
    /// generation advances on every refresh, so a gesture that snapshotted the
    /// geometry when it armed can tell, right before it applies, that a display
    /// was unplugged, rearranged or resized in the meantime.
    screen_geometry: Mutex<ScreenGeometry>,
    /// Serializes whole-config mutations. A save or delete writes to the
    /// database and then rebuilds the live engines/shortcuts to match; two of
    /// them running at once would leave the in-memory state out of sync with
    /// disk. Every save/delete command holds this for its whole operation, so
    /// they never interleave. It guards the *sequence* of operations, not a
    /// value, hence `Mutex<()>`.
    config_mutation: Mutex<()>,
    #[cfg(test)]
    config_mutation_waiters: AtomicUsize,
    /// Serializes window mutations end to end. A snap, recall, undo or redo
    /// is a sequence — read the history, move the window over Accessibility,
    /// record the result — reachable from the main thread (global shortcuts,
    /// modifier taps, the panel) and from the drag-to-snap worker at once.
    /// The history's own lock guards each push/pop, not the sequence: an undo
    /// that popped its entry, then lost the CPU to a snap that cleared the redo
    /// branch, would push the stale entry back onto redo after it. Every
    /// window operation in `window_ops` holds this for its whole run.
    window_mutation: Mutex<()>,
    /// Sleep-prevention ("keep awake") runtime state. Not persisted — always
    /// starts inactive at launch. See [`crate::keepawake`].
    pub keep_awake: Mutex<KeepAwake>,
    /// Whether the tidied menu bar items are expanded, plus the auto-collapse
    /// deadline. Runtime state like keep-awake: a launch starts collapsed. See
    /// [`crate::menubar`].
    pub menu_bar: Mutex<MenuBarState>,
    /// Whether this launch is a true first run — the database was pristine and
    /// the defaults were just seeded. `setup` reads it to auto-open the
    /// settings window once; every uncertain detection lands on `false` so an
    /// existing user is never surprised by a window.
    pub first_run: bool,
    /// Whether this launch looks like an app update silently revoked
    /// previously granted permissions (see [`crate::regrant`]). Computed once
    /// in `setup` — it needs the initial permission read — and `false` until
    /// then, the safe side for the frontend's `setup_status` pull.
    update_regrant: AtomicBool,
    /// Monotonic origin for the millisecond timestamps fed to the engines.
    epoch: Instant,
}

impl AppState {
    pub fn new(
        db: Database,
        engine: ModifierEngine,
        windows: Box<dyn WindowManager + Send + Sync>,
        settings: AppSettings,
        first_run: bool,
    ) -> Self {
        let menu_bar = MenuBarState::new(settings.menu_bar_auto_collapse_secs);
        Self {
            lifecycle: Arc::new(AppLifecycle::default()),
            db,
            engine: Mutex::new(engine),
            windows,
            settings: Mutex::new(settings),
            shortcuts: Mutex::new(HashMap::new()),
            shortcut_registration_incomplete: AtomicBool::new(false),
            window_history: Mutex::new(WindowHistory::default()),
            placement_edit_history: Mutex::new(Vec::new()),
            last_snap: Mutex::new(None),
            last_placement: Mutex::new(None),
            screen_geometry: Mutex::new(ScreenGeometry::default()),
            config_mutation: Mutex::new(()),
            #[cfg(test)]
            config_mutation_waiters: AtomicUsize::new(0),
            window_mutation: Mutex::new(()),
            keep_awake: Mutex::new(KeepAwake::default()),
            menu_bar: Mutex::new(menu_bar),
            first_run,
            update_regrant: AtomicBool::new(false),
            epoch: Instant::now(),
        }
    }

    /// Record the update-regrant detection result (see [`crate::regrant`]).
    pub fn set_update_regrant(&self, value: bool) {
        self.update_regrant.store(value, Ordering::Relaxed);
    }

    /// Whether this launch detected an update-caused permission revocation.
    pub fn update_regrant(&self) -> bool {
        self.update_regrant.load(Ordering::Relaxed)
    }

    /// Acquire the config-mutation lock for the duration of a save or delete.
    /// Hold the returned guard for the whole operation — DB write *and* the
    /// live-state sync that follows — so config mutations stay serialized and
    /// the in-memory engines never disagree with the database. Returns `None`
    /// after shutdown begins; checking the lifecycle only after acquiring the
    /// lock closes the race where a command waited behind another save.
    ///
    /// Never wait for this on the main thread. Its holders run off the main
    /// thread and, while holding it, re-register global shortcuts — which the
    /// plugin performs *on* the main thread, waiting for it — so a main-thread
    /// caller blocked here would deadlock the app. Window operations use
    /// [`Self::lock_window_mutation`] instead.
    pub fn lock_config_mutation(&self) -> Option<std::sync::MutexGuard<'_, ()>> {
        #[cfg(test)]
        self.config_mutation_waiters.fetch_add(1, Ordering::SeqCst);
        let guard = self.config_mutation.lock_safe();
        #[cfg(test)]
        self.config_mutation_waiters.fetch_sub(1, Ordering::SeqCst);
        self.lifecycle.is_running().then_some(guard)
    }

    #[cfg(test)]
    pub(crate) fn config_mutation_waiters_for_test(&self) -> usize {
        self.config_mutation_waiters.load(Ordering::SeqCst)
    }

    /// Wait for a config mutation that already crossed the lifecycle gate.
    /// The shutdown coordinator calls this off the main thread because an
    /// in-flight shortcut registration may itself be waiting on that thread.
    pub(crate) fn drain_config_mutations_for_shutdown(&self) {
        drop(self.config_mutation.lock_safe());
    }

    /// Acquire the window-mutation lock for one whole window operation (see
    /// the field). Only the entry points in `window_ops` take it; the helpers
    /// they call must not, or a nested call would deadlock.
    pub fn lock_window_mutation(&self) -> std::sync::MutexGuard<'_, ()> {
        self.window_mutation.lock_safe()
    }

    pub(crate) fn set_shortcut_registration_incomplete(&self, incomplete: bool) {
        self.shortcut_registration_incomplete
            .store(incomplete, Ordering::Relaxed);
    }

    pub(crate) fn shortcut_registration_incomplete(&self) -> bool {
        self.shortcut_registration_incomplete
            .load(Ordering::Relaxed)
    }

    /// The cached display geometry for drag-to-snap (`(full_frame, work_area)`
    /// per display, CG coordinates). Empty until first refreshed.
    /// The cached display geometry together with its generation, read as one
    /// snapshot so the two cannot come from different refreshes.
    pub fn screen_geometry_snapshot(&self) -> (u64, Vec<(Rect, Rect)>) {
        let cached = self.screen_geometry.lock_safe();
        (cached.generation, cached.screens.clone())
    }

    /// The generation of the cached display geometry: changes whenever a
    /// refresh has published new geometry since a snapshot was taken.
    pub fn screen_geometry_generation(&self) -> u64 {
        self.screen_geometry.lock_safe().generation
    }

    /// Replace the cached display geometry, advancing its generation. Called
    /// from the main thread.
    pub fn set_screen_geometry(&self, screens: Vec<(Rect, Rect)>) {
        let mut cached = self.screen_geometry.lock_safe();
        cached.screens = screens;
        cached.generation = cached.generation.wrapping_add(1);
    }

    /// Re-read the display geometry from the window manager into the cache.
    /// Must run on the main thread — AppKit's per-display frames are only
    /// readable there (off it, `screens_cg` degrades to the main display).
    pub fn refresh_screen_geometry(&self) {
        if let Ok(screens) = self.windows.screens_cg() {
            self.set_screen_geometry(screens);
        }
    }

    /// Record a new user-visible change and discard the redo branch it replaces.
    pub fn push_window_change(&self, change: WindowChange) {
        let mut history = self.window_history.lock_safe();
        if history.undo.len() == WINDOW_HISTORY_CAP {
            history.undo.remove(0);
        }
        history.undo.push(change);
        history.redo.clear();
    }

    pub fn pop_undo(&self) -> Option<WindowChange> {
        self.window_history.lock_safe().undo.pop()
    }

    pub fn restore_undo(&self, change: WindowChange) {
        self.window_history.lock_safe().undo.push(change);
    }

    pub fn push_redo(&self, change: WindowChange) {
        self.window_history.lock_safe().redo.push(change);
    }

    pub fn pop_redo(&self) -> Option<WindowChange> {
        self.window_history.lock_safe().redo.pop()
    }

    pub fn restore_redo(&self, change: WindowChange) {
        self.window_history.lock_safe().redo.push(change);
    }

    pub fn push_undo_from_redo(&self, change: WindowChange) {
        let mut history = self.window_history.lock_safe();
        if history.undo.len() == WINDOW_HISTORY_CAP {
            history.undo.remove(0);
        }
        history.undo.push(change);
    }

    pub fn window_history_status(&self) -> (bool, bool) {
        let history = self.window_history.lock_safe();
        (!history.undo.is_empty(), !history.redo.is_empty())
    }

    /// Record a persistent home edit, keeping enough information to restore
    /// the value that existed immediately before it.
    pub fn push_placement_edit(&self, edit: PlacementEdit) {
        let mut history = self.placement_edit_history.lock_safe();
        if history.len() == PLACEMENT_EDIT_HISTORY_CAP {
            history.remove(0);
        }
        history.push(edit);
    }

    pub fn pop_placement_edit(&self) -> Option<PlacementEdit> {
        self.placement_edit_history.lock_safe().pop()
    }

    pub fn restore_placement_edit(&self, edit: PlacementEdit) {
        self.placement_edit_history.lock_safe().push(edit);
    }

    /// The most recent preset snap, for hotkey-repeat cycling.
    pub fn last_snap(&self) -> Option<LastSnap> {
        *self.last_snap.lock_safe()
    }

    pub fn set_last_snap(&self, snap: LastSnap) {
        *self.last_snap.lock_safe() = Some(snap);
    }

    /// Forget the cycle state so the next snap starts fresh — used after an
    /// exact (non-cycling) snap, which sits outside the cycle.
    pub fn clear_last_snap(&self) {
        *self.last_snap.lock_safe() = None;
    }

    pub fn last_placement(&self) -> Option<LastPlacement> {
        self.last_placement.lock_safe().clone()
    }

    pub fn set_last_placement(&self, placement: LastPlacement) {
        *self.last_placement.lock_safe() = Some(placement);
    }

    pub fn clear_last_placement(&self) {
        *self.last_placement.lock_safe() = None;
    }

    /// Milliseconds since this state was built — the clock both the event tap
    /// and dispatched leader arming share, so their timestamps are comparable.
    pub fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    /// Whether keyboard customization is currently enabled.
    pub fn keyboard_enabled(&self) -> bool {
        self.settings.lock_safe().keyboard_enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_screen_geometry_refresh_advances_the_generation() {
        let state = state();
        let (g0, screens) = state.screen_geometry_snapshot();
        assert!(screens.is_empty());
        let display = (
            Rect::new(0.0, 0.0, 100.0, 100.0),
            Rect::new(0.0, 0.0, 100.0, 90.0),
        );
        state.set_screen_geometry(vec![display]);
        let (g1, screens) = state.screen_geometry_snapshot();
        assert_ne!(g0, g1);
        assert_eq!(screens, vec![display]);
        // Publishing the very same geometry again still counts as a refresh:
        // a gesture cannot tell "unchanged" from "changed back" and must
        // re-validate either way.
        state.set_screen_geometry(vec![display]);
        assert_ne!(state.screen_geometry_generation(), g1);
    }
    use tomari_core::WindowPreset;
    use tomari_keyboard::ModifierEngine;
    use tomari_window::MockWindowManager;

    fn state() -> AppState {
        AppState::new(
            Database::open_in_memory().unwrap(),
            ModifierEngine::new(vec![]),
            Box::new(MockWindowManager::new(Rect::new(0.0, 0.0, 100.0, 100.0))),
            AppSettings::default(),
            false,
        )
    }

    /// A minimal handle that only needs to occupy a history slot; the frame
    /// pushed alongside it is what the assertions identify entries by.
    struct DummyHandle;

    impl WindowHandle for DummyHandle {
        fn frame(&self) -> tomari_window::Result<Rect> {
            Ok(Rect::new(0.0, 0.0, 0.0, 0.0))
        }
        fn set_frame(&self, _frame: Rect) -> tomari_window::Result<()> {
            Ok(())
        }
        fn stable_hash(&self) -> u64 {
            0
        }
    }

    /// A frame tagged by its `x`, so popped entries can be identified by order.
    fn frame_at(i: usize) -> Rect {
        Rect::new(i as f64, 0.0, 10.0, 10.0)
    }

    fn push(state: &AppState, i: usize) {
        state.push_window_change(WindowChange {
            window: Box::new(DummyHandle),
            before: frame_at(i),
            after: frame_at(i + 100),
        });
    }

    #[test]
    fn history_pops_in_lifo_order() {
        let state = state();
        push(&state, 1);
        push(&state, 2);
        push(&state, 3);

        assert_eq!(state.pop_undo().unwrap().before, frame_at(3));
        assert_eq!(state.pop_undo().unwrap().before, frame_at(2));
        assert_eq!(state.pop_undo().unwrap().before, frame_at(1));
        assert!(state.pop_undo().is_none());
    }

    #[test]
    fn history_caps_at_fifty_dropping_the_oldest() {
        let state = state();
        // One past the cap: the oldest (frame 0) must fall off the front.
        for i in 0..=WINDOW_HISTORY_CAP {
            push(&state, i);
        }

        // Newest first, exactly `WINDOW_HISTORY_CAP` entries, down to frame 1.
        assert_eq!(
            state.pop_undo().unwrap().before,
            frame_at(WINDOW_HISTORY_CAP)
        );
        let mut count = 1;
        let mut oldest = WINDOW_HISTORY_CAP;
        while let Some(change) = state.pop_undo() {
            oldest = change.before.x as usize;
            count += 1;
        }
        assert_eq!(count, WINDOW_HISTORY_CAP, "only the cap many are retained");
        assert_eq!(oldest, 1, "frame 0 was dropped as the oldest");
    }

    #[test]
    fn a_new_change_discards_the_redo_branch() {
        let state = state();
        push(&state, 1);
        let undone = state.pop_undo().unwrap();
        state.push_redo(undone);
        assert_eq!(state.window_history_status(), (false, true));

        push(&state, 2);
        assert_eq!(state.window_history_status(), (true, false));
    }

    #[test]
    fn last_snap_round_trips_and_clears() {
        let state = state();
        assert!(state.last_snap().is_none());

        let snap = LastSnap {
            requested: WindowPreset::LeftHalf,
            applied: WindowPreset::LeftThird,
            window_hash: 42,
            after: frame_at(7),
        };
        state.set_last_snap(snap);

        let got = state.last_snap().expect("snap stored");
        assert_eq!(got.requested, WindowPreset::LeftHalf);
        assert_eq!(got.applied, WindowPreset::LeftThird);
        assert_eq!(got.window_hash, 42);
        assert_eq!(got.after, frame_at(7));

        state.clear_last_snap();
        assert!(state.last_snap().is_none());
    }
}
