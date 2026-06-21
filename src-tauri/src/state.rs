//! Shared application state managed by Tauri and accessed from commands, the
//! tray, the global-shortcut handler and the keyboard event tap.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use tauri_plugin_global_shortcut::Shortcut;
use tomari_core::{AppAction, AppSettings, Database, Rect, WindowPreset};
use tomari_keyboard::ModifierEngine;
use tomari_window::{WindowHandle, WindowManager};

use crate::keepawake::KeepAwake;
use crate::locks::MutexExt;

/// How many window frames the undo history keeps before dropping the oldest.
const WINDOW_HISTORY_CAP: usize = 50;

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

pub struct AppState {
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
    /// The windows moved by window actions paired with the frame each held
    /// beforehand, newest last, so Undo restores the window that was actually
    /// moved (not whatever is focused later). In-memory only: handles are
    /// meaningless across a relaunch.
    window_history: Mutex<Vec<(Box<dyn WindowHandle>, Rect)>>,
    /// The most recent preset snap, for hotkey-repeat cycling.
    last_snap: Mutex<Option<LastSnap>>,
    /// Cached display geometry — each display's `(full_frame, work_area)` in CG
    /// coordinates — for drag-to-snap. Refreshed from the main thread (the only
    /// place AppKit's per-display frames are readable) so the drag-to-snap tap
    /// thread can resolve snap zones without a blocking main-thread round-trip.
    screen_geometry: Mutex<Vec<(Rect, Rect)>>,
    /// Serializes whole-config mutations. A save or delete writes to the
    /// database and then rebuilds the live engines/shortcuts to match; two of
    /// them running at once would leave the in-memory state out of sync with
    /// disk. Every save/delete command holds this for its whole operation, so
    /// they never interleave. It guards the *sequence* of operations, not a
    /// value, hence `Mutex<()>`.
    config_mutation: Mutex<()>,
    /// Sleep-prevention ("keep awake") runtime state. Not persisted — always
    /// starts inactive at launch. See [`crate::keepawake`].
    pub keep_awake: Mutex<KeepAwake>,
    /// Monotonic origin for the millisecond timestamps fed to the engines.
    epoch: Instant,
}

impl AppState {
    pub fn new(
        db: Database,
        engine: ModifierEngine,
        windows: Box<dyn WindowManager + Send + Sync>,
        settings: AppSettings,
    ) -> Self {
        Self {
            db,
            engine: Mutex::new(engine),
            windows,
            settings: Mutex::new(settings),
            shortcuts: Mutex::new(HashMap::new()),
            window_history: Mutex::new(Vec::new()),
            last_snap: Mutex::new(None),
            screen_geometry: Mutex::new(Vec::new()),
            config_mutation: Mutex::new(()),
            keep_awake: Mutex::new(KeepAwake::default()),
            epoch: Instant::now(),
        }
    }

    /// Acquire the config-mutation lock for the duration of a save or delete.
    /// Hold the returned guard for the whole operation — DB write *and* the
    /// live-state sync that follows — so config mutations stay serialized and
    /// the in-memory engines never disagree with the database.
    pub fn lock_config_mutation(&self) -> std::sync::MutexGuard<'_, ()> {
        self.config_mutation.lock_safe()
    }

    /// The cached display geometry for drag-to-snap (`(full_frame, work_area)`
    /// per display, CG coordinates). Empty until first refreshed.
    pub fn screen_geometry(&self) -> Vec<(Rect, Rect)> {
        self.screen_geometry.lock_safe().clone()
    }

    /// Replace the cached display geometry. Called from the main thread.
    pub fn set_screen_geometry(&self, screens: Vec<(Rect, Rect)>) {
        *self.screen_geometry.lock_safe() = screens;
    }

    /// Re-read the display geometry from the window manager into the cache.
    /// Must run on the main thread — AppKit's per-display frames are only
    /// readable there (off it, `screens_cg` degrades to the main display).
    pub fn refresh_screen_geometry(&self) {
        if let Ok(screens) = self.windows.screens_cg() {
            self.set_screen_geometry(screens);
        }
    }

    /// Record a window and the frame it held before a window action moved it.
    pub fn push_window_history(&self, window: Box<dyn WindowHandle>, frame: Rect) {
        let mut history = self.window_history.lock_safe();
        if history.len() == WINDOW_HISTORY_CAP {
            history.remove(0);
        }
        history.push((window, frame));
    }

    /// Take the most recently recorded window/frame pair, if any.
    pub fn pop_window_history(&self) -> Option<(Box<dyn WindowHandle>, Rect)> {
        self.window_history.lock_safe().pop()
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
    use tomari_core::WindowPreset;
    use tomari_keyboard::ModifierEngine;
    use tomari_window::MockWindowManager;

    fn state() -> AppState {
        AppState::new(
            Database::open_in_memory().unwrap(),
            ModifierEngine::new(vec![]),
            Box::new(MockWindowManager::new(Rect::new(0.0, 0.0, 100.0, 100.0))),
            AppSettings::default(),
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
        state.push_window_history(Box::new(DummyHandle), frame_at(i));
    }

    #[test]
    fn history_pops_in_lifo_order() {
        let state = state();
        push(&state, 1);
        push(&state, 2);
        push(&state, 3);

        assert_eq!(state.pop_window_history().unwrap().1, frame_at(3));
        assert_eq!(state.pop_window_history().unwrap().1, frame_at(2));
        assert_eq!(state.pop_window_history().unwrap().1, frame_at(1));
        assert!(state.pop_window_history().is_none());
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
            state.pop_window_history().unwrap().1,
            frame_at(WINDOW_HISTORY_CAP)
        );
        let mut count = 1;
        let mut oldest = WINDOW_HISTORY_CAP;
        while let Some((_, frame)) = state.pop_window_history() {
            oldest = frame.x as usize;
            count += 1;
        }
        assert_eq!(count, WINDOW_HISTORY_CAP, "only the cap many are retained");
        assert_eq!(oldest, 1, "frame 0 was dropped as the oldest");
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
