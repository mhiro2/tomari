//! Menu bar tidying — gather the status items you rarely look at behind a
//! divider, and push them off-screen until you ask for them.
//!
//! The mechanism and its one hard limit live in [`status`]: Tomari can only
//! control its own status items. Moving another application's item therefore
//! uses the same ⌘-drag gesture a person would perform, synthesized from the
//! Accessibility geometry returned by [`inventory`].
//!
//! Like keep-awake, the expanded/collapsed state is runtime-only and always
//! starts collapsed — with one exception: switching the feature on starts
//! expanded, because a user who has not arranged anything yet would otherwise
//! see nothing happen at all and conclude the switch is broken.

mod state;

#[cfg(target_os = "macos")]
mod inventory;

#[cfg(target_os = "macos")]
mod movement;

#[cfg(target_os = "macos")]
mod status;

#[cfg(not(target_os = "macos"))]
mod status {
    use tauri::AppHandle;

    pub fn apply(_app: &AppHandle, _enabled: bool, _collapsed: bool) {}
    pub fn teardown(_app: &AppHandle) {}
}

#[cfg(not(target_os = "macos"))]
mod inventory {
    use super::MenuBarInventory;

    pub fn unsupported() -> MenuBarInventory {
        MenuBarInventory {
            supported: false,
            permission_granted: false,
            divider_available: false,
            items: Vec::new(),
        }
    }
}

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, TryLockError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
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

/// Which side of Tomari's divider a menu bar item currently occupies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MenuBarItemZone {
    Hidden,
    Visible,
}

/// A best-effort Accessibility snapshot of one menu bar item.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuBarItem {
    /// Ephemeral render key. The Accessibility API exposes no durable item id.
    pub id: String,
    /// The most useful user-facing label available for this item.
    pub name: String,
    /// Owning application when the item label is more specific (for example,
    /// Wi-Fi owned by Control Center).
    pub owner_name: Option<String>,
    pub bundle_id: Option<String>,
    pub zone: MenuBarItemZone,
    /// Physical order used inside the backend; not part of the command payload.
    #[serde(skip)]
    pub position: f64,
    /// Vertical center in AX/Core Graphics' global top-left coordinate space.
    #[serde(skip)]
    pub center_y: f64,
    /// Physical size used to choose a drop point clear of the divider.
    #[serde(skip)]
    pub width: f64,
    /// Owning process for this exact scan. A restarted process invalidates the
    /// snapshot even if its bundle identifier and labels are unchanged.
    #[serde(skip)]
    pub owner_pid: i32,
    /// The strongest Accessibility identity available. This is still scoped
    /// to one process lifetime and may be absent or duplicated.
    #[serde(skip)]
    pub ax_identifier: Option<String>,
}

/// Result of scanning the currently active menu bar.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuBarInventory {
    pub supported: bool,
    pub permission_granted: bool,
    pub divider_available: bool,
    pub items: Vec<MenuBarItem>,
}

/// Whether a settings-requested menu bar move took effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MenuBarMoveOutcome {
    Moved,
    AlreadyInZone,
    StaleItem,
    NotMovable,
}

/// A move result always carries a newly scanned inventory so the settings UI
/// never has to guess what macOS ultimately accepted.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuBarMoveResult {
    pub outcome: MenuBarMoveOutcome,
    pub inventory: MenuBarInventory,
}

#[derive(Default)]
struct InventorySession {
    generation: u64,
    latest: Vec<MenuBarItem>,
}

/// Accessibility scans expand Tomari's divider and menu item moves temporarily
/// take over the pointer. Keeping both under one gate prevents refreshes from
/// changing the snapshot or divider geometry in the middle of a move.
static INVENTORY_SESSION: Mutex<InventorySession> = Mutex::new(InventorySession {
    generation: 0,
    latest: Vec::new(),
});

/// A status publication that collides with a scan must not block AppKit's main
/// thread on `INVENTORY_SESSION`: the scan may itself be waiting for a main-
/// thread geometry read. One waiter reconciles the latest logical state after
/// the operation releases the gate; later publications are coalesced into it.
static STATUS_RECONCILE_PENDING: AtomicBool = AtomicBool::new(false);

pub fn status(state: &AppState) -> MenuBarStatus {
    MenuBarStatus {
        enabled: state.settings.lock_safe().menu_bar_tidy_enabled,
        collapsed: state.menu_bar.lock_safe().is_collapsed(),
    }
}

