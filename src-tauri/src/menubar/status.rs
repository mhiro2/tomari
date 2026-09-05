//! The two `NSStatusItem`s that do the tidying, and the click handler on the
//! second one.
//!
//! AppKit offers no API to move another app's status item directly. Tomari can
//! reproduce the user's Command-drag gesture (see `movement`), but hiding still
//! works by owning an enormous item: the menu bar lays items out right to left,
//! so an item stretched to [`COLLAPSED_LENGTH`] pushes everything to its left
//! past the edge of the screen.
//!
//! Two items, with distinct jobs:
//!
//! - the **divider** is the boundary. It carries a visible mark so the user can
//!   grab it with ⌘-drag and place it, and it is the item that stretches.
//! - the **controller** is the handle. Fixed width, always clickable, always to
//!   the right of the divider so collapsing never swallows it.
//!
//! Stretching the divider itself and leaving it as the only item would put the
//! click target somewhere off the left edge of the screen while collapsed,
//! which is exactly where it cannot be clicked.
//!
//! AppKit objects are not `Send` and status items are main-thread only, so they
//! live in a main-thread `thread_local!` and every caller goes through [`apply`]
//! / [`teardown`], which hop. Both carry a generation for the same reason
//! `overlay` does: `run_on_main_thread` only queues, so without it a stale
//! "expand" landing after a teardown could resurrect items nothing owns.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSImage, NSScreen, NSStatusBar, NSStatusItem, NSVariableStatusItemLength};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString, ns_string};
use tauri::AppHandle;

/// How wide the divider grows to when collapsed. Far wider than any menu bar,
/// which is the point: everything left of it lands off-screen. A sentinel like
/// this rather than the current screen's width because which screen a status
/// item is "on" is not a stable question — Spaces, display changes and the
/// system's own reshuffling all move it — and the same value is what Ice and
/// Dozer have shipped for years.
const COLLAPSED_LENGTH: f64 = 10_000.0;

/// Width of the divider when expanded. Wide enough to be seen and grabbed, no
/// wider than an ordinary status item.
const DIVIDER_LENGTH: f64 = 12.0;

/// Position autosave keys. macOS remembers where the user ⌘-dragged an item
/// under these names — best effort on Apple's side, but without them the items
/// would land wherever the system pleases on every launch.
const DIVIDER_AUTOSAVE: &str = "TomariMenuBarDivider";
const CONTROLLER_AUTOSAVE: &str = "TomariMenuBarController";

thread_local! {
    /// The live items, or `None` while the feature is off. Main thread only.
    static ITEMS: RefCell<Option<Items>> = const { RefCell::new(None) };
}

/// Claimed by every [`apply`]/[`teardown`] as it is issued; a queued closure
/// applies itself only while its generation is still current, so whichever call
/// was made last in program order wins no matter what order the hops land in.
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// Cheap, read-only availability for Diagnostics. Unlike `scan_context`, this
/// never expands the divider or invalidates an inventory snapshot.
static DIVIDER_AVAILABLE: AtomicBool = AtomicBool::new(false);
/// Once quit begins, queued reconciliation and scan restoration must never
/// recreate status items behind teardown's unconditional removal.
static TERMINATING: AtomicBool = AtomicBool::new(false);

fn claim_generation() -> u64 {
    GENERATION.fetch_add(1, Ordering::SeqCst) + 1
}

fn is_current(generation: u64) -> bool {
    GENERATION.load(Ordering::SeqCst) == generation
}

struct Items {
    divider: Retained<NSStatusItem>,
    controller: Retained<NSStatusItem>,
    /// `NSControl.target` is a weak reference, so the handler has to be owned
    /// here. Drop it and the controller's clicks go nowhere.
    _target: Retained<ClickTarget>,
}

/// Geometry needed to classify Accessibility menu extras relative to Tomari's
/// divider. Coordinates use the AX/Core Graphics top-left global space.
#[derive(Debug, Clone, Copy)]
pub struct ScanContext {
    pub divider_x: f64,
    pub divider_left: f64,
    pub divider_right: f64,
    pub screen_left: f64,
    pub screen_right: f64,
    pub menu_top: f64,
    pub menu_bottom: f64,
}

