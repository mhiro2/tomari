//! macOS implementation of [`WindowManager`] backed by the Accessibility API.
//!
//! Moving another application's window requires the *Accessibility* permission
//! (System Settings → Privacy & Security → Accessibility). We bind the handful
//! of stable HIServices C functions we need directly, and use Core Foundation /
//! Core Graphics value types for the rest.

#![allow(non_upper_case_globals, non_camel_case_types)]

use std::ffi::c_void;

use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex};
use core_foundation_sys::base::{CFGetTypeID, CFHash, CFRelease, CFRetain, CFTypeRef};
use core_foundation_sys::dictionary::{CFDictionaryGetValueIfPresent, CFDictionaryRef};
use core_foundation_sys::number::{CFNumberGetValue, kCFNumberSInt32Type};
use core_foundation_sys::string::{CFStringGetTypeID, CFStringRef};
use core_graphics::display::CGDisplay;
use core_graphics::geometry::{CGPoint, CGSize};
use core_graphics::window::{
    CGWindowListCopyWindowInfo, kCGNullWindowID, kCGWindowLayer,
    kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly, kCGWindowOwnerPID,
};
use objc2::MainThreadMarker;
use objc2_app_kit::NSScreen;
use tomari_core::domain::window::Rect;

use crate::error::{Error, Result};
use crate::manager::{WindowHandle, WindowManager};

type AXError = i32;
type AXValueType = u32;
type pid_t = i32;

const kAXErrorSuccess: AXError = 0;
const kAXValueTypeCGPoint: AXValueType = 1;
const kAXValueTypeCGSize: AXValueType = 2;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> u8;
    fn AXUIElementCreateSystemWide() -> CFTypeRef;
    fn AXUIElementCopyAttributeValue(
        element: CFTypeRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementSetAttributeValue(
        element: CFTypeRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> AXError;
    fn AXValueCreate(the_type: AXValueType, value_ptr: *const c_void) -> CFTypeRef;
    fn AXValueGetValue(value: CFTypeRef, the_type: AXValueType, value_ptr: *mut c_void) -> u8;
    fn AXUIElementCopyElementAtPosition(
        application: CFTypeRef,
        x: f32,
        y: f32,
        element: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementSetMessagingTimeout(element: CFTypeRef, timeout_in_seconds: f32) -> AXError;
    fn AXUIElementGetPid(element: CFTypeRef, pid: *mut pid_t) -> AXError;
    fn AXUIElementCreateApplication(pid: pid_t) -> CFTypeRef;
}

/// Cap on every Accessibility round-trip made through a dragged/hit-tested
/// element. A title-bar drag or a per-frame move issues these from an event-tap
/// thread; without a bound a target app whose AX server has wedged would block
/// that thread until the OS disables the tap (and, for the active drag-to-move
/// tap, stall input system-wide). 0.25 s is comfortably long for a healthy app
/// to answer yet short enough that a hung one cannot stall input perceptibly.
/// A bounded call simply returns an AX error, which the drag paths treat as
/// "abort this gesture".
const AX_MESSAGING_TIMEOUT_SECS: f32 = 0.25;

/// Bound every Accessibility message sent to `element` to
/// [`AX_MESSAGING_TIMEOUT_SECS`]. Best-effort: if the call fails the element
/// keeps the (unbounded) global default, so log it — that is the case where a
/// wedged app could still block a caller, and it must not pass silently.
///
/// # Safety
/// `element` must be a valid `AXUIElementRef`.
unsafe fn set_messaging_timeout(element: CFTypeRef) {
    let err = unsafe { AXUIElementSetMessagingTimeout(element, AX_MESSAGING_TIMEOUT_SECS) };
    if err != kAXErrorSuccess {
        tracing::warn!(
            error = err,
            "could not bound AX messaging timeout; calls stay unbounded"
        );
    }
}

/// RAII guard that `CFRelease`s an owned (`Copy`/`Create`-returned) CF object.
struct CFOwned(CFTypeRef);

impl Drop for CFOwned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}

/// `kAXErrorNoValue`: the requested attribute or element does not exist (as
/// opposed to the messaging round-trip itself failing).
const kAXErrorNoValue: AXError = -25212;

/// Read a +1-retained attribute value off an element.
///
/// Returns the raw `AXError` on failure rather than collapsing it, so callers
/// can tell a transient messaging failure (`kAXErrorCannotComplete`, e.g. the
/// bounded timeout in [`AX_MESSAGING_TIMEOUT_SECS`] tripping on a hung app)
/// from the attribute genuinely not existing (`kAXErrorNoValue`) or the
/// element genuinely being gone (`kAXErrorInvalidUIElement`).
///
/// # Safety
/// `element` must be a valid `AXUIElementRef`.
unsafe fn copy_attr(element: CFTypeRef, name: &str) -> std::result::Result<CFOwned, AXError> {
    let attr = CFString::new(name);
    let mut value: CFTypeRef = std::ptr::null();
    let err =
        unsafe { AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value) };
    if err == kAXErrorSuccess {
        if value.is_null() {
            // Successful call, but nothing came back: treat like "no value".
            Err(kAXErrorNoValue)
        } else {
            Ok(CFOwned(value))
        }
    } else {
        Err(err)
    }
}

