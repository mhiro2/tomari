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

use block2::RcBlock;
use objc2_app_kit::{
    NSWorkspace, NSWorkspaceDidWakeNotification, NSWorkspaceSessionDidBecomeActiveNotification,
};
use objc2_foundation::NSNotification;
use tauri::{AppHandle, Manager};

use crate::locks::MutexExt;
use crate::state::AppState;

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
    if let Some(state) = app.try_state::<AppState>() {
        state.engine.lock_safe().reset();
    }
    // Restarting a tap joins its previous thread and (for the keyboard tap)
    // can shell out to `hidutil` while reconciling the Caps Lock remap; none
    // of that touches AppKit UI, so it needs no main-thread hop — only run
    // off this notification callback's own thread so a slow join/`hidutil`
    // never delays it (and, transitively, whatever queue the notification
    // center delivers on). `AppState::config_mutation` is not held here:
    // these restarts do not touch the database or the shortcut map, only the
    // tap-local caps/hyper tracking, so they cannot race a config save/delete
    // in a way that matters.
    let handle = app.clone();
    let _ = std::thread::Builder::new()
        .name("tomari-wake-reset".into())
        .spawn(move || {
            crate::eventtap::restart(&handle);
            crate::drag_to_snap::restart(&handle);
            crate::drag_to_move::restart(&handle);
        });
}
