//! Keep [`AppState`]'s display-geometry cache current for drag-to-snap.
//!
//! Edge detection needs each display's full frame and work area, which only the
//! main thread can read (`WindowManager::screens_cg`). Rather than have the
//! gesture tap thread block on a main-thread round-trip, the geometry is cached
//! in [`AppState`] and refreshed here: once at startup, then on every display
//! reconfiguration (resolution, arrangement, a display plugged or unplugged,
//! the Dock changing the visible frame). `NSApplication` posts
//! `NSApplicationDidChangeScreenParametersNotification` on the main thread for
//! all of these, so observing it keeps the cache correct without the tap ever
//! waiting on AppKit.

use std::ptr::NonNull;

use block2::RcBlock;
use objc2_app_kit::NSApplicationDidChangeScreenParametersNotification;
use objc2_foundation::{NSNotification, NSNotificationCenter};
use tauri::{AppHandle, Manager};

use crate::state::AppState;

/// Prime the display-geometry cache and observe display changes for the app's
/// lifetime. Must be called on the main thread (it reads AppKit immediately).
pub fn install(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        state.refresh_screen_geometry();
    }

    let center = NSNotificationCenter::defaultCenter();
    let handle = app.clone();
    let block = RcBlock::new(move |_: NonNull<NSNotification>| {
        if let Some(state) = handle.try_state::<AppState>() {
            state.refresh_screen_geometry();
        }
    });
    // The token owns the observation; it is intentionally leaked because the
    // observation must last until the process exits (mirrors `wake.rs`).
    let token = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSApplicationDidChangeScreenParametersNotification),
            None,
            None,
            &block,
        )
    };
    std::mem::forget(token);
}