/// Map an `AXError` from [`copy_attr`] to this crate's [`Error`]. `kAXErrorNoValue`
/// keeps the existing "nothing to act on" behavior (`NoFocusedWindow`); every
/// other code — including transient failures like `kAXErrorCannotComplete` —
/// is preserved as [`Error::Ax`] so [`Error::window_gone`] can tell a hung app
/// apart from one that has truly gone away.
fn map_attr_err(err: AXError) -> Error {
    if err == kAXErrorNoValue {
        Error::NoFocusedWindow
    } else {
        Error::Ax(err)
    }
}

/// Read an `AXUIElement`'s owning process ID.
///
/// # Safety
/// `element` must be a valid `AXUIElementRef`.
unsafe fn element_pid(element: CFTypeRef) -> Option<pid_t> {
    let mut pid: pid_t = 0;
    let err = unsafe { AXUIElementGetPid(element, &mut pid) };
    (err == kAXErrorSuccess).then_some(pid)
}

/// Read a `CFNumber` dictionary value as an `i32`, if present and numeric.
fn dict_get_i32(dict: CFTypeRef, key: CFStringRef) -> Option<i32> {
    let mut value: *const c_void = std::ptr::null();
    let found =
        unsafe { CFDictionaryGetValueIfPresent(dict as CFDictionaryRef, key.cast(), &mut value) };
    if found == 0 || value.is_null() {
        return None;
    }
    let mut out: i32 = 0;
    let ok = unsafe {
        CFNumberGetValue(
            value.cast(),
            kCFNumberSInt32Type,
            (&mut out as *mut i32).cast(),
        )
    };
    ok.then_some(out)
}

/// Find the PID owning the frontmost on-screen window that is not `exclude_pid`.
///
/// Walks the system's on-screen window list (front-to-back order, desktop
/// elements excluded) and returns the owner of the first normal-level
/// (`kCGWindowLayer == 0`) window belonging to a different process — i.e. the
/// application actually visible behind our own frontmost window.
fn frontmost_other_app_pid(exclude_pid: pid_t) -> Option<pid_t> {
    let list = unsafe {
        CGWindowListCopyWindowInfo(
            kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
            kCGNullWindowID,
        )
    };
    if list.is_null() {
        return None;
    }
    let list = CFOwned(list as CFTypeRef);

    let count = unsafe { CFArrayGetCount(list.0 as _) };
    for i in 0..count {
        let entry = unsafe { CFArrayGetValueAtIndex(list.0 as _, i) } as CFTypeRef;
        if entry.is_null() {
            continue;
        }
        let layer = dict_get_i32(entry, unsafe { kCGWindowLayer });
        if layer != Some(0) {
            continue;
        }
        let owner_pid = dict_get_i32(entry, unsafe { kCGWindowOwnerPID });
        match owner_pid {
            Some(pid) if pid != exclude_pid => return Some(pid),
            _ => continue,
        }
    }
    None
}

