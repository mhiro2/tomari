//! One-way application shutdown and the gates around restartable runtime work.
//!
//! Tomari owns process-external state (the Caps Lock HID mapping and the
//! lid-close sleep override), so exiting is a coordinated transition rather
//! than letting process teardown drop whatever happens to be live. Once the
//! lifecycle leaves [`Phase::Running`], no worker or runtime effect may start
//! again. The leader drains work that already crossed the gate, performs the
//! cleanup once, and releases any concurrent shutdown callers only after the
//! process-external state has been restored.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use tauri::{AppHandle, ExitRequestApi, Manager};

use crate::locks::MutexExt;
use crate::state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Running,
    ShuttingDown,
    Stopped,
}

struct Inner {
    phase: Phase,
    workers: Vec<JoinHandle<()>>,
}

impl Inner {
    fn reap_finished_workers(&mut self) {
        let mut index = 0;
        while index < self.workers.len() {
            if self.workers[index].is_finished() {
                let worker = self.workers.swap_remove(index);
                warn_if_worker_panicked(worker);
            } else {
                index += 1;
            }
        }
    }
}

fn warn_if_worker_panicked(worker: JoinHandle<()>) {
    if worker.join().is_err() {
        tracing::warn!("a lifecycle worker panicked");
    }
}

/// App-wide terminal state, tracked workers, and the serialization gate for
/// restartable tap/Caps effects and transient synthetic input that must finish
/// before cleanup can release the process-owned input machinery.
pub struct AppLifecycle {
    inner: Mutex<Inner>,
    changed: Condvar,
    runtime_effect: Mutex<()>,
}

impl Default for AppLifecycle {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Inner {
                phase: Phase::Running,
                workers: Vec::new(),
            }),
            changed: Condvar::new(),
            runtime_effect: Mutex::new(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShutdownClaim {
    Leader,
    InProgress,
    Complete,
}

impl AppLifecycle {
    pub fn is_running(&self) -> bool {
        self.inner.lock_safe().phase == Phase::Running
    }

    /// Register a worker atomically with respect to shutdown. A worker that
    /// loses the race is not spawned; one that wins remains owned until a
    /// later registration reaps it or shutdown joins it.
    pub fn spawn_tracked<F>(self: &Arc<Self>, name: &str, task: F) -> std::io::Result<bool>
    where
        F: FnOnce(Arc<Self>) + Send + 'static,
    {
        let mut inner = self.inner.lock_safe();
        if inner.phase != Phase::Running {
            return Ok(false);
        }
        inner.reap_finished_workers();
        let lifecycle = Arc::clone(self);
        let worker = std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || task(lifecycle))?;
        inner.workers.push(worker);
        Ok(true)
    }

    /// Interruptible replacement for a process-lifetime worker's `sleep`.
    /// Returns `true` as soon as shutdown begins, otherwise `false` after the
    /// timeout expires.
    pub fn wait_for_shutdown(&self, timeout: Duration) -> bool {
        let inner = self.inner.lock_safe();
        if inner.phase != Phase::Running {
            return true;
        }
        let (inner, _) = self
            .changed
            .wait_timeout_while(inner, timeout, |inner| inner.phase == Phase::Running)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.phase != Phase::Running
    }

    /// Enter a terminally gated OS effect. Checking the phase only after
    /// acquiring the effect mutex closes the race where shutdown starts while
    /// a permission/wake/save path or synthetic gesture is queued behind an
    /// effect already in progress.
    pub fn runtime_effect(&self) -> Option<MutexGuard<'_, ()>> {
        let effect = self.runtime_effect.lock_safe();
        self.is_running().then_some(effect)
    }

    fn claim_shutdown(&self) -> ShutdownClaim {
        let mut inner = self.inner.lock_safe();
        match inner.phase {
            Phase::Running => {
                inner.phase = Phase::ShuttingDown;
                self.changed.notify_all();
                ShutdownClaim::Leader
            }
            Phase::ShuttingDown => ShutdownClaim::InProgress,
            Phase::Stopped => ShutdownClaim::Complete,
        }
    }

    fn join_workers(&self) {
        let workers = {
            let mut inner = self.inner.lock_safe();
            debug_assert_ne!(inner.phase, Phase::Running);
            std::mem::take(&mut inner.workers)
        };
        for worker in workers {
            warn_if_worker_panicked(worker);
        }
    }

    fn drain_runtime_effects(&self) {
        drop(self.runtime_effect.lock_safe());
    }

    fn finish_shutdown(&self) {
        let mut inner = self.inner.lock_safe();
        inner.phase = Phase::Stopped;
        self.changed.notify_all();
    }

    fn wait_until_stopped(&self) {
        let inner = self.inner.lock_safe();
        drop(
            self.changed
                .wait_while(inner, |inner| inner.phase != Phase::Stopped)
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
    }

    /// Make the lifecycle terminal without invoking platform cleanup. This
    /// mirrors the runtime-effect gate used by shutdown while keeping unit
    /// tests independent from Tauri and macOS resources.
    #[cfg(test)]
    pub(crate) fn stop_for_test(&self) {
        match self.claim_shutdown() {
            ShutdownClaim::Leader => {
                self.drain_runtime_effects();
                self.finish_shutdown();
            }
            ShutdownClaim::InProgress => self.wait_until_stopped(),
            ShutdownClaim::Complete => {}
        }
    }
}

