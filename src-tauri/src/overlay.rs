//! The translucent preview shown while a window is dragged toward a screen
//! edge, marking where it will snap on release.
//!
//! The preview is a borderless, non-activating, click-through `NSPanel` with a
//! layer-backed fill and border. AppKit window objects are neither `Send` nor
//! safe to touch off the main thread, so the panel lives in a main-thread
//! `thread_local!` and the drag-to-snap tap (which runs on its own thread)
//! drives it solely through [`show`] / [`hide`], each hopping to the main
//! thread. The
//! frame arrives in CG coordinates (top-left origin) — the space the snap
//! geometry works in — and is converted to Cocoa coordinates here.

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSPanel, NSScreen, NSStatusWindowLevel,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use tauri::AppHandle;
use tomari_core::Rect;

thread_local! {
    /// The single preview panel, created on first show and reused thereafter.
    /// Only ever touched on the main thread.
    static PANEL: RefCell<Option<Retained<NSPanel>>> = const { RefCell::new(None) };
}

/// Show the snap preview at `frame_cg` (CG coordinates, top-left origin),
/// creating the panel on first use. Hops to the main thread.
pub fn show(app: &AppHandle, frame_cg: Rect) {
    let app = app.clone();
    let _ = app.run_on_main_thread(move || show_on_main(frame_cg));
}

/// Hide the snap preview if it is showing. Idempotent and cheap. Hops to the
/// main thread.
pub fn hide(app: &AppHandle) {
    let app = app.clone();
    let _ = app.run_on_main_thread(hide_on_main);
}

fn show_on_main(frame_cg: Rect) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let cocoa = cg_rect_to_cocoa(frame_cg, mtm);
    PANEL.with(|cell| {
        let mut slot = cell.borrow_mut();
        let panel = slot.get_or_insert_with(|| make_panel(mtm));
        panel.setFrame_display(cocoa, true);
        panel.orderFrontRegardless();
    });
}

fn hide_on_main() {
    PANEL.with(|cell| {
        if let Some(panel) = cell.borrow().as_ref() {
            panel.orderOut(None);
        }
    });
}

/// Build the reusable preview panel: borderless, non-activating, click-through,
/// floating above ordinary windows and present on every Space (so it shows over
/// full-screen apps too).
fn make_panel(mtm: MainThreadMarker) -> Retained<NSPanel> {
    let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
        NSPanel::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)),
        NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
        NSBackingStoreType::Buffered,
        false,
    );
    panel.setFloatingPanel(true);
    panel.setBecomesKeyOnlyIfNeeded(true);
    panel.setOpaque(false);
    panel.setHasShadow(false);
    // Click-through: the panel must never intercept the drag in progress, nor
    // pollute the Accessibility hit-test for the window underneath.
    panel.setIgnoresMouseEvents(true);
    panel.setLevel(NSStatusWindowLevel);
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Stationary,
    );
    let clear = NSColor::clearColor();
    panel.setBackgroundColor(Some(&clear));
    configure_layer(&panel);
    panel
}

/// Give the panel's content view a translucent fill with a rounded, more opaque
/// border, drawn through its backing layer.
fn configure_layer(panel: &NSPanel) {
    let Some(content) = panel.contentView() else {
        return;
    };
    content.setWantsLayer(true);
    let Some(layer) = content.layer() else {
        return;
    };
    let fill = NSColor::colorWithSRGBRed_green_blue_alpha(0.15, 0.5, 0.95, 0.25);
    let border = NSColor::colorWithSRGBRed_green_blue_alpha(0.15, 0.5, 0.95, 0.9);
    let fill_cg = fill.CGColor();
    let border_cg = border.CGColor();
    layer.setBackgroundColor(Some(&fill_cg));
    layer.setBorderColor(Some(&border_cg));
    layer.setBorderWidth(2.0);
    layer.setCornerRadius(10.0);
}

/// Convert a CG rect (top-left origin, y down) to a Cocoa rect (bottom-left
/// origin, y up) for placing the panel. The flip is around the primary screen's
/// height, the same basis `macos.rs` uses in the other direction.
fn cg_rect_to_cocoa(rect: Rect, mtm: MainThreadMarker) -> NSRect {
    let main_h = primary_screen_height(mtm);
    let cocoa_y = main_h - (rect.y + rect.height);
    NSRect::new(
        NSPoint::new(rect.x, cocoa_y),
        NSSize::new(rect.width, rect.height),
    )
}

/// Height (points) of the primary screen — the one whose Cocoa frame origin is
/// `(0, 0)` and so anchors the coordinate flip — falling back to the first.
fn primary_screen_height(mtm: MainThreadMarker) -> f64 {
    let screens = NSScreen::screens(mtm);
    let count = screens.count();
    for i in 0..count {
        let frame = screens.objectAtIndex(i).frame();
        if frame.origin.x == 0.0 && frame.origin.y == 0.0 {
            return frame.size.height;
        }
    }
    if count > 0 {
        screens.objectAtIndex(0).frame().size.height
    } else {
        0.0
    }
}