/// Reflect `enabled` / `collapsed` onto the status items, creating or removing
/// them as needed. Hops to the main thread; safe to call from anywhere.
pub fn apply(app: &AppHandle, enabled: bool, collapsed: bool) {
    let generation = claim_generation();
    if TERMINATING.load(Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    let _ = app
        .clone()
        .run_on_main_thread(move || apply_on_main(&app, enabled, collapsed, generation));
}

/// Take the items down. Called on quit — not as a safety net (a status item
/// belongs to the process and goes with it, even on a crash) but so the menu
/// bar is tidy the moment Tomari is asked to leave, rather than a beat later
/// when the process actually exits.
pub fn teardown(app: &AppHandle) {
    TERMINATING.store(true, Ordering::SeqCst);
    claim_generation();
    let _ = app.run_on_main_thread(move || {
        remove_items();
    });
}

fn apply_on_main(app: &AppHandle, enabled: bool, collapsed: bool, generation: u64) {
    if TERMINATING.load(Ordering::SeqCst) || !is_current(generation) {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    if !enabled {
        remove_items();
        return;
    }
    ITEMS.with(|cell| {
        let mut slot = cell.borrow_mut();
        let items = slot.get_or_insert_with(|| make_items(mtm, app.clone()));
        DIVIDER_AVAILABLE.store(true, Ordering::SeqCst);
        set_collapsed(items, collapsed, mtm);
    });
}

fn remove_items() {
    let Some(items) = ITEMS.with(|cell| cell.borrow_mut().take()) else {
        DIVIDER_AVAILABLE.store(false, Ordering::SeqCst);
        return;
    };
    DIVIDER_AVAILABLE.store(false, Ordering::SeqCst);
    let bar = NSStatusBar::systemStatusBar();
    bar.removeStatusItem(&items.divider);
    bar.removeStatusItem(&items.controller);
}

pub fn divider_available() -> bool {
    DIVIDER_AVAILABLE.load(Ordering::SeqCst)
}

/// Stretch or restore the divider, and point the controller's chevron the way
/// the next click will go.
fn set_collapsed(items: &Items, collapsed: bool, mtm: MainThreadMarker) {
    items.divider.setLength(if collapsed {
        COLLAPSED_LENGTH
    } else {
        DIVIDER_LENGTH
    });
    // Collapsed, the divider is a 10,000pt expanse of nothing; a mark drawn in
    // it would sit at its left edge, off-screen. Only show the mark when it is
    // back to a width the user can actually see and grab.
    set_image(
        &items.divider,
        (!collapsed).then_some(("line.3.horizontal", "Tomari menu bar divider")),
        mtm,
    );
    set_image(
        &items.controller,
        Some(if collapsed {
            ("chevron.left", "Show hidden menu bar icons")
        } else {
            ("chevron.right", "Hide menu bar icons")
        }),
        mtm,
    );
}

/// Expand the divider and snapshot its screen-space boundary. Commands may run
/// on a worker thread, so hop to AppKit's main thread when necessary.
pub fn scan_context(app: &AppHandle) -> Option<ScanContext> {
    if TERMINATING.load(Ordering::SeqCst) {
        return None;
    }
    if let Some(mtm) = MainThreadMarker::new() {
        return scan_context_on_main(mtm);
    }

    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        let value = MainThreadMarker::new().and_then(scan_context_on_main);
        let _ = sender.send(value);
    })
    .ok()?;
    receiver
        .recv_timeout(std::time::Duration::from_secs(1))
        .ok()?
}

/// Restore the physical divider after an inventory scan. Unlike normal state
/// publication this waits for the main-thread hop, so the scan/move operation
/// gate is not released while the divider is still temporarily expanded.
pub fn finish_scan(app: &AppHandle, state: &crate::state::AppState) {
    let generation = claim_generation();
    if TERMINATING.load(Ordering::SeqCst) {
        return;
    }
    // Claim before reading. A state change that completed first is included in
    // this snapshot; one that races after the claim gets a newer generation
    // and its own queued apply wins over this restore.
    let current = super::status(state);
    if MainThreadMarker::new().is_some() {
        apply_on_main(app, current.enabled, current.collapsed, generation);
        return;
    }

    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let handle = app.clone();
    if app
        .run_on_main_thread(move || {
            apply_on_main(&handle, current.enabled, current.collapsed, generation);
            let _ = sender.send(());
        })
        .is_err()
    {
        tracing::warn!("could not schedule menu bar divider restoration");
        return;
    }
    if receiver
        .recv_timeout(std::time::Duration::from_secs(1))
        .is_err()
    {
        tracing::warn!("timed out restoring menu bar divider");
    }
}