trait ShutdownOps {
    fn cancel_workers(&self);
    fn drain_config_mutations(&self);
    fn join_workers(&self);
    fn drain_runtime_effects(&self);
    fn stop_shortcuts(&self);
    fn stop_keyboard_tap(&self);
    fn stop_drag_to_snap_tap(&self);
    fn stop_drag_to_move_tap(&self);
    fn restore_caps_lock(&self) -> bool;
    fn teardown_menu_bar(&self);
    fn cleanup_keep_awake(&self);
}

struct SystemShutdown<'a> {
    app: &'a AppHandle,
    state: &'a AppState,
}

impl ShutdownOps for SystemShutdown<'_> {
    fn cancel_workers(&self) {
        crate::keepawake::prepare_shutdown(self.app);
        crate::menubar::prepare_shutdown();
    }

    fn drain_config_mutations(&self) {
        self.state.drain_config_mutations_for_shutdown();
    }

    fn join_workers(&self) {
        self.state.lifecycle.join_workers();
    }

    fn drain_runtime_effects(&self) {
        self.state.lifecycle.drain_runtime_effects();
    }

    fn stop_shortcuts(&self) {
        if let Err(error) = crate::shortcuts::suspend_all(self.app) {
            tracing::warn!(%error, "global shortcuts could not be released during shutdown");
        }
    }

    fn stop_keyboard_tap(&self) {
        #[cfg(target_os = "macos")]
        crate::eventtap::teardown(self.app);
    }

    fn stop_drag_to_snap_tap(&self) {
        #[cfg(target_os = "macos")]
        crate::drag_to_snap::teardown(self.app);
    }

    fn stop_drag_to_move_tap(&self) {
        #[cfg(target_os = "macos")]
        crate::drag_to_move::teardown();
    }

    fn restore_caps_lock(&self) -> bool {
        crate::capsmap::reconcile(false).reconciled
    }

    fn teardown_menu_bar(&self) {
        crate::menubar::teardown(self.app);
    }

    fn cleanup_keep_awake(&self) {
        crate::keepawake::cleanup_blocking(self.app);
    }
}

fn run_cleanup(ops: &impl ShutdownOps) {
    // Cancel and drain every path that can restart a tap or temporarily own
    // synthetic input before taking the taps down. The final Caps operation
    // then always points toward native behavior, before either visible
    // menu-bar teardown or slow admin cleanup.
    ops.cancel_workers();
    ops.drain_config_mutations();
    ops.join_workers();
    ops.drain_runtime_effects();
    ops.stop_shortcuts();
    ops.stop_keyboard_tap();
    ops.stop_drag_to_snap_tap();
    ops.stop_drag_to_move_tap();
    if !ops.restore_caps_lock() {
        tracing::warn!(
            "caps-lock HID remap could not be restored on quit; will retry at next launch"
        );
    }
    ops.teardown_menu_bar();
    ops.cleanup_keep_awake();
}