/// Resolve the system-wide focused window, returning the owned CF handles for
/// the system element, focused application and focused window. The window
/// handle is +1-retained and stays valid on its own once returned; the system
/// and application handles are returned alongside it only because they too
/// are owned and must eventually be released, not because the window
/// borrows from them.
///
/// When the system-reported focused application is this very process — which
/// happens the instant a click lands on one of Tomari's own windows, e.g. the
/// preset grid in the settings window — fall back to the frontmost *other*
/// application's focused window instead, so a snap never targets Tomari's own
/// UI.
///
/// # Safety
/// Must run while the Accessibility permission is granted.
unsafe fn focused_window() -> Result<(CFOwned, CFOwned, CFOwned)> {
    let system = unsafe { AXUIElementCreateSystemWide() };
    if system.is_null() {
        return Err(Error::NoFocusedWindow);
    }
    let system = CFOwned(system);
    // Bound every AX round-trip that follows before the first one can block on a
    // wedged focused app. Passing the system-wide element sets the timeout
    // process-globally (per `AXUIElementSetMessagingTimeout`), so it also covers
    // the focused-application and focused-window reads below — and any fallback
    // application element created afterwards — without a per-element set on each.
    unsafe { set_messaging_timeout(system.0) };
    let app = unsafe { copy_attr(system.0, "AXFocusedApplication") }.map_err(map_attr_err)?;

    let own_pid = std::process::id() as pid_t;
    if unsafe { element_pid(app.0) } == Some(own_pid) {
        let other_pid = frontmost_other_app_pid(own_pid).ok_or(Error::NoFocusedWindow)?;
        let other_app = unsafe { AXUIElementCreateApplication(other_pid) };
        if other_app.is_null() {
            return Err(Error::NoFocusedWindow);
        }
        let other_app = CFOwned(other_app);
        // Covered by the process-global timeout set on `system` above.
        let window = unsafe { copy_attr(other_app.0, "AXFocusedWindow") }.map_err(map_attr_err)?;
        return Ok((system, other_app, window));
    }

    let window = unsafe { copy_attr(app.0, "AXFocusedWindow") }.map_err(map_attr_err)?;
    Ok((system, app, window))
}

/// Read a window's frame (CG coordinates, top-left origin) by decoding its
/// `AXPosition`/`AXSize` value objects.
///
/// # Safety
/// `window` must be a valid `AXUIElementRef`.
unsafe fn window_rect(window: CFTypeRef) -> Result<Rect> {
    let pos = unsafe { copy_attr(window, "AXPosition") }.map_err(map_attr_err)?;
    let size = unsafe { copy_attr(window, "AXSize") }.map_err(map_attr_err)?;
    let mut point = CGPoint { x: 0.0, y: 0.0 };
    let mut sz = CGSize {
        width: 0.0,
        height: 0.0,
    };
    let got_pos = unsafe {
        AXValueGetValue(
            pos.0,
            kAXValueTypeCGPoint,
            (&mut point as *mut CGPoint).cast(),
        )
    };
    let got_size =
        unsafe { AXValueGetValue(size.0, kAXValueTypeCGSize, (&mut sz as *mut CGSize).cast()) };
    if got_pos == 0 || got_size == 0 {
        return Err(Error::NoFocusedWindow);
    }
    Ok(Rect::new(point.x, point.y, sz.width, sz.height))
}

/// A display's full frame and usable (Dock/menu-bar/notch-excluded) frame, in
/// Cocoa coordinates (bottom-left origin). Kept as plain values so the layout
/// math can be unit-tested without AppKit.
#[derive(Debug, Clone, Copy)]
struct ScreenInfo {
    frame: Rect,
    visible_frame: Rect,
}

/// The main screen — the one whose Cocoa frame origin is `(0, 0)` and so anchors
/// the coordinate space — falling back to the first screen. `None` only when no
/// screens were reported, so callers do not have to guard an empty slice index.
fn main_screen(screens: &[ScreenInfo]) -> Option<ScreenInfo> {
    screens
        .iter()
        .find(|s| s.frame.x == 0.0 && s.frame.y == 0.0)
        .or_else(|| screens.first())
        .copied()
}

/// Height of the main screen, the basis for converting Cocoa Y to CG Y.
fn main_screen_height(screens: &[ScreenInfo]) -> Option<f64> {
    main_screen(screens).map(|s| s.frame.height)
}