/// Inspect the real menu bar layout. The divider is expanded only for the scan
/// and then restored to the live state; the physical arrangement remembered by
/// macOS remains the source of truth, including changes made with ⌘-drag while
/// the settings window is open.
pub fn inventory(app: &AppHandle, state: &AppState) -> MenuBarInventory {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, state);
        return inventory::unsupported();
    }

    #[cfg(target_os = "macos")]
    {
        let mut session = INVENTORY_SESSION.lock_safe();
        inventory_locked(app, state, &mut session)
    }
}

/// Move the item represented by the latest inventory snapshot to the requested
/// side of Tomari's divider. The opaque id embeds the snapshot generation;
/// refreshing the list invalidates prior ids instead of risking a move of a
/// different item that later reused the same Accessibility attributes.
pub fn move_item(
    app: &AppHandle,
    state: &AppState,
    item_id: &str,
    target_zone: MenuBarItemZone,
) -> MenuBarMoveResult {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, state, item_id, target_zone);
        return MenuBarMoveResult {
            outcome: MenuBarMoveOutcome::NotMovable,
            inventory: inventory::unsupported(),
        };
    }

    #[cfg(target_os = "macos")]
    {
        let mut session = INVENTORY_SESSION.lock_safe();
        move_item_locked(app, state, item_id, target_zone, &mut session)
    }
}

#[cfg(target_os = "macos")]
struct DividerRestore<'a> {
    app: &'a AppHandle,
    state: &'a AppState,
}

#[cfg(target_os = "macos")]
impl Drop for DividerRestore<'_> {
    fn drop(&mut self) {
        status::finish_scan(self.app, self.state);
    }
}

#[cfg(target_os = "macos")]
fn inventory_locked(
    app: &AppHandle,
    state: &AppState,
    session: &mut InventorySession,
) -> MenuBarInventory {
    if let Some(inventory) = unavailable_inventory(state, session) {
        return inventory;
    }

    let _restore = DividerRestore { app, state };
    let Some(context) = status::scan_context(app) else {
        return publish_inventory(session, true, true, false, Vec::new());
    };
    wait_for_menu_bar_layout();
    let items = inventory::scan(context);
    publish_inventory(session, true, true, true, items)
}