fn finish_claimed_shutdown(lifecycle: &AppLifecycle, ops: &impl ShutdownOps) {
    run_cleanup(ops);
    lifecycle.finish_shutdown();
}

fn shutdown_with(lifecycle: &AppLifecycle, ops: &impl ShutdownOps) {
    match lifecycle.claim_shutdown() {
        ShutdownClaim::Leader => finish_claimed_shutdown(lifecycle, ops),
        ShutdownClaim::InProgress => lifecycle.wait_until_stopped(),
        ShutdownClaim::Complete => {}
    }
}

fn shutdown_then_with<R>(
    lifecycle: &AppLifecycle,
    ops: &impl ShutdownOps,
    continuation: impl FnOnce() -> R,
) -> R {
    shutdown_with(lifecycle, ops);
    continuation()
}

/// Complete terminal cleanup and only then run `continuation`. Keeping the
/// updater's relaunch behind this seam makes it impossible for that exit path
/// to bypass a newly added cleanup step.
pub fn shutdown_then<R>(app: &AppHandle, continuation: impl FnOnce() -> R) -> R {
    let state = app.state::<AppState>();
    let ops = SystemShutdown {
        app,
        state: state.inner(),
    };
    shutdown_then_with(&state.lifecycle, &ops, continuation)
}