/// Convert a Cocoa rect (bottom-left origin, Y up) to a CG rect (top-left
/// origin, Y down) given the main screen height `h`.
fn cocoa_rect_to_cg(rect: Rect, h: f64) -> Rect {
    Rect::new(rect.x, h - (rect.y + rect.height), rect.width, rect.height)
}

fn rect_center(rect: Rect) -> (f64, f64) {
    (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0)
}

fn rect_contains(rect: Rect, px: f64, py: f64) -> bool {
    px >= rect.x && px < rect.x + rect.width && py >= rect.y && py < rect.y + rect.height
}

/// The usable work area (CG coordinates) of the display containing the focused
/// window. Picks the screen whose CG frame contains the window's center, and
/// falls back to the main screen's visible frame when none matches.
fn work_area_for_window(screens: &[ScreenInfo], window_cg: Rect) -> Option<Rect> {
    let main = main_screen(screens)?;
    let h = main.frame.height;
    let (cx, cy) = rect_center(window_cg);
    for s in screens {
        if rect_contains(cocoa_rect_to_cg(s.frame, h), cx, cy) {
            return Some(cocoa_rect_to_cg(s.visible_frame, h));
        }
    }
    Some(cocoa_rect_to_cg(main.visible_frame, h))
}

/// Snapshot every screen's frame and visible frame (Cocoa coordinates).
fn collect_screens(mtm: MainThreadMarker) -> Vec<ScreenInfo> {
    let screens = NSScreen::screens(mtm);
    let mut out = Vec::with_capacity(screens.count());
    for i in 0..screens.count() {
        let screen = screens.objectAtIndex(i);
        let f = screen.frame();
        let v = screen.visibleFrame();
        out.push(ScreenInfo {
            frame: Rect::new(f.origin.x, f.origin.y, f.size.width, f.size.height),
            visible_frame: Rect::new(v.origin.x, v.origin.y, v.size.width, v.size.height),
        });
    }
    out
}

/// [`WindowManager`] driven by the macOS Accessibility API.
#[derive(Debug, Clone)]
pub struct AxWindowManager {
    /// Height (points) of the menu bar to exclude from the top of the screen.
    menu_bar_inset: f64,
}

impl Default for AxWindowManager {
    fn default() -> Self {
        Self {
            // Only used by `cg_fallback_work_area` when the real per-display
            // visible frame cannot be read. 25pt matches a pre-notch menu bar;
            // notched Macs' menu bar is taller (roughly 32-38pt), so this
            // fallback is a known-imprecise approximation on those machines.
            menu_bar_inset: 25.0,
        }
    }
}

impl AxWindowManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_menu_bar_inset(inset: f64) -> Self {
        Self {
            menu_bar_inset: inset,
        }
    }

    /// Fallback work area when AppKit's per-display visible frame is unavailable
    /// (e.g. called off the main thread): the main display minus a fixed
    /// menu-bar inset. The default inset (see [`Default`] below) is shorter
    /// than the actual menu bar on notched Macs (roughly 32-38pt), so on those
    /// machines this fallback can place a window's top edge under the menu
    /// bar / notch area instead of flush below it.
    fn cg_fallback_work_area(&self) -> Rect {
        let bounds = CGDisplay::main().bounds();
        Rect::new(
            bounds.origin.x,
            bounds.origin.y + self.menu_bar_inset,
            bounds.size.width,
            (bounds.size.height - self.menu_bar_inset).max(0.0),
        )
    }
}

impl WindowManager for AxWindowManager {
    fn permission_granted(&self) -> bool {
        unsafe { AXIsProcessTrusted() != 0 }
    }

    fn focused_window(&self) -> Result<Box<dyn WindowHandle>> {
        unsafe {
            if AXIsProcessTrusted() == 0 {
                return Err(Error::PermissionDenied);
            }
            // The window element is +1-retained, so it stays valid on its own
            // after the system-wide and application elements are released.
            // This holds in the fallback path too: `_app` there is a fresh
            // `AXUIElementCreateApplication` handle for the frontmost other
            // process, and its `AXFocusedWindow` is copied (not borrowed) from it.
            let (_system, _app, window) = focused_window()?;
            Ok(Box::new(DragWindow::new(window)))
        }
    }