fn scan_context_on_main(mtm: MainThreadMarker) -> Option<ScanContext> {
    if TERMINATING.load(Ordering::SeqCst) {
        return None;
    }
    ITEMS.with(|cell| {
        let items = cell.borrow();
        let items = items.as_ref()?;
        // The resting collapsed width puts the divider window off-screen. Put
        // it back before reading the frame so the boundary is meaningful.
        set_collapsed(items, false, mtm);
        let window = items.divider.button(mtm)?.window()?;
        let menu = window.frame();
        let screen = window.screen()?.frame();
        let screens = NSScreen::screens(mtm);
        let main_height = (0..screens.count())
            .map(|index| screens.objectAtIndex(index).frame())
            .find(|frame| frame.origin.x == 0.0 && frame.origin.y == 0.0)
            .or_else(|| (screens.count() > 0).then(|| screens.objectAtIndex(0).frame()))?
            .size
            .height;
        let menu_top = main_height - (menu.origin.y + menu.size.height);
        Some(ScanContext {
            divider_x: menu.origin.x + menu.size.width / 2.0,
            divider_left: menu.origin.x,
            divider_right: menu.origin.x + menu.size.width,
            screen_left: screen.origin.x,
            screen_right: screen.origin.x + screen.size.width,
            menu_top,
            menu_bottom: menu_top + menu.size.height,
        })
    })
}

fn make_items(mtm: MainThreadMarker, app: AppHandle) -> Items {
    let bar = NSStatusBar::systemStatusBar();

    // Order matters, and only on the very first launch: macOS drops each new
    // status item to the *left* of the ones already there, so the controller
    // has to be created first to end up on the divider's right. Created the
    // other way round it lands inside the region the divider sweeps off-screen
    // and the user loses the only handle for getting their icons back. After
    // this first placement the autosave names take over.
    let controller = bar.statusItemWithLength(NSVariableStatusItemLength);
    controller.setAutosaveName(Some(&NSString::from_str(CONTROLLER_AUTOSAVE)));
    let target = ClickTarget::new(mtm, app);
    if let Some(button) = controller.button(mtm) {
        button.setToolTip(Some(ns_string!("Tomari: show or hide menu bar icons")));
        // SAFETY: the target outlives the button — `Items` owns it — and the
        // selector is the one `ClickTarget` defines below.
        unsafe {
            button.setTarget(Some(&target));
            button.setAction(Some(sel!(tomariToggleMenuBar:)));
        }
    }

    let divider = bar.statusItemWithLength(DIVIDER_LENGTH);
    divider.setAutosaveName(Some(&NSString::from_str(DIVIDER_AUTOSAVE)));
    if let Some(button) = divider.button(mtm) {
        button.setToolTip(Some(ns_string!(
            "Tomari: drag menu bar icons left of this with ⌘ to hide them"
        )));
    }

    Items {
        divider,
        controller,
        _target: target,
    }
}

/// Put an SF Symbol on a status item's button, or clear it. Template mode lets
/// AppKit tint it for the menu bar's appearance instead of pinning a colour.
/// The description is spelled out rather than left to the symbol's own: VoiceOver
/// reads `chevron.left` as "Back", which is not what this button does.
fn set_image(item: &NSStatusItem, symbol: Option<(&str, &str)>, mtm: MainThreadMarker) {
    let Some(button) = item.button(mtm) else {
        return;
    };
    let Some((name, description)) = symbol else {
        button.setImage(None);
        return;
    };
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        Some(&NSString::from_str(description)),
    );
    if let Some(image) = image.as_deref() {
        image.setTemplate(true);
    }
    button.setImage(image.as_deref());
}

define_class!(
    // SAFETY:
    // - NSObject has no subclassing requirements.
    // - `ClickTarget` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TomariMenuBarClickTarget"]
    #[ivars = AppHandle]
    struct ClickTarget;

    impl ClickTarget {
        #[unsafe(method(tomariToggleMenuBar:))]
        fn tomari_toggle_menu_bar(&self, _sender: Option<&AnyObject>) {
            super::toggle(self.ivars());
        }
    }

    unsafe impl NSObjectProtocol for ClickTarget {}
);

impl ClickTarget {
    fn new(mtm: MainThreadMarker, app: AppHandle) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(app);
        unsafe { msg_send![super(this), init] }
    }
}
