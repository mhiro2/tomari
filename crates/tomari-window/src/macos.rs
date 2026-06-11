//! macOS implementation of [`WindowManager`] backed by the Accessibility API.
//!
//! Moving another application's window requires the *Accessibility* permission
//! (System Settings → Privacy & Security → Accessibility). We bind the handful
//! of stable HIServices C functions we need directly, and use Core Foundation /
//! Core Graphics value types for the rest.

#![allow(non_upper_case_globals)]

use std::ffi::c_void;

use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use core_foundation_sys::base::{CFHash, CFRelease, CFRetain, CFTypeRef};
use core_foundation_sys::dictionary::CFDictionaryRef;
use core_foundation_sys::string::CFStringRef;
use core_graphics::display::CGDisplay;
use core_graphics::geometry::{CGPoint, CGSize};
use objc2::MainThreadMarker;
use objc2_app_kit::NSScreen;
use tomari_core::domain::window::Rect;

use crate::error::{Error, Result};
use crate::manager::{WindowHandle, WindowManager};

type AXError = i32;
type AXValueType = u32;

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

/// Read a +1-retained attribute value off an element.
///
/// # Safety
/// `element` must be a valid `AXUIElementRef`.
unsafe fn copy_attr(element: CFTypeRef, name: &str) -> Option<CFOwned> {
    let attr = CFString::new(name);
    let mut value: CFTypeRef = std::ptr::null();
    let err =
        unsafe { AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value) };
    if err == kAXErrorSuccess && !value.is_null() {
        Some(CFOwned(value))
    } else {
        None
    }
}

/// Resolve the system-wide focused window, returning the owned CF handles for
/// the system element, focused application and focused window. The caller must
/// keep all three alive while using the window (it is owned by the others).
///
/// # Safety
/// Must run while the Accessibility permission is granted.
unsafe fn focused_window() -> Result<(CFOwned, CFOwned, CFOwned)> {
    let system = unsafe { AXUIElementCreateSystemWide() };
    if system.is_null() {
        return Err(Error::NoFocusedWindow);
    }
    let system = CFOwned(system);
    let app =
        unsafe { copy_attr(system.0, "AXFocusedApplication") }.ok_or(Error::NoFocusedWindow)?;
    let window = unsafe { copy_attr(app.0, "AXFocusedWindow") }.ok_or(Error::NoFocusedWindow)?;
    Ok((system, app, window))
}

/// Read a window's frame (CG coordinates, top-left origin) by decoding its
/// `AXPosition`/`AXSize` value objects.
///
/// # Safety
/// `window` must be a valid `AXUIElementRef`.
unsafe fn window_rect(window: CFTypeRef) -> Result<Rect> {
    let pos = unsafe { copy_attr(window, "AXPosition") }.ok_or(Error::NoFocusedWindow)?;
    let size = unsafe { copy_attr(window, "AXSize") }.ok_or(Error::NoFocusedWindow)?;
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
    /// menu-bar inset.
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
            let (_system, _app, window) = focused_window()?;
            Ok(Box::new(DragWindow { window }))
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

/// A handle to one AX window: what [`AxWindowManager`] resolves the focused
/// window to, and what a mouse gesture holds so repeated updates do not
/// re-hit-test under the cursor.
pub struct DragWindow {
    window: CFOwned,
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
        // Set position, then size, then position again: some windows clamp
        // their size until the origin is inside the target screen.
        let e1 = self.set_origin(frame.x, frame.y);
        let e2 = self.set_size(frame.width, frame.height);
        let _ = self.set_origin(frame.x, frame.y);
        e1?;
        e2
    }

    fn stable_hash(&self) -> u64 {
        // AXUIElement overrides CFHash/CFEqual so that two references to the
        // same UI element compare equal — good enough to tell "same window".
        unsafe { CFHash(self.window.0) as u64 }
    }
}

/// Read an element's `AXRole`, if it has one.
///
/// # Safety
/// `element` must be a valid `AXUIElementRef`.
unsafe fn element_role(element: CFTypeRef) -> Option<String> {
    let role = unsafe { copy_attr(element, "AXRole") }?;
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

        let mut hit: CFTypeRef = std::ptr::null();
        let err = AXUIElementCopyElementAtPosition(system.0, x as f32, y as f32, &mut hit);
        if err != kAXErrorSuccess || hit.is_null() {
            return Err(Error::NoFocusedWindow);
        }
        let mut element = CFOwned(hit);

        for _ in 0..32 {
            if element_role(element.0).as_deref() == Some("AXWindow") {
                return Ok(DragWindow { window: element });
            }
            if let Some(window) = copy_attr(element.0, "AXWindow") {
                return Ok(DragWindow { window });
            }
            match copy_attr(element.0, "AXParent") {
                Some(parent) => element = parent,
                None => break,
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