#[cfg(target_os = "macos")]
fn move_item_locked(
    app: &AppHandle,
    state: &AppState,
    item_id: &str,
    target_zone: MenuBarItemZone,
    session: &mut InventorySession,
) -> MenuBarMoveResult {
    let expected = unique_item(&session.latest, |item| item.id == item_id).cloned();
    let requested_latest_item = expected.is_some();

    if let Some(inventory) = unavailable_inventory(state, session) {
        return MenuBarMoveResult {
            outcome: if requested_latest_item {
                MenuBarMoveOutcome::NotMovable
            } else {
                MenuBarMoveOutcome::StaleItem
            },
            inventory,
        };
    }

    let _restore = DividerRestore { app, state };
    let Some(context) = status::scan_context(app) else {
        return MenuBarMoveResult {
            outcome: MenuBarMoveOutcome::NotMovable,
            inventory: publish_inventory(session, true, true, false, Vec::new()),
        };
    };
    wait_for_menu_bar_layout();
    let current_items = inventory::scan(context);

    let Some(expected) = expected else {
        return MenuBarMoveResult {
            outcome: MenuBarMoveOutcome::StaleItem,
            inventory: publish_inventory(session, true, true, true, current_items),
        };
    };
    let matching: Vec<&MenuBarItem> = current_items
        .iter()
        .filter(|candidate| tracks_snapshot_item(&expected, candidate))
        .collect();
    let current = match matching.as_slice() {
        [] => {
            return MenuBarMoveResult {
                outcome: MenuBarMoveOutcome::StaleItem,
                inventory: publish_inventory(session, true, true, true, current_items),
            };
        }
        [item] => (*item).clone(),
        _ => {
            return MenuBarMoveResult {
                outcome: MenuBarMoveOutcome::NotMovable,
                inventory: publish_inventory(session, true, true, true, current_items),
            };
        }
    };
    drop(matching);
    if current.zone == target_zone {
        return MenuBarMoveResult {
            outcome: MenuBarMoveOutcome::AlreadyInZone,
            inventory: publish_inventory(session, true, true, true, current_items),
        };
    }

    let Some((drag_target, drag_context)) = resolve_drag_target(app, state, &current) else {
        tracing::warn!(item = %current.name, "menu bar changed before item move started");
        let outcome = if movement_available(state) {
            MenuBarMoveOutcome::StaleItem
        } else {
            MenuBarMoveOutcome::NotMovable
        };
        return MenuBarMoveResult {
            outcome,
            inventory: refresh_inventory_locked(app, state, session),
        };
    };
    let current = drag_target.item().clone();
    if current.zone == target_zone {
        return MenuBarMoveResult {
            outcome: MenuBarMoveOutcome::AlreadyInZone,
            inventory: refresh_inventory_locked(app, state, session),
        };
    }

    let cursor = match movement::command_drag(&current, drag_context, target_zone, || {
        drag_target_is_current(state, &drag_target, &current)
            && status::scan_context(app)
                .is_some_and(|latest| same_scan_context(drag_context, latest))
            && movement_available(state)
    }) {
        Ok(cursor) => cursor,
        Err(movement::CommandDragError::TargetChanged) => {
            tracing::warn!(item = %current.name, "menu bar changed before item move started");
            let outcome = if movement_available(state) {
                MenuBarMoveOutcome::StaleItem
            } else {
                MenuBarMoveOutcome::NotMovable
            };
            return MenuBarMoveResult {
                outcome,
                inventory: refresh_inventory_locked(app, state, session),
            };
        }
        Err(error) => {
            tracing::warn!(item = %current.name, %error, "menu bar item move was not started");
            return MenuBarMoveResult {
                outcome: MenuBarMoveOutcome::NotMovable,
                inventory: refresh_inventory_locked(app, state, session),
            };
        }
    };

    // The gesture itself includes a Window Server settling delay. Restore the
    // person's pointer before the slower all-process AX verification scan.
    drop(cursor);

    if let Some(inventory) = unavailable_inventory(state, session) {
        return MenuBarMoveResult {
            outcome: MenuBarMoveOutcome::NotMovable,
            inventory,
        };
    }

    // Poll the exact retained AX element first so the full inventory is not
    // captured before a successful drag has settled. This is both cheaper and
    // stricter than rediscovering an identifierless item by process and label.
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(80));
        }
        if drag_target_is_in_zone(app, state, &drag_target, target_zone) {
            break;
        }
    }

    // One all-process scan supplies the inventory returned to the UI. Re-read
    // the retained element after it so a target scanned early cannot move again
    // while the slower remainder of this snapshot is collected.
    let (divider_available, final_items) =
        scan_current_layout(app).map_or((false, Vec::new()), |items| (true, items));
    let full_scan_moved = unique_item(&final_items, |candidate| {
        tracks_same_item(&current, candidate)
    })
    .is_some_and(|candidate| candidate.zone == target_zone);
    let moved = full_scan_moved && drag_target_is_in_zone(app, state, &drag_target, target_zone);
    if let Some(inventory) = unavailable_inventory(state, session) {
        return MenuBarMoveResult {
            outcome: MenuBarMoveOutcome::NotMovable,
            inventory,
        };
    }
    if moved {
        tracing::info!(item = %current.name, ?target_zone, "menu bar item moved");
    } else {
        tracing::warn!(item = %current.name, ?target_zone, "macOS did not accept menu bar item move");
    }
    MenuBarMoveResult {
        outcome: if moved {
            MenuBarMoveOutcome::Moved
        } else {
            MenuBarMoveOutcome::NotMovable
        },
        inventory: publish_inventory(session, true, true, divider_available, final_items),
    }
}

#[cfg(target_os = "macos")]
fn unavailable_inventory(
    state: &AppState,
    session: &mut InventorySession,
) -> Option<MenuBarInventory> {
    if !state.windows.permission_granted() {
        return Some(publish_inventory(session, true, false, false, Vec::new()));
    }
    if !state.settings.lock_safe().menu_bar_tidy_enabled {
        return Some(publish_inventory(session, true, true, false, Vec::new()));
    }
    None
}

#[cfg(target_os = "macos")]
fn refresh_inventory_locked(
    app: &AppHandle,
    state: &AppState,
    session: &mut InventorySession,
) -> MenuBarInventory {
    unavailable_inventory(state, session).unwrap_or_else(|| {
        let (divider_available, items) =
            scan_current_layout(app).map_or((false, Vec::new()), |items| (true, items));
        publish_inventory(session, true, true, divider_available, items)
    })
}