    fn work_area(&self, window_frame: Rect) -> Result<Rect> {
        // Prefer the real visible frame (Dock, menu bar and notch excluded) of
        // the display the focused window is on. Fall back to the main display
        // minus a fixed inset only when AppKit can't be reached.
        if let Some(mtm) = MainThreadMarker::new() {
            let screens = collect_screens(mtm);
            if let Some(area) = work_area_for_window(&screens, window_frame) {
                return Ok(area);
            }
        }
        Ok(self.cg_fallback_work_area())
    }

    fn screen_work_areas(&self) -> Result<Vec<Rect>> {
        if let Some(mtm) = MainThreadMarker::new() {
            let screens = collect_screens(mtm);
            if let Some(h) = main_screen_height(&screens) {
                return Ok(screens
                    .iter()
                    .map(|s| cocoa_rect_to_cg(s.visible_frame, h))
                    .collect());
            }
        }
        Ok(vec![self.cg_fallback_work_area()])
    }

    fn screens_cg(&self) -> Result<Vec<(Rect, Rect)>> {
        if let Some(mtm) = MainThreadMarker::new() {
            let screens = collect_screens(mtm);
            if let Some(h) = main_screen_height(&screens) {
                return Ok(screens
                    .iter()
                    .map(|s| {
                        (
                            cocoa_rect_to_cg(s.frame, h),
                            cocoa_rect_to_cg(s.visible_frame, h),
                        )
                    })
                    .collect());
            }
        }
        // Off the main thread (or no screens reported): the main display only.
        let bounds = CGDisplay::main().bounds();
        let full = Rect::new(
            bounds.origin.x,
            bounds.origin.y,
            bounds.size.width,
            bounds.size.height,
        );
        Ok(vec![(full, self.cg_fallback_work_area())])
    }
}

/// Write a window's `AXPosition`.
///
/// # Safety
/// `window` must be a valid `AXUIElementRef`.
unsafe fn set_window_position(window: CFTypeRef, x: f64, y: f64) -> Result<()> {
    let point = CGPoint { x, y };
    let value = unsafe { AXValueCreate(kAXValueTypeCGPoint, (&point as *const CGPoint).cast()) };
    if value.is_null() {
        return Err(Error::Ax(-1));
    }
    let value = CFOwned(value);
    let attr = CFString::new("AXPosition");
    let err = unsafe { AXUIElementSetAttributeValue(window, attr.as_concrete_TypeRef(), value.0) };
    if err != kAXErrorSuccess {
        return Err(Error::Ax(err));
    }
    Ok(())
}

/// Write a window's `AXSize`.
///
/// # Safety
/// `window` must be a valid `AXUIElementRef`.
unsafe fn set_window_size(window: CFTypeRef, width: f64, height: f64) -> Result<()> {
    let size = CGSize { width, height };
    let value = unsafe { AXValueCreate(kAXValueTypeCGSize, (&size as *const CGSize).cast()) };
    if value.is_null() {
        return Err(Error::Ax(-1));
    }
    let value = CFOwned(value);
    let attr = CFString::new("AXSize");
    let err = unsafe { AXUIElementSetAttributeValue(window, attr.as_concrete_TypeRef(), value.0) };
    if err != kAXErrorSuccess {
        return Err(Error::Ax(err));
    }
    Ok(())
}

/// Apply a frame as position → size → position, rolling back to `original` on
/// any failure so the outcome is all-or-nothing rather than a half-applied
/// window. The second position write corrects an origin that the first left
/// clamped because the old (larger) size pushed it off-screen.
///
/// Every step must succeed: a comparison against a read-back is deliberately
/// *not* used as the success test, because a window may legitimately clamp to a
/// minimum size, which is not a failure. If any write fails the window may be
/// half-applied, so the same origin/size/origin sequence is replayed toward
/// `original` (best-effort — a rollback write that itself fails is ignored, as
/// there is nothing better to do) and the first error is returned. `original`
/// is `None` only when the pre-move frame could not be read, leaving nothing to
/// roll back to.
///
/// Generic over the two write ops so this all-or-nothing logic is testable
/// without a live Accessibility window.
fn apply_frame_sequence(
    original: Option<Rect>,
    frame: Rect,
    mut set_origin: impl FnMut(f64, f64) -> Result<()>,
    mut set_size: impl FnMut(f64, f64) -> Result<()>,
) -> Result<()> {
    let r1 = set_origin(frame.x, frame.y);
    let r2 = set_size(frame.width, frame.height);
    let r3 = set_origin(frame.x, frame.y);
    if let Some(err) = r1.err().or(r2.err()).or(r3.err()) {
        if let Some(original) = original {
            let _ = set_origin(original.x, original.y);
            let _ = set_size(original.width, original.height);
            let _ = set_origin(original.x, original.y);
        }
        return Err(err);
    }
    Ok(())
}