/// Hold the first ordinary exit request, make the lifecycle terminal on the
/// main thread immediately, and finish cleanup off-main. A second request while
/// cleanup is active is held too; the request issued after `Stopped` is allowed
/// through. Tauri restart requests arrive only after the updater has already
/// called [`shutdown_then`], so their prevent API is intentionally unused.
pub fn handle_exit_requested(app: &AppHandle, code: Option<i32>, api: &ExitRequestApi) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    match state.lifecycle.claim_shutdown() {
        ShutdownClaim::Leader => {
            api.prevent_exit();
            let handle = app.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let state = handle.state::<AppState>();
                let ops = SystemShutdown {
                    app: &handle,
                    state: state.inner(),
                };
                finish_claimed_shutdown(&state.lifecycle, &ops);
                handle.exit(code.unwrap_or(0));
            });
        }
        ShutdownClaim::InProgress => api.prevent_exit(),
        ShutdownClaim::Complete => {}
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Barrier, mpsc};

    use tomari_core::{AppSettings, Database, Rect};
    use tomari_keyboard::ModifierEngine;
    use tomari_window::MockWindowManager;

    use super::*;

    fn test_state() -> AppState {
        AppState::new(
            Database::open_in_memory().unwrap(),
            ModifierEngine::new(Vec::new()),
            Box::new(MockWindowManager::new(Rect::new(0.0, 0.0, 100.0, 100.0))),
            AppSettings::default(),
            false,
        )
    }

    struct FakeOps {
        lifecycle: Arc<AppLifecycle>,
        config_state: Option<Arc<AppState>>,
        steps: Mutex<Vec<&'static str>>,
        step_tx: Option<mpsc::Sender<&'static str>>,
        drain_config_entered_tx: Option<mpsc::Sender<()>>,
        drain_effects_entered_tx: Option<mpsc::Sender<()>>,
        cleanup_barrier: Option<Arc<Barrier>>,
        caps_ok: bool,
    }

    impl FakeOps {
        fn new(lifecycle: Arc<AppLifecycle>) -> Self {
            Self {
                lifecycle,
                config_state: None,
                steps: Mutex::new(Vec::new()),
                step_tx: None,
                drain_config_entered_tx: None,
                drain_effects_entered_tx: None,
                cleanup_barrier: None,
                caps_ok: true,
            }
        }

        fn record(&self, step: &'static str) {
            self.steps.lock_safe().push(step);
            if let Some(tx) = &self.step_tx {
                tx.send(step).unwrap();
            }
        }

        fn steps(&self) -> Vec<&'static str> {
            self.steps.lock_safe().clone()
        }
    }

    impl ShutdownOps for FakeOps {
        fn cancel_workers(&self) {
            self.record("cancel-workers");
        }
        fn drain_config_mutations(&self) {
            if let Some(tx) = &self.drain_config_entered_tx {
                tx.send(()).unwrap();
            }
            if let Some(state) = &self.config_state {
                state.drain_config_mutations_for_shutdown();
            }
            self.record("drain-config");
        }
        fn join_workers(&self) {
            self.lifecycle.join_workers();
            self.record("join-workers");
        }
        fn drain_runtime_effects(&self) {
            if let Some(tx) = &self.drain_effects_entered_tx {
                tx.send(()).unwrap();
            }
            self.lifecycle.drain_runtime_effects();
            self.record("drain-effects");
            if let Some(barrier) = &self.cleanup_barrier {
                barrier.wait();
                barrier.wait();
            }
        }
        fn stop_shortcuts(&self) {
            self.record("stop-shortcuts");
        }
        fn stop_keyboard_tap(&self) {
            self.record("stop-keyboard");
        }
        fn stop_drag_to_snap_tap(&self) {
            self.record("stop-drag-to-snap");
        }
        fn stop_drag_to_move_tap(&self) {
            self.record("stop-drag-to-move");
        }
        fn restore_caps_lock(&self) -> bool {
            self.record("restore-caps");
            self.caps_ok
        }
        fn teardown_menu_bar(&self) {
            self.record("teardown-menu-bar");
        }
        fn cleanup_keep_awake(&self) {
            self.record("cleanup-keep-awake");
        }
    }

    #[test]
    fn shutdown_runs_once_in_the_required_order() {
        let lifecycle = Arc::new(AppLifecycle::default());
        let ops = FakeOps::new(Arc::clone(&lifecycle));

        shutdown_with(&lifecycle, &ops);
        shutdown_with(&lifecycle, &ops);

        assert_eq!(
            ops.steps(),
            [
                "cancel-workers",
                "drain-config",
                "join-workers",
                "drain-effects",
                "stop-shortcuts",
                "stop-keyboard",
                "stop-drag-to-snap",
                "stop-drag-to-move",
                "restore-caps",
                "teardown-menu-bar",
                "cleanup-keep-awake",
            ]
        );
        assert!(!lifecycle.is_running());
    }

    #[test]
    fn tracked_workers_are_cancelled_and_joined_before_taps_stop() {
        let lifecycle = Arc::new(AppLifecycle::default());
        let (exited_tx, exited_rx) = mpsc::channel();
        assert!(
            lifecycle
                .spawn_tracked("test-worker", move |lifecycle| {
                    assert!(lifecycle.wait_for_shutdown(Duration::from_secs(60)));
                    exited_tx.send(()).unwrap();
                })
                .unwrap()
        );
        let ops = FakeOps::new(Arc::clone(&lifecycle));

        shutdown_with(&lifecycle, &ops);

        exited_rx.try_recv().unwrap();
        let steps = ops.steps();
        assert!(
            steps.iter().position(|step| *step == "join-workers")
                < steps.iter().position(|step| *step == "stop-keyboard")
        );
    }

    #[test]
    fn finished_short_lived_workers_do_not_accumulate() {
        const WORKER_COUNT: usize = 64;

        let lifecycle = Arc::new(AppLifecycle::default());
        for index in 0..WORKER_COUNT {
            assert!(
                lifecycle
                    .spawn_tracked("short-lived-test-worker", |_| {})
                    .unwrap()
            );

            while !{
                let inner = lifecycle.inner.lock_safe();
                inner.workers.iter().all(JoinHandle::is_finished)
            } {
                std::thread::yield_now();
            }

            assert_eq!(
                lifecycle.inner.lock_safe().workers.len(),
                1,
                "finished worker handle accumulated after spawn {index}"
            );
        }

        lifecycle.stop_for_test();
        lifecycle.join_workers();
        assert!(lifecycle.inner.lock_safe().workers.is_empty());
    }

    #[test]
    fn a_runtime_effect_in_progress_is_drained_before_taps_stop() {
        let lifecycle = Arc::new(AppLifecycle::default());
        let effect = lifecycle.runtime_effect().unwrap();
        let (step_tx, step_rx) = mpsc::channel();
        let (entered_tx, entered_rx) = mpsc::channel();
        let mut fake = FakeOps::new(Arc::clone(&lifecycle));
        fake.step_tx = Some(step_tx);
        fake.drain_effects_entered_tx = Some(entered_tx);
        let ops = Arc::new(fake);
        let leader_lifecycle = Arc::clone(&lifecycle);
        let leader_ops = Arc::clone(&ops);
        let leader = std::thread::spawn(move || shutdown_with(&leader_lifecycle, &*leader_ops));

        assert_eq!(step_rx.recv().unwrap(), "cancel-workers");
        assert_eq!(step_rx.recv().unwrap(), "drain-config");
        assert_eq!(step_rx.recv().unwrap(), "join-workers");
        entered_rx.recv().unwrap();
        assert!(matches!(step_rx.try_recv(), Err(mpsc::TryRecvError::Empty)));

        drop(effect);
        leader.join().unwrap();
        assert_eq!(step_rx.try_recv().unwrap(), "drain-effects");
        assert!(
            ops.steps().iter().position(|step| *step == "drain-effects")
                < ops.steps().iter().position(|step| *step == "stop-keyboard")
        );
    }

    #[test]
    fn an_in_flight_config_mutation_is_drained_before_cleanup_continues() {
        let state = Arc::new(test_state());
        let config = state.lock_config_mutation().unwrap();
        let lifecycle = Arc::clone(&state.lifecycle);
        let (step_tx, step_rx) = mpsc::channel();
        let (entered_tx, entered_rx) = mpsc::channel();
        let mut fake = FakeOps::new(Arc::clone(&lifecycle));
        fake.config_state = Some(Arc::clone(&state));
        fake.step_tx = Some(step_tx);
        fake.drain_config_entered_tx = Some(entered_tx);
        let ops = Arc::new(fake);
        let leader_lifecycle = Arc::clone(&lifecycle);
        let leader_ops = Arc::clone(&ops);
        let leader = std::thread::spawn(move || shutdown_with(&leader_lifecycle, &*leader_ops));

        assert_eq!(step_rx.recv().unwrap(), "cancel-workers");
        entered_rx.recv().unwrap();
        assert!(matches!(step_rx.try_recv(), Err(mpsc::TryRecvError::Empty)));

        drop(config);
        leader.join().unwrap();
        assert_eq!(step_rx.try_recv().unwrap(), "drain-config");
        assert!(
            ops.steps().iter().position(|step| *step == "drain-config")
                < ops
                    .steps()
                    .iter()
                    .position(|step| *step == "stop-shortcuts")
        );
    }

    #[test]
    fn an_effect_queued_before_terminal_is_rejected_when_the_gate_opens() {
        let lifecycle = Arc::new(AppLifecycle::default());
        let held = lifecycle.runtime_effect().unwrap();
        let (attempting_tx, attempting_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let waiter_lifecycle = Arc::clone(&lifecycle);
        let waiter = std::thread::spawn(move || {
            attempting_tx.send(()).unwrap();
            result_tx
                .send(waiter_lifecycle.runtime_effect().is_some())
                .unwrap();
        });
        attempting_rx.recv().unwrap();

        assert_eq!(lifecycle.claim_shutdown(), ShutdownClaim::Leader);
        drop(held);

        assert!(!result_rx.recv().unwrap());
        waiter.join().unwrap();
        lifecycle.finish_shutdown();
    }

    #[test]
    fn a_concurrent_shutdown_waits_for_the_leader() {
        let lifecycle = Arc::new(AppLifecycle::default());
        let barrier = Arc::new(Barrier::new(2));
        let mut fake = FakeOps::new(Arc::clone(&lifecycle));
        fake.cleanup_barrier = Some(Arc::clone(&barrier));
        let ops = Arc::new(fake);
        let leader_lifecycle = Arc::clone(&lifecycle);
        let leader_ops = Arc::clone(&ops);
        let leader = std::thread::spawn(move || shutdown_with(&leader_lifecycle, &*leader_ops));
        barrier.wait();

        let (started_tx, started_rx) = mpsc::channel();
        let (returned_tx, returned_rx) = mpsc::channel();
        let follower_lifecycle = Arc::clone(&lifecycle);
        let follower_ops = Arc::clone(&ops);
        let follower = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            shutdown_with(&follower_lifecycle, &*follower_ops);
            returned_tx.send(()).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(returned_rx.recv_timeout(Duration::from_millis(50)).is_err());

        barrier.wait();
        leader.join().unwrap();
        follower.join().unwrap();
        returned_rx.try_recv().unwrap();
        assert_eq!(
            ops.steps()
                .iter()
                .filter(|step| **step == "stop-keyboard")
                .count(),
            1
        );
    }

    #[test]
    fn terminal_rejects_new_workers_and_runtime_effects() {
        let lifecycle = Arc::new(AppLifecycle::default());
        assert_eq!(lifecycle.claim_shutdown(), ShutdownClaim::Leader);

        assert!(
            !lifecycle
                .spawn_tracked("too-late", |_| panic!("must not spawn"))
                .unwrap()
        );
        assert!(lifecycle.runtime_effect().is_none());
        lifecycle.finish_shutdown();
    }

    #[test]
    fn stop_for_test_is_terminal_before_it_drains_runtime_effects() {
        let lifecycle = Arc::new(AppLifecycle::default());
        let effect = lifecycle.runtime_effect().unwrap();
        let (returned_tx, returned_rx) = mpsc::channel();
        let stopper_lifecycle = Arc::clone(&lifecycle);
        let stopper = std::thread::spawn(move || {
            stopper_lifecycle.stop_for_test();
            returned_tx.send(()).unwrap();
        });

        while lifecycle.is_running() {
            std::thread::yield_now();
        }
        assert!(
            !lifecycle
                .spawn_tracked("too-late", |_| panic!("must not spawn"))
                .unwrap()
        );
        assert!(matches!(
            returned_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));

        drop(effect);
        stopper.join().unwrap();
        returned_rx.try_recv().unwrap();
        assert!(!lifecycle.is_running());
    }

    #[test]
    fn terminal_rejects_config_mutations_before_they_touch_persistence() {
        let state = test_state();
        assert!(state.lock_config_mutation().is_some());

        assert_eq!(state.lifecycle.claim_shutdown(), ShutdownClaim::Leader);

        assert!(state.lock_config_mutation().is_none());
        state.lifecycle.finish_shutdown();
    }

    #[test]
    fn a_config_mutation_waiting_for_the_lock_is_rejected_after_terminal() {
        let state = Arc::new(test_state());
        let held = state.lock_config_mutation().unwrap();
        let (attempting_tx, attempting_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let waiter_state = Arc::clone(&state);
        let waiter = std::thread::spawn(move || {
            attempting_tx.send(()).unwrap();
            result_tx
                .send(waiter_state.lock_config_mutation().is_some())
                .unwrap();
        });
        attempting_rx.recv().unwrap();

        assert_eq!(state.lifecycle.claim_shutdown(), ShutdownClaim::Leader);
        drop(held);

        assert!(!result_rx.recv().unwrap());
        waiter.join().unwrap();
        state.lifecycle.finish_shutdown();
    }

    #[test]
    fn relaunch_runs_after_cleanup_and_does_not_reopen_the_lifecycle() {
        let lifecycle = Arc::new(AppLifecycle::default());
        let ops = FakeOps::new(Arc::clone(&lifecycle));

        let result: Result<(), &str> = shutdown_then_with(&lifecycle, &ops, || {
            ops.record("relaunch");
            Err("spawn failed")
        });

        assert_eq!(result, Err("spawn failed"));
        assert_eq!(ops.steps().last(), Some(&"relaunch"));
        assert!(!lifecycle.is_running());
        assert!(lifecycle.runtime_effect().is_none());
    }

    #[test]
    fn a_caps_restore_failure_does_not_skip_later_cleanup() {
        let lifecycle = Arc::new(AppLifecycle::default());
        let mut ops = FakeOps::new(Arc::clone(&lifecycle));
        ops.caps_ok = false;

        shutdown_with(&lifecycle, &ops);

        assert_eq!(
            &ops.steps()[8..],
            ["restore-caps", "teardown-menu-bar", "cleanup-keep-awake"]
        );
    }
}