#[cfg(target_os = "macos")]
fn wait_for_menu_bar_layout() {
    // AppKit applies status-item frame changes on the main run loop. One frame
    // keeps the AX scan from observing the divider's previous 10,000pt width.
    std::thread::sleep(Duration::from_millis(40));
}

#[cfg(target_os = "macos")]
fn scan_current_layout(app: &AppHandle) -> Option<Vec<MenuBarItem>> {
    let context = status::scan_context(app)?;
    wait_for_menu_bar_layout();
    let items = inventory::scan(context);
    // A full scan crosses process boundaries and is not atomic. If unrelated
    // menu extras moved Tomari's divider while it ran, none of the recorded
    // zone classifications are authoritative enough to verify a move.
    status::scan_context(app)
        .is_some_and(|after_scan| same_scan_context(context, after_scan))
        .then_some(items)
}

#[cfg(target_os = "macos")]
fn resolve_drag_target(
    app: &AppHandle,
    state: &AppState,
    expected: &MenuBarItem,
) -> Option<(inventory::DragTarget, status::ScanContext)> {
    if !movement_available(state) {
        return None;
    }
    let context = status::scan_context(app)?;
    wait_for_menu_bar_layout();
    let targets = inventory::resolve_owner_at_point(
        context,
        expected.owner_pid,
        expected.position,
        expected.center_y,
    );
    let mut matching = targets.into_iter().filter(|candidate| {
        tracks_same_item(expected, candidate.item())
            && same_item_geometry(expected, candidate.item())
    });
    let target = matching.next()?;
    if matching.next().is_some()
        || !status::scan_context(app)
            .is_some_and(|after_scan| same_scan_context(context, after_scan))
        || !movement_available(state)
    {
        return None;
    }
    Some((target, context))
}

#[cfg(target_os = "macos")]
fn drag_target_is_current(
    state: &AppState,
    target: &inventory::DragTarget,
    expected: &MenuBarItem,
) -> bool {
    if !movement_available(state) {
        return false;
    }
    let frame_matches = target.current_geometry().is_some_and(|(x, y, width)| {
        close_geometry(expected.position, x)
            && close_geometry(expected.center_y, y)
            && close_geometry(expected.width, width)
    });
    frame_matches && movement_available(state)
}

#[cfg(target_os = "macos")]
fn drag_target_is_in_zone(
    app: &AppHandle,
    state: &AppState,
    target: &inventory::DragTarget,
    expected_zone: MenuBarItemZone,
) -> bool {
    if !movement_available(state) {
        return false;
    }
    let Some(context) = status::scan_context(app) else {
        return false;
    };
    wait_for_menu_bar_layout();
    let geometry_zone = target
        .current_geometry()
        .and_then(|(x, y, _width)| zone_at_point(context, x, y));
    geometry_zone == Some(expected_zone)
        && status::scan_context(app)
            .is_some_and(|after_scan| same_scan_context(context, after_scan))
        && movement_available(state)
}

#[cfg(target_os = "macos")]
fn zone_at_point(context: status::ScanContext, x: f64, y: f64) -> Option<MenuBarItemZone> {
    (x.is_finite()
        && y.is_finite()
        && x >= context.screen_left
        && x <= context.screen_right
        && y >= context.menu_top - 4.0
        && y <= context.menu_bottom + 4.0)
        .then_some(if x < context.divider_x {
            MenuBarItemZone::Hidden
        } else {
            MenuBarItemZone::Visible
        })
}

#[cfg(target_os = "macos")]
fn movement_available(state: &AppState) -> bool {
    state.windows.permission_granted() && state.settings.lock_safe().menu_bar_tidy_enabled
}

#[cfg(target_os = "macos")]
fn publish_inventory(
    session: &mut InventorySession,
    supported: bool,
    permission_granted: bool,
    divider_available: bool,
    mut items: Vec<MenuBarItem>,
) -> MenuBarInventory {
    session.generation = session.generation.wrapping_add(1).max(1);
    for (index, item) in items.iter_mut().enumerate() {
        item.id = format!("{}:{index}", session.generation);
    }
    session.latest.clone_from(&items);
    MenuBarInventory {
        supported,
        permission_granted,
        divider_available,
        items,
    }
}