/// A handle to one AX window: what [`AxWindowManager`] resolves the focused
/// window to, and what a mouse gesture holds so repeated updates do not
/// re-hit-test under the cursor.
pub struct DragWindow {
    window: CFOwned,
}

impl DragWindow {
    /// Wrap an owned AX window element, bounding every later round-trip to it so
    /// a wedged target app cannot block the thread that drags or measures it.
    fn new(window: CFOwned) -> Self {
        unsafe { set_messaging_timeout(window.0) };
        Self { window }
    }
}

// SAFETY: an `AXUIElementRef` is a CoreFoundation object (thread-safe
// retain/release) and the HIServices accessibility client API it is used with
// is documented as thread-safe, so the handle may move between threads.
unsafe impl Send for DragWindow {}

impl Clone for DragWindow {
    fn clone(&self) -> Self {
        // CFRetain the underlying element so both handles own a reference.
        unsafe { CFRetain(self.window.0) };
        Self {
            window: CFOwned(self.window.0),
        }
    }
}

impl DragWindow {
    /// Move the window so its top-left corner sits at (`x`, `y`).
    pub fn set_origin(&self, x: f64, y: f64) -> Result<()> {
        unsafe { set_window_position(self.window.0, x, y) }
    }

    /// Resize the window, keeping its top-left corner anchored.
    pub fn set_size(&self, width: f64, height: f64) -> Result<()> {
        unsafe { set_window_size(self.window.0, width, height) }
    }
}

impl WindowHandle for DragWindow {
    /// The window's current frame (CG coordinates, top-left origin).
    fn frame(&self) -> Result<Rect> {
        unsafe { window_rect(self.window.0) }
    }

    fn set_frame(&self, frame: Rect) -> Result<()> {
        // Capture the starting frame so a partial application — the window moved
        // but a later step failed — can be rolled back rather than left stranded
        // somewhere the user never asked for. `None` when it cannot be read, in
        // which case there is nothing to roll back to.
        let original = self.frame().ok();
        apply_frame_sequence(
            original,
            frame,
            |x, y| unsafe { set_window_position(self.window.0, x, y) },
            |w, h| unsafe { set_window_size(self.window.0, w, h) },
        )
    }

    fn stable_hash(&self) -> u64 {
        // AXUIElement overrides CFHash/CFEqual so that two references to the
        // same UI element compare equal — good enough to tell "same window".
        unsafe { CFHash(self.window.0) as u64 }
    }
}

/// Read an element's `AXRole`, if it has one.
///
/// `window_at_point` walks elements owned by whatever third-party app is under
/// the cursor, so the returned attribute value cannot be trusted to actually be
/// a `CFString` — a misbehaving or unusual AX implementation could hand back
/// any CF type. Check the runtime type ID before reinterpreting the pointer as
/// a `CFStringRef`; a mismatch returns `None` instead of reading through a
/// wrongly-typed pointer.
///
/// # Safety
/// `element` must be a valid `AXUIElementRef`.
unsafe fn element_role(element: CFTypeRef) -> Option<String> {
    let role = unsafe { copy_attr(element, "AXRole") }.ok()?;
    if unsafe { CFGetTypeID(role.0) } != unsafe { CFStringGetTypeID() } {
        return None;
    }
    let s = unsafe { CFString::wrap_under_get_rule(role.0 as CFStringRef) };
    Some(s.to_string())
}

