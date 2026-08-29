//! Reset key-tracking state when the system wakes from sleep or the user
//! session becomes active again (fast user switching).
//!
//! A modifier held while the machine goes to sleep never delivers its release
//! to the event tap, so the engines and the tap-local state would keep
//! believing a key is down — a remap applied to nothing, a hyper combo
//! stamped onto every keystroke. `NSWorkspace` posts wake and session
//! notifications on its own notification center; observing them lets the
//! app drop every transient assumption about what is held.

use std::ptr::NonNull;
use std::sync::Arc;

use block2::RcBlock;
use objc2_app_kit::{
    NSWorkspace, NSWorkspaceDidWakeNotification, NSWorkspaceSessionDidBecomeActiveNotification,
};
use objc2_foundation::NSNotification;
use tauri::{AppHandle, Manager};

use crate::state::AppState;

/// Run the three wake repairs in order, rechecking the terminal lifecycle
/// between them. A quit that starts while one restart is in progress must not
/// let the remaining taps come back behind shutdown cleanup.
fn apply_reset_if_running<K, S, M>(
    lifecycle: &crate::lifecycle::AppLifecycle,
    restart_keyboard: K,
    restart_drag_to_snap: S,
    restart_drag_to_move: M,
) where
    K: FnOnce(),
    S: FnOnce(),
    M: FnOnce(),
{
    if !lifecycle.is_running() {
        return;
    }
    restart_keyboard();
    if !lifecycle.is_running() {
        return;
    }
    restart_drag_to_snap();
    if !lifecycle.is_running() {
        return;
    }
    restart_drag_to_move();
}

/// Observe wake / session-active notifications for the app's lifetime.
pub fn install(app: &AppHandle) {
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    let names = unsafe {
        [
            NSWorkspaceDidWakeNotification,
            NSWorkspaceSessionDidBecomeActiveNotification,
        ]
    };
    for name in names {
        let handle = app.clone();
        let block = RcBlock::new(move |_: NonNull<NSNotification>| reset(&handle));
        // The returned token owns the observation; it is intentionally leaked
        // because the observation must last until the process exits.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &block)
        };
        std::mem::forget(token);
    }
}

/// Drop all transient key-tracking state and restart the taps, so nothing
/// carries a "key is held" belief across a sleep or session switch.
fn reset(app: &AppHandle) {
    tracing::info!("woke from sleep or session became active — resetting input state");
    // The engine's hold state is reset inside `eventtap::restart`, *after* it
    // has released whatever the old tap still owed downstream for held remapped
    // keys; resetting here first would make that release find nothing to do.
    //
    // Restarting a tap joins its previous thread and (for the keyboard tap)
    // can shell out to `hidutil` while reconciling the Caps Lock remap; none
    // of that touches AppKit UI, so it needs no main-thread hop — only run
    // off this notification callback's own thread so a slow join/`hidutil`
    // never delays it (and, transitively, whatever queue the notification
    // center delivers on). `AppState::config_mutation` is not held here:
    // these restarts do not touch the database or the shortcut map, only the
    // tap-local caps/hyper tracking. The lifecycle owns this worker and each
    // restart has its own terminal effect gate, so quit can join a reset that
    // already started without letting any later tap come back.
    let lifecycle = {
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        Arc::clone(&state.lifecycle)
    };
    let keyboard_handle = app.clone();
    let snap_handle = app.clone();
    let move_handle = app.clone();
    match lifecycle.spawn_tracked("tomari-wake-reset", move |lifecycle| {
        apply_reset_if_running(
            &lifecycle,
            || crate::eventtap::restart(&keyboard_handle),
            || crate::drag_to_snap::restart(&snap_handle),
            || crate::drag_to_move::restart(&move_handle),
        );
    }) {
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "could not spawn the wake reset worker"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, mpsc};

    use super::*;

    #[test]
    fn terminal_quit_stops_a_wake_reset_between_restart_steps() {
        let lifecycle = Arc::new(crate::lifecycle::AppLifecycle::default());
        let (ran_tx, ran_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let reset_lifecycle = Arc::clone(&lifecycle);
        let keyboard_tx = ran_tx.clone();
        let snap_tx = ran_tx.clone();
        let reset = std::thread::spawn(move || {
            apply_reset_if_running(
                &reset_lifecycle,
                || {
                    keyboard_tx.send("keyboard").unwrap();
                    release_rx.recv().unwrap();
                },
                || snap_tx.send("drag-to-snap").unwrap(),
                || ran_tx.send("drag-to-move").unwrap(),
            );
        });

        assert_eq!(ran_rx.recv().unwrap(), "keyboard");
        lifecycle.stop_for_test();
        release_tx.send(()).unwrap();

        reset.join().unwrap();
        assert!(ran_rx.try_recv().is_err());
    }
}