#[cfg(target_os = "macos")]
fn unique_item(
    items: &[MenuBarItem],
    predicate: impl Fn(&MenuBarItem) -> bool,
) -> Option<&MenuBarItem> {
    let mut matching = items.iter().filter(|item| predicate(item));
    let item = matching.next()?;
    matching.next().is_none().then_some(item)
}

#[cfg(target_os = "macos")]
fn tracks_same_item(expected: &MenuBarItem, candidate: &MenuBarItem) -> bool {
    if expected.owner_pid != candidate.owner_pid || expected.bundle_id != candidate.bundle_id {
        return false;
    }
    match expected.ax_identifier.as_deref() {
        Some(identifier) => candidate.ax_identifier.as_deref() == Some(identifier),
        None => {
            candidate.ax_identifier.is_none()
                && expected.name == candidate.name
                && expected.owner_name == candidate.owner_name
        }
    }
}

#[cfg(target_os = "macos")]
fn tracks_snapshot_item(expected: &MenuBarItem, candidate: &MenuBarItem) -> bool {
    tracks_same_item(expected, candidate)
        && (expected.ax_identifier.is_some() || same_item_geometry(expected, candidate))
}

#[cfg(target_os = "macos")]
const GEOMETRY_TOLERANCE: f64 = 0.5;

#[cfg(target_os = "macos")]
fn same_item_geometry(left: &MenuBarItem, right: &MenuBarItem) -> bool {
    close_geometry(left.position, right.position)
        && close_geometry(left.center_y, right.center_y)
        && close_geometry(left.width, right.width)
}

#[cfg(target_os = "macos")]
fn same_scan_context(left: status::ScanContext, right: status::ScanContext) -> bool {
    close_geometry(left.divider_x, right.divider_x)
        && close_geometry(left.divider_left, right.divider_left)
        && close_geometry(left.divider_right, right.divider_right)
        && close_geometry(left.screen_left, right.screen_left)
        && close_geometry(left.screen_right, right.screen_right)
        && close_geometry(left.menu_top, right.menu_top)
        && close_geometry(left.menu_bottom, right.menu_bottom)
}

#[cfg(target_os = "macos")]
fn close_geometry(left: f64, right: f64) -> bool {
    left.is_finite() && right.is_finite() && (left - right).abs() <= GEOMETRY_TOLERANCE
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
    apply_physical_status_when_idle(app, current);
    let _ = app.emit(CHANGED_EVENT, current);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || crate::tray::refresh(&handle));
}

fn apply_physical_status_when_idle(app: &AppHandle, current: MenuBarStatus) {
    match INVENTORY_SESSION.try_lock() {
        // Keep the gate through `run_on_main_thread`'s enqueue. A later scan can
        // enqueue its synchronous geometry task only after this publication, so
        // the event-loop channel applies the state before the scan expands it.
        Ok(_operation) => status::apply(app, current.enabled, current.collapsed),
        Err(TryLockError::Poisoned(error)) => {
            let _operation = error.into_inner();
            status::apply(app, current.enabled, current.collapsed);
        }
        Err(TryLockError::WouldBlock) => schedule_status_reconciliation(app),
    }
}