/// Hit-test the window under the point (`x`, `y`) in CG coordinates and return
/// a handle for dragging it. The hit element is usually a control deep inside
/// the window, so walk to the owning window via `AXWindow` / `AXParent`.
pub fn window_at_point(x: f64, y: f64) -> Result<DragWindow> {
    unsafe {
        if AXIsProcessTrusted() == 0 {
            return Err(Error::PermissionDenied);
        }
        let system = AXUIElementCreateSystemWide();
        if system.is_null() {
            return Err(Error::NoFocusedWindow);
        }
        let system = CFOwned(system);
        // Bound the hit-test and everything after it: passing the system-wide
        // element sets the timeout process-globally, so the initial
        // position hit-test *and* the `AXWindow`/`AXParent` walk below — each
        // messaging the app under the cursor, possibly the wedged one we must
        // not block on — are all covered before the first round-trip is sent.
        set_messaging_timeout(system.0);

        let mut hit: CFTypeRef = std::ptr::null();
        let err = AXUIElementCopyElementAtPosition(system.0, x as f32, y as f32, &mut hit);
        if err != kAXErrorSuccess || hit.is_null() {
            return Err(Error::NoFocusedWindow);
        }
        let mut element = CFOwned(hit);

        for _ in 0..32 {
            if element_role(element.0).as_deref() == Some("AXWindow") {
                return Ok(DragWindow::new(element));
            }
            if let Ok(window) = copy_attr(element.0, "AXWindow") {
                return Ok(DragWindow::new(window));
            }
            match copy_attr(element.0, "AXParent") {
                Ok(parent) => element = parent,
                Err(_) => break,
            }
        }
        Err(Error::NoFocusedWindow)
    }
}

