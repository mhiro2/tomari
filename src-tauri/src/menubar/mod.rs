//! Menu bar tidying — gather the status items you rarely look at behind a
//! divider, and push them off-screen until you ask for them.
//!
//! The mechanism and its one hard limit live in [`status`]: Tomari can only
//! control its own status items, so *which* icons end up hidden is whatever the
//! user has ⌘-dragged to the left of the divider. There is no API to do that
//! for them.
//!
//! Like keep-awake, the expanded/collapsed state is runtime-only and always
//! starts collapsed — with one exception: switching the feature on starts
//! expanded, because a user who has not arranged anything yet would otherwise
//! see nothing happen at all and conclude the switch is broken.

mod state;

#[cfg(target_os = "macos")]
mod status;

#[cfg(not(target_os = "macos"))]
mod status {
    use tauri::AppHandle;

    pub fn apply(_app: &AppHandle, _enabled: bool, _collapsed: bool) {}
    pub fn teardown(_app: &AppHandle) {}
}

use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tomari_core::AppSettings;

use crate::locks::MutexExt;
use crate::state::AppState;

pub use state::MenuBarState;

/// Emitted whenever the menu bar state changes, so the panel toggle and the
/// tray checkmark stay in step regardless of which surface initiated it.
const CHANGED_EVENT: &str = "tomari:menu-bar-changed";

/// What the panel and the tray render.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuBarStatus {
    /// The feature's master switch.
    pub enabled: bool,
    /// Whether the tidied items are currently pushed off-screen.
    pub collapsed: bool,
}

pub fn status(state: &AppState) -> MenuBarStatus {
    MenuBarStatus {
        enabled: state.settings.lock_safe().menu_bar_tidy_enabled,
        collapsed: state.menu_bar.lock_safe().is_collapsed(),
    }
}

/// Bring the status items up (or leave them down) to match the stored settings.
/// Called once from `setup`.
pub fn init(app: &AppHandle) {
    publish(app);
}

/// Take the status items down on the way out. A status item belongs to the
/// process and disappears with it even on a crash, so this is not a safety net
/// — it just means the menu bar is tidy the moment Tomari is asked to quit
/// rather than whenever the process actually goes away.
pub fn teardown(app: &AppHandle) {
    status::teardown(app);
}

/// Flip between expanded and collapsed. Reached from the controller item's
/// click, the tray, a hotkey and the panel.
pub fn toggle(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    if !state.settings.lock_safe().menu_bar_tidy_enabled {
        return;
    }
    let now = state.now_ms();
    let pending = {
        let mut menu_bar = state.menu_bar.lock_safe();
        menu_bar.toggle(now);
        menu_bar.pending_collapse()
    };
    publish(app);
    arm_auto_collapse(app, pending);
}

/// Expand or collapse explicitly, for the panel's switch.
pub fn set_collapsed(app: &AppHandle, collapsed: bool) -> MenuBarStatus {
    let Some(state) = app.try_state::<AppState>() else {
        return MenuBarStatus {
            enabled: false,
            collapsed: true,
        };
    };
    if !state.settings.lock_safe().menu_bar_tidy_enabled {
        return status(state.inner());
    }
    let now = state.now_ms();
    let pending = {
        let mut menu_bar = state.menu_bar.lock_safe();
        menu_bar.set_collapsed(collapsed, now);
        menu_bar.pending_collapse()
    };
    publish(app);
    arm_auto_collapse(app, pending);
    status(state.inner())
}

/// Reconcile the runtime state with a settings save. `state.settings` must
/// already hold `next` — [`publish`] reads the live settings, not this argument.
pub fn apply_settings(app: &AppHandle, previous: &AppSettings, next: &AppSettings) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let now = state.now_ms();
    let pending = {
        let mut menu_bar = state.menu_bar.lock_safe();
        menu_bar.set_auto_collapse_secs(next.menu_bar_auto_collapse_secs, now);
        if !previous.menu_bar_tidy_enabled && next.menu_bar_tidy_enabled {
            // See the module doc: switching it on has to show something.
            menu_bar.expand(now);
        } else if !next.menu_bar_tidy_enabled {
            // Switching it off drops the items entirely, so leave the state
            // collapsed — the resting position a later switch-on starts from.
            menu_bar.collapse();
        }
        menu_bar.pending_collapse()
    };
    publish(app);
    arm_auto_collapse(app, pending);
}

/// Push the current state out to the status items, the panel and the tray.
fn publish(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let current = status(state.inner());
    status::apply(app, current.enabled, current.collapsed);
    let _ = app.emit(CHANGED_EVENT, current);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || crate::tray::refresh(&handle));
}

/// Arm a one-shot timer to collapse the expand identified by `generation`.
/// `None` means nothing to do — collapsed, or the auto-collapse timer is off,
/// which is the default.
fn arm_auto_collapse(app: &AppHandle, pending: Option<(u64, u64)>) {
    let Some((at_ms, generation)) = pending else {
        return;
    };
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let delay = Duration::from_millis(at_ms.saturating_sub(state.now_ms()));
    let app = app.clone();
    // A plain sleeping thread rather than a repeating tick: expands are rare
    // and short-lived, and a superseded timer costs nothing — it wakes, finds
    // its generation stale and exits.
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        let fired = state.menu_bar.lock_safe().auto_collapse_elapsed(generation);
        if fired {
            publish(&app);
        }
    });
}