fn schedule_status_reconciliation(app: &AppHandle) {
    if STATUS_RECONCILE_PENDING.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let _operation = INVENTORY_SESSION.lock_safe();
        // Clear this while still holding the gate. If state changes after the
        // snapshot below, its publication schedules one more waiter rather than
        // being folded into a reconciliation that has already read old state.
        STATUS_RECONCILE_PENDING.store(false, Ordering::SeqCst);
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        let current = status(state.inner());
        status::apply(&app, current.enabled, current.collapsed);
    });
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
            // `publish` defers the physical collapse if a scan or move owns the
            // divider, while still updating the panel and tray immediately.
            publish(&app);
        }
    });
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    fn item(name: &str) -> MenuBarItem {
        MenuBarItem {
            id: "raw".to_string(),
            name: name.to_string(),
            owner_name: Some("Example".to_string()),
            bundle_id: Some("com.example.app".to_string()),
            zone: MenuBarItemZone::Visible,
            position: 900.0,
            center_y: 12.0,
            width: 24.0,
            owner_pid: 42,
            ax_identifier: Some("example-item".to_string()),
        }
    }

    #[test]
    fn publishing_a_refresh_invalidates_previous_snapshot_ids() {
        let mut session = InventorySession::default();
        let first = publish_inventory(&mut session, true, true, true, vec![item("First")]);
        let first_id = first.items[0].id.clone();
        assert_eq!(session.latest[0].id, first_id);

        let second = publish_inventory(&mut session, true, true, true, vec![item("First")]);
        assert_ne!(second.items[0].id, first_id);
        assert_eq!(session.latest, second.items);
        assert!(unique_item(&session.latest, |candidate| candidate.id == first_id).is_none());
    }

    #[test]
    fn stable_accessibility_identity_survives_dynamic_label_changes() {
        let expected = item("Wi-Fi");
        assert!(tracks_same_item(&expected, &expected));

        let mut changed_label = expected.clone();
        changed_label.name = "CPU 14%".to_string();
        changed_label.owner_name = None;
        assert!(tracks_same_item(&expected, &changed_label));
    }

    #[test]
    fn tracking_rejects_reused_or_changed_accessibility_identity() {
        let expected = item("Wi-Fi");

        let mut changed_pid = expected.clone();
        changed_pid.owner_pid += 1;
        assert!(!tracks_same_item(&expected, &changed_pid));

        let mut changed_identifier = expected.clone();
        changed_identifier.ax_identifier = Some("other-item".to_string());
        assert!(!tracks_same_item(&expected, &changed_identifier));
    }

    #[test]
    fn identifierless_snapshot_requires_unchanged_label_and_geometry() {
        let mut expected = item("Example");
        expected.ax_identifier = None;
        assert!(tracks_snapshot_item(&expected, &expected));

        let mut changed_label = expected.clone();
        changed_label.name = "Replacement".to_string();
        assert!(!tracks_same_item(&expected, &changed_label));

        let mut shifted = expected.clone();
        shifted.position += 1.0;
        assert!(tracks_same_item(&expected, &shifted));
        assert!(!tracks_snapshot_item(&expected, &shifted));

        let mut resized = expected.clone();
        resized.width += 1.0;
        assert!(!tracks_snapshot_item(&expected, &resized));
    }

    #[test]
    fn drag_validation_rejects_changed_divider_geometry() {
        let context = status::ScanContext {
            divider_x: 800.0,
            divider_left: 794.0,
            divider_right: 806.0,
            screen_left: 0.0,
            screen_right: 1_000.0,
            menu_top: 0.0,
            menu_bottom: 24.0,
        };
        assert!(same_scan_context(context, context));
        assert!(same_scan_context(
            context,
            status::ScanContext {
                divider_x: 800.5,
                ..context
            }
        ));
        assert!(!same_scan_context(
            context,
            status::ScanContext {
                divider_x: 801.0,
                divider_left: 795.0,
                divider_right: 807.0,
                ..context
            }
        ));
        assert_eq!(
            zone_at_point(context, 700.0, 12.0),
            Some(MenuBarItemZone::Hidden)
        );
        assert_eq!(
            zone_at_point(context, 900.0, 12.0),
            Some(MenuBarItemZone::Visible)
        );
        assert_eq!(zone_at_point(context, 900.0, 40.0), None);
    }

    #[test]
    fn duplicate_tracking_candidates_are_ambiguous() {
        let candidate = item("Control Center");
        let candidates = vec![candidate.clone(), candidate];

        assert!(
            unique_item(&candidates, |item| item.name == "Control Center").is_none(),
            "an ambiguous match must never pick the first candidate"
        );
    }

    #[test]
    fn target_zone_deserializes_from_the_frontend_contract() {
        assert_eq!(
            serde_json::from_str::<MenuBarItemZone>(r#""hidden""#).unwrap(),
            MenuBarItemZone::Hidden
        );
        assert_eq!(
            serde_json::from_str::<MenuBarItemZone>(r#""visible""#).unwrap(),
            MenuBarItemZone::Visible
        );
        for (outcome, wire) in [
            (MenuBarMoveOutcome::Moved, r#""moved""#),
            (MenuBarMoveOutcome::AlreadyInZone, r#""alreadyInZone""#),
            (MenuBarMoveOutcome::StaleItem, r#""staleItem""#),
            (MenuBarMoveOutcome::NotMovable, r#""notMovable""#),
        ] {
            assert_eq!(serde_json::to_string(&outcome).unwrap(), wire);
        }
    }
}