/// Prompt the user to grant the Accessibility permission (shows the system
/// dialog the first time). Returns whether the process is already trusted.
pub fn request_permission() -> bool {
    let key = CFString::new("AXTrustedCheckOptionPrompt");
    let value = CFBoolean::true_value();
    let options = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), value.as_CFType())]);
    unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) != 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(frame: Rect, visible: Rect) -> ScreenInfo {
        ScreenInfo {
            frame,
            visible_frame: visible,
        }
    }

    #[test]
    fn apply_frame_sequence_writes_position_size_position_on_success() {
        use std::cell::RefCell;
        let calls = RefCell::new(Vec::new());
        let frame = Rect::new(10.0, 20.0, 300.0, 400.0);
        let res = apply_frame_sequence(
            Some(Rect::new(0.0, 0.0, 100.0, 100.0)),
            frame,
            |x, y| {
                calls.borrow_mut().push(format!("origin {x} {y}"));
                Ok(())
            },
            |w, h| {
                calls.borrow_mut().push(format!("size {w} {h}"));
                Ok(())
            },
        );
        assert!(res.is_ok());
        assert_eq!(
            *calls.borrow(),
            vec![
                "origin 10 20".to_string(),
                "size 300 400".to_string(),
                "origin 10 20".to_string(),
            ],
            "no rollback runs when every write succeeds"
        );
    }

    #[test]
    fn apply_frame_sequence_rolls_back_to_original_when_a_write_fails() {
        use std::cell::RefCell;
        let calls = RefCell::new(Vec::new());
        let original = Rect::new(1.0, 2.0, 3.0, 4.0);
        let frame = Rect::new(10.0, 20.0, 300.0, 400.0);
        let res = apply_frame_sequence(
            Some(original),
            frame,
            |x, y| {
                calls.borrow_mut().push(format!("origin {x} {y}"));
                Ok(())
            },
            |w, h| {
                calls.borrow_mut().push(format!("size {w} {h}"));
                Err(Error::Ax(-25200))
            },
        );
        assert!(matches!(res, Err(Error::Ax(-25200))));
        assert_eq!(
            *calls.borrow(),
            vec![
                // Forward: all three run (the size failure surfaces after).
                "origin 10 20".to_string(),
                "size 300 400".to_string(),
                "origin 10 20".to_string(),
                // Rollback toward the starting frame.
                "origin 1 2".to_string(),
                "size 3 4".to_string(),
                "origin 1 2".to_string(),
            ],
        );
    }

    #[test]
    fn apply_frame_sequence_treats_a_failed_final_origin_as_failure() {
        // Regression: the old code dropped the second set_origin result, so a
        // window that clamped its origin and could not be re-positioned still
        // reported success.
        use std::cell::Cell;
        let origin_calls = Cell::new(0);
        let res = apply_frame_sequence(
            Some(Rect::new(1.0, 2.0, 3.0, 4.0)),
            Rect::new(10.0, 20.0, 300.0, 400.0),
            |_x, _y| {
                let n = origin_calls.get();
                origin_calls.set(n + 1);
                // The second forward origin write (index 1) fails.
                if n == 1 {
                    Err(Error::Ax(-25200))
                } else {
                    Ok(())
                }
            },
            |_w, _h| Ok(()),
        );
        assert!(
            res.is_err(),
            "a failed final origin must not report success"
        );
        assert_eq!(
            origin_calls.get(),
            4,
            "two forward origin writes plus two rollback origin writes"
        );
    }

    #[test]
    fn apply_frame_sequence_without_a_readable_original_skips_rollback() {
        use std::cell::RefCell;
        let calls = RefCell::new(Vec::new());
        let res = apply_frame_sequence(
            None,
            Rect::new(10.0, 20.0, 300.0, 400.0),
            |_x, _y| {
                calls.borrow_mut().push("origin");
                Err(Error::Ax(-1))
            },
            |_w, _h| {
                calls.borrow_mut().push("size");
                Ok(())
            },
        );
        assert!(res.is_err());
        assert_eq!(
            calls.borrow().len(),
            3,
            "only the forward writes run; nothing to roll back to"
        );
    }

    #[test]
    fn cocoa_y_flips_against_main_height() {
        // On a 1080-tall main screen, a 100-tall rect at Cocoa y=0 (bottom) maps
        // to CG y = 1080 - (0 + 100) = 980.
        let cg = cocoa_rect_to_cg(Rect::new(0.0, 0.0, 200.0, 100.0), 1080.0);
        assert_eq!(cg, Rect::new(0.0, 980.0, 200.0, 100.0));
    }

    #[test]
    fn single_screen_returns_its_visible_frame_in_cg() {
        // 1920x1080 main, visible frame inset by a 25pt menu bar.
        let screens = [screen(
            Rect::new(0.0, 0.0, 1920.0, 1080.0),
            Rect::new(0.0, 0.0, 1920.0, 1055.0),
        )];
        let area = work_area_for_window(&screens, Rect::new(800.0, 400.0, 400.0, 300.0)).unwrap();
        assert_eq!(area, Rect::new(0.0, 25.0, 1920.0, 1055.0));
    }

    #[test]
    fn window_on_secondary_left_display_uses_that_display() {
        let main = screen(
            Rect::new(0.0, 0.0, 1920.0, 1080.0),
            Rect::new(0.0, 0.0, 1920.0, 1055.0),
        );
        // Secondary display to the left of main (Cocoa x = -1440).
        let left = screen(
            Rect::new(-1440.0, 0.0, 1440.0, 900.0),
            Rect::new(-1440.0, 0.0, 1440.0, 875.0),
        );
        let screens = [main, left];
        // A window whose CG center lands on the left display.
        let win = Rect::new(-1200.0, 100.0, 400.0, 300.0);
        let area = work_area_for_window(&screens, win).unwrap();
        assert_eq!(area, cocoa_rect_to_cg(left.visible_frame, 1080.0));
        // Not the main display's area.
        assert!(area.x < 0.0);
    }

    #[test]
    fn window_off_all_screens_falls_back_to_main_visible_frame() {
        let main = screen(
            Rect::new(0.0, 0.0, 1920.0, 1080.0),
            Rect::new(0.0, 0.0, 1920.0, 1055.0),
        );
        let screens = [main];
        let win = Rect::new(-5000.0, -5000.0, 100.0, 100.0);
        let area = work_area_for_window(&screens, win).unwrap();
        assert_eq!(area, cocoa_rect_to_cg(main.visible_frame, 1080.0));
    }

    #[test]
    fn empty_screens_yields_none() {
        assert!(work_area_for_window(&[], Rect::new(0.0, 0.0, 10.0, 10.0)).is_none());
    }
}
