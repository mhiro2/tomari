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
use core_foundation_sys::dictionary::{
    CFDictionaryGetTypeID, CFDictionaryGetValueIfPresent, CFDictionaryRef,
};
use core_foundation_sys::number::{CFNumberGetValue, kCFNumberSInt32Type};
use core_foundation_sys::string::{CFStringGetTypeID, CFStringRef};
use core_graphics::display::CGDisplay;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_graphics::window::{
    CGWindowListCopyWindowInfo, kCGNullWindowID, kCGWindowBounds, kCGWindowLayer,
    kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly, kCGWindowOwnerPID,
};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSRunningApplication, NSScreen};
use tomari_core::domain::window::{Rect, WindowApplication};

use crate::error::{Error, Result};
use crate::manager::{FocusedWindow, WindowHandle, WindowManager};

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
/// [`AX_MESSAGING_TIMEOUT_SECS`]. Fails closed: if the call fails the element
/// keeps the (unbounded) global default, and a wedged target app could then
/// block the caller — a command, or a drag worker whose stall backs up into
/// the gesture tap — for as long as it likes. The caller must therefore not
/// use the element at all; the error is returned so the operation is aborted
/// and reported instead of proceeding unbounded.
///
/// # Safety
/// `element` must be a valid `AXUIElementRef`.
unsafe fn set_messaging_timeout(element: CFTypeRef) -> Result<()> {
    let err = unsafe { AXUIElementSetMessagingTimeout(element, AX_MESSAGING_TIMEOUT_SECS) };
    if err != kAXErrorSuccess {
        tracing::warn!(
            error = err,
            "could not bound AX messaging timeout; refusing to use the element unbounded"
        );
        return Err(Error::Ax(err));
    }
    Ok(())
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

/// `kAXErrorAttributeUnsupported`: the element has no such attribute at all —
/// e.g. an application that exposes no `AXFocusedWindow`, or a window element
/// with no `AXPosition`. Like [`kAXErrorNoValue`] it means "nothing to act on
/// here", not a failure worth showing the user a raw code for.
const kAXErrorAttributeUnsupported: AXError = -25205;

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

/// Map an `AXError` from [`copy_attr`] to this crate's [`Error`].
/// `kAXErrorNoValue` and `kAXErrorAttributeUnsupported` both mean the thing
/// asked for is not there, so both keep the "nothing to act on" behavior
/// (`NoFocusedWindow`) and reach the user as that message rather than as a bare
/// error code. Every other code — including transient failures like
/// `kAXErrorCannotComplete` — is preserved as [`Error::Ax`] so
/// [`Error::window_gone`] and [`Error::retryable`] can tell a hung app apart
/// from one that has truly gone away.
fn map_attr_err(err: AXError) -> Error {
    if err == kAXErrorNoValue || err == kAXErrorAttributeUnsupported {
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

/// Whether a pointer gesture may target a window with this owner.
///
/// A missing PID is rejected as well as this process: pointer-driven frame
/// writes run off the main thread, and applying one to Tomari's own `NSWindow`
/// triggers AppKit's main-thread precondition. Requiring a known external PID
/// keeps both drag-to-snap and drag-to-move on their safe cross-process path.
fn is_external_window_pid(owner_pid: Option<pid_t>, own_pid: pid_t) -> bool {
    owner_pid.is_some_and(|pid| pid != own_pid)
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

/// Read a Core Graphics bounds dictionary as the crate's plain rectangle type.
fn dict_get_rect(dict: CFTypeRef, key: CFStringRef) -> Option<Rect> {
    let mut value: *const c_void = std::ptr::null();
    let found =
        unsafe { CFDictionaryGetValueIfPresent(dict as CFDictionaryRef, key.cast(), &mut value) };
    if found == 0 || value.is_null() {
        return None;
    }
    let value = value as CFTypeRef;
    if unsafe { CFGetTypeID(value) } != unsafe { CFDictionaryGetTypeID() } {
        return None;
    }
    let bounds = unsafe { CFDictionary::wrap_under_get_rule(value as CFDictionaryRef) };
    let rect = CGRect::from_dict_representation(&bounds)?;
    Some(Rect::new(
        rect.origin.x,
        rect.origin.y,
        rect.size.width,
        rect.size.height,
    ))
}

#[derive(Debug, Clone, Copy)]
struct CgWindowInfo {
    owner_pid: pid_t,
    bounds: Rect,
}

/// How many distinct surface owners at a point are worth AX hit-testing before
/// giving up. One is the common case; the extras cover the system chrome that
/// can sit in front of every window (see [`pointer_window_owners`]). Bounded so
/// a point stacked deep in surfaces cannot turn one gesture into a long run of
/// Accessibility round-trips.
const POINTER_OWNER_CANDIDATES: usize = 8;

/// Resolve the owners of the Window Server surfaces at a point, front to back.
///
/// The list is already ordered front-to-back, and every layer is kept: floating
/// app windows are valid drag targets, while menu bars, popovers and other
/// surfaces must block unrelated windows underneath.
///
/// The frontmost surface is not necessarily the answer, though, which is why
/// this returns *candidates* and leaves the choice to the app-scoped AX
/// hit-test, which only looks behind a candidate that has nothing at the point
/// at all (see [`PointerHit`]). macOS keeps full-screen surfaces of its own in front of every app
/// window — the Dock owns one covering the entire display (wallpaper / Stage
/// Manager), the Window Server owns its chrome — and neither is flagged as a
/// desktop element, so the window list does not exclude them. Answering with
/// the frontmost owner alone therefore returned "the Dock" for *every* point on
/// screen, and since the Dock has no accessible element there, both pointer
/// gestures resolved nothing at all: drag-to-snap never armed and drag-to-move
/// never found a window. A surface with no Accessibility presence at the point
/// has to be transparent to the search.
///
/// Duplicate owners are collapsed: one application with several stacked windows
/// at the point is one candidate, hit-tested once.
fn pointer_window_owners(windows: &[CgWindowInfo], x: f64, y: f64) -> Vec<pid_t> {
    let mut owners: Vec<pid_t> = Vec::new();
    for window in windows
        .iter()
        .filter(|window| rect_contains(window.bounds, x, y))
    {
        if owners.len() >= POINTER_OWNER_CANDIDATES {
            break;
        }
        if !owners.contains(&window.owner_pid) {
            owners.push(window.owner_pid);
        }
    }
    owners
}

/// Snapshot enough Window Server metadata to choose the processes to AX
/// hit-test.
///
/// This path deliberately uses no Accessibility objects. In particular, it is
/// safe to call from an event-tap thread while Tomari's main thread is handling
/// its own AppKit controls.
fn pointer_window_owners_at_point(x: f64, y: f64) -> Vec<pid_t> {
    let list = unsafe {
        CGWindowListCopyWindowInfo(
            kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
            kCGNullWindowID,
        )
    };
    if list.is_null() {
        return Vec::new();
    }
    let list = CFOwned(list as CFTypeRef);
    let count = unsafe { CFArrayGetCount(list.0 as _) };
    let mut windows = Vec::with_capacity(count as usize);
    for i in 0..count {
        let entry = unsafe { CFArrayGetValueAtIndex(list.0 as _, i) } as CFTypeRef;
        if entry.is_null() {
            continue;
        }
        let Some(owner_pid) = dict_get_i32(entry, unsafe { kCGWindowOwnerPID }) else {
            continue;
        };
        let Some(bounds) = dict_get_rect(entry, unsafe { kCGWindowBounds }) else {
            continue;
        };
        windows.push(CgWindowInfo { owner_pid, bounds });
    }
    pointer_window_owners(&windows, x, y)
}

/// How many of the frontmost applications to ask for a focused window before
/// giving up. One is the common case; the extras cover the processes that own a
/// normal-level window without answering for one (see
/// [`frontmost_other_app_pids`]). Bounded so resolving the focused window cannot
/// turn into a long run of Accessibility round-trips.
const FOCUSED_WINDOW_CANDIDATES: usize = 8;

/// The owners of the frontmost normal-level (`kCGWindowLayer == 0`) on-screen
/// windows that are not `exclude_pid`, front to back — the applications visible
/// behind our own frontmost window.
///
/// Candidates rather than one answer, for the same reason as
/// [`pointer_window_owners`]: owning a window on screen does not mean answering
/// `AXFocusedWindow` for it. A process whose windows are not exposed through
/// Accessibility used to make the whole lookup fail with its raw AX error, which
/// surfaced in the UI as an unexplained code even though a perfectly ordinary
/// application sat right behind it.
///
/// Duplicate owners are collapsed: one application with several windows on
/// screen is one candidate, asked once.
fn frontmost_other_app_pids(windows: &[(i32, pid_t)], exclude_pid: pid_t) -> Vec<pid_t> {
    let mut pids: Vec<pid_t> = Vec::new();
    for (_, owner_pid) in windows
        .iter()
        .filter(|(layer, owner_pid)| *layer == 0 && *owner_pid != exclude_pid)
    {
        if pids.len() >= FOCUSED_WINDOW_CANDIDATES {
            break;
        }
        if !pids.contains(owner_pid) {
            pids.push(*owner_pid);
        }
    }
    pids
}

/// Snapshot the on-screen window list (front-to-back, desktop elements excluded)
/// as `(layer, owner pid)` pairs and pick the candidates from it.
fn frontmost_other_app_pids_on_screen(exclude_pid: pid_t) -> Vec<pid_t> {
    let list = unsafe {
        CGWindowListCopyWindowInfo(
            kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
            kCGNullWindowID,
        )
    };
    if list.is_null() {
        return Vec::new();
    }
    let list = CFOwned(list as CFTypeRef);

    let count = unsafe { CFArrayGetCount(list.0 as _) };
    let mut windows = Vec::with_capacity(count as usize);
    for i in 0..count {
        let entry = unsafe { CFArrayGetValueAtIndex(list.0 as _, i) } as CFTypeRef;
        if entry.is_null() {
            continue;
        }
        let Some(layer) = dict_get_i32(entry, unsafe { kCGWindowLayer }) else {
            continue;
        };
        let Some(owner_pid) = dict_get_i32(entry, unsafe { kCGWindowOwnerPID }) else {
            continue;
        };
        windows.push((layer, owner_pid));
    }
    frontmost_other_app_pids(&windows, exclude_pid)
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
    // If it cannot be set, no read below is made (see `set_messaging_timeout`).
    unsafe { set_messaging_timeout(system.0) }?;
    let app = unsafe { copy_attr(system.0, "AXFocusedApplication") }.map_err(map_attr_err)?;

    let own_pid = std::process::id() as pid_t;
    if unsafe { element_pid(app.0) } == Some(own_pid) {
        // Ask the applications behind us in turn: the frontmost of them may own
        // a window without exposing one through Accessibility, and the one
        // behind it is then the honest answer.
        let mut retryable = None;
        for other_pid in frontmost_other_app_pids_on_screen(own_pid) {
            let other_app = unsafe { AXUIElementCreateApplication(other_pid) };
            if other_app.is_null() {
                continue;
            }
            let other_app = CFOwned(other_app);
            // Covered by the process-global timeout set on `system` above.
            match unsafe { copy_attr(other_app.0, "AXFocusedWindow") } {
                Ok(window) => return Ok((system, other_app, window)),
                Err(err) => {
                    // An application that did not answer in time is worth
                    // retrying at a higher level, so keep the first such error
                    // in case nothing behind it answers either — reporting
                    // "nothing to act on" would lose that retry.
                    let error = map_attr_err(err);
                    if error.retryable() && retryable.is_none() {
                        retryable = Some(error);
                    }
                }
            }
        }
        return Err(retryable.unwrap_or(Error::NoFocusedWindow));
    }

    let window = unsafe { copy_attr(app.0, "AXFocusedWindow") }.map_err(map_attr_err)?;
    Ok((system, app, window))
}

/// Resolve the stable bundle identifier and localized presentation name for an
/// Accessibility application element. Applications without a bundle cannot
/// participate in remembered placement because a process ID is not durable.
///
/// # Safety
/// `application` must be a valid application `AXUIElementRef`.
unsafe fn window_application(application: CFTypeRef) -> Result<WindowApplication> {
    let pid = unsafe { element_pid(application) }.ok_or(Error::Unsupported)?;
    let running = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        .ok_or(Error::Unsupported)?;
    let bundle_id = running
        .bundleIdentifier()
        .map(|value| value.to_string())
        .filter(|value| !value.is_empty())
        .ok_or(Error::Unsupported)?;
    let name = running
        .localizedName()
        .map(|value| value.to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| bundle_id.clone());
    Ok(WindowApplication { bundle_id, name })
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

    fn focused_window_context(&self) -> Result<FocusedWindow> {
        unsafe {
            if AXIsProcessTrusted() == 0 {
                return Err(Error::PermissionDenied);
            }
            // The window element is +1-retained, so it stays valid on its own
            // after the system-wide and application elements are released.
            // This holds in the fallback path too: `_app` there is a fresh
            // `AXUIElementCreateApplication` handle for the frontmost other
            // process, and its `AXFocusedWindow` is copied (not borrowed) from it.
            let (_system, app, window) = focused_window()?;
            let application = window_application(app.0)?;
            Ok(FocusedWindow {
                handle: Box::new(DragWindow::new(window)?),
                application,
            })
        }
    }

    fn focused_window(&self) -> Result<Box<dyn WindowHandle>> {
        unsafe {
            if AXIsProcessTrusted() == 0 {
                return Err(Error::PermissionDenied);
            }
            // Ordinary snap/move operations need only the retained window
            // element. Do not resolve `NSRunningApplication` here: some
            // movable GUI processes have no bundle identifier and therefore
            // cannot participate in remembered placement, but their windows
            // must remain usable by handle-only operations.
            let (_system, _app, window) = focused_window()?;
            Ok(Box::new(DragWindow::new(window)?))
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
    /// Wrap an external application's owned AX window element, bounding every
    /// later round-trip to it so a wedged target app cannot block the thread
    /// that drags or measures it.
    ///
    /// Reject this process and unknown owners at the handle boundary. Pointer
    /// gestures mutate windows from worker threads, but AppKit traps if such an
    /// AX write resolves directly to Tomari's own `NSWindow`.
    fn new(window: CFOwned) -> Result<Self> {
        let own_pid = std::process::id() as pid_t;
        let owner_pid = unsafe { element_pid(window.0) };
        if !is_external_window_pid(owner_pid, own_pid) {
            return Err(Error::NoFocusedWindow);
        }
        unsafe { set_messaging_timeout(window.0) }?;
        Ok(Self { window })
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
/// Tomari's own windows are deliberately excluded: pointer gestures run on
/// worker threads, and even a read-only system-wide AX hit-test can synchronously
/// enter Tomari's AppKit accessibility implementation off the main thread.
/// Resolve external owners with Window Server metadata first, then constrain
/// each AX hit-test to one application's element so it cannot route back into
/// Tomari even if the window ordering changes between the two operations.
///
/// The surfaces at a point are tried front to back (see
/// [`pointer_window_owners`]): one with no accessible element there — macOS's
/// own full-screen Dock and Window Server chrome — is transparent to the search
/// rather than a wall in front of the app window beneath it. Tomari's own
/// surface is the exception: finding it in front stops the search, because a
/// gesture over our own window is not for whatever it covers.
pub fn window_at_point(x: f64, y: f64) -> Result<DragWindow> {
    unsafe {
        if AXIsProcessTrusted() == 0 {
            return Err(Error::PermissionDenied);
        }

        let own_pid = std::process::id() as pid_t;
        let owners = pointer_window_owners_at_point(x, y);
        if owners.is_empty() {
            return Err(Error::NoFocusedWindow);
        }

        // A timeout set on the system-wide object applies globally to this AX
        // client process. We never hit-test that object: creating it only keeps
        // the app-scoped hits and every returned child/parent element bounded.
        let system = AXUIElementCreateSystemWide();
        if system.is_null() {
            return Err(Error::NoFocusedWindow);
        }
        let system = CFOwned(system);
        // No hit-test proceeds unbounded (see `set_messaging_timeout`).
        set_messaging_timeout(system.0)?;

        for owner_pid in owners {
            if !is_external_window_pid(Some(owner_pid), own_pid) {
                return Err(Error::NoFocusedWindow);
            }
            match window_in_app_at_point(owner_pid, x, y) {
                PointerHit::Window(window) => return DragWindow::new(window),
                // A real surface with nothing draggable behind it as far as this
                // gesture is concerned: a menu, the menu bar, a status item.
                PointerHit::Blocked => return Err(Error::NoFocusedWindow),
                // Nothing there at all; keep looking behind it.
                PointerHit::NoHit => continue,
            }
        }
        Err(Error::NoFocusedWindow)
    }
}

/// How far up the `AXParent` chain to look for the window owning a hit element.
/// Deep view hierarchies are common; an unbounded walk is not, since a cycle or
/// a pathological hierarchy would spin an AX round-trip per step.
const PARENT_WALK_LIMIT: usize = 32;

/// What one application had at a hit-tested point.
///
/// The distinction between the two failures is what keeps
/// [`pointer_window_owners`]'s candidate walk honest: only a surface that is not
/// *there* may be looked behind.
enum PointerHit {
    /// The window owning whatever was hit.
    Window(CFOwned),
    /// The application answered with a real element that belongs to no window —
    /// a menu, the menu bar, a status item. It is a surface the user can see and
    /// it must block whatever is behind it, exactly as before this walk existed.
    Blocked,
    /// The application has nothing at that point: the hit-test failed, returned
    /// nothing, or answered with the application element itself (which is how
    /// the Dock's full-screen wallpaper surface answers — it identifies no
    /// element there, so it is not really "there").
    NoHit,
}

/// AX hit-test one application at a screen point and walk up to the window that
/// owns whatever was hit.
///
/// # Safety
/// The caller must have bounded this process's AX messaging timeout (see
/// [`set_messaging_timeout`]) before calling this, or a wedged application can
/// block the calling thread indefinitely.
unsafe fn window_in_app_at_point(owner_pid: pid_t, x: f64, y: f64) -> PointerHit {
    let application = unsafe { AXUIElementCreateApplication(owner_pid) };
    if application.is_null() {
        return PointerHit::NoHit;
    }
    let application = CFOwned(application);

    let mut hit: CFTypeRef = std::ptr::null();
    let err =
        unsafe { AXUIElementCopyElementAtPosition(application.0, x as f32, y as f32, &mut hit) };
    if err != kAXErrorSuccess || hit.is_null() {
        return PointerHit::NoHit;
    }
    let mut element = CFOwned(hit);

    // "The application itself" is not an element at the point; anything else is.
    let mut hit_a_real_element = false;
    for _ in 0..PARENT_WALK_LIMIT {
        let role = unsafe { element_role(element.0) };
        if role.as_deref() == Some("AXWindow") {
            return PointerHit::Window(element);
        }
        hit_a_real_element |= role.as_deref() != Some("AXApplication");
        if let Ok(window) = unsafe { copy_attr(element.0, "AXWindow") } {
            return PointerHit::Window(window);
        }
        match unsafe { copy_attr(element.0, "AXParent") } {
            Ok(parent) => element = parent,
            Err(_) => break,
        }
    }
    if hit_a_real_element {
        PointerHit::Blocked
    } else {
        PointerHit::NoHit
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
    fn pointer_gestures_require_a_known_external_window_owner() {
        let own_pid = 42;
        assert!(!is_external_window_pid(None, own_pid));
        assert!(!is_external_window_pid(Some(own_pid), own_pid));
        assert!(is_external_window_pid(Some(own_pid + 1), own_pid));
    }

    #[test]
    fn pointer_hit_keeps_a_self_window_in_front_of_external_windows() {
        let own_pid = 42;
        let windows = [
            CgWindowInfo {
                owner_pid: own_pid,
                bounds: Rect::new(100.0, 100.0, 500.0, 400.0),
            },
            CgWindowInfo {
                owner_pid: 99,
                bounds: Rect::new(0.0, 0.0, 1_000.0, 800.0),
            },
        ];

        let owners = pointer_window_owners(&windows, 200.0, 200.0);
        assert_eq!(owners, vec![own_pid, 99]);
        // The search stops at our own surface rather than looking behind it.
        assert!(!is_external_window_pid(owners.first().copied(), own_pid));
    }

    #[test]
    fn pointer_hit_preserves_floating_surfaces_and_their_order() {
        let windows = [
            CgWindowInfo {
                owner_pid: 7,
                bounds: Rect::new(0.0, 0.0, 1_000.0, 40.0),
            },
            CgWindowInfo {
                owner_pid: 99,
                bounds: Rect::new(0.0, 0.0, 1_000.0, 800.0),
            },
        ];

        assert_eq!(pointer_window_owners(&windows, 500.0, 20.0), vec![7, 99]);
        assert_eq!(pointer_window_owners(&windows, 500.0, 200.0), vec![99]);
    }

    #[test]
    fn pointer_hit_looks_behind_a_full_screen_system_surface() {
        // What macOS actually puts on screen: the Dock owns a window covering
        // the whole display in front of every app window. It has no accessible
        // element at the pointer, so the app behind it has to stay reachable.
        let dock_pid = 1_827;
        let windows = [
            CgWindowInfo {
                owner_pid: dock_pid,
                bounds: Rect::new(0.0, 0.0, 1_800.0, 1_169.0),
            },
            CgWindowInfo {
                owner_pid: 99,
                bounds: Rect::new(0.0, 39.0, 1_800.0, 1_040.0),
            },
        ];

        assert_eq!(
            pointer_window_owners(&windows, 500.0, 500.0),
            vec![dock_pid, 99]
        );
    }

    #[test]
    fn pointer_hit_lists_each_owner_once_and_stops_at_the_cap() {
        // An application with several windows stacked at the point is one
        // candidate, and a deep stack cannot grow the AX round-trips without
        // bound.
        let stacked: Vec<CgWindowInfo> = (0..POINTER_OWNER_CANDIDATES + 4)
            .map(|i| CgWindowInfo {
                owner_pid: i as pid_t + 1,
                bounds: Rect::new(0.0, 0.0, 100.0, 100.0),
            })
            .chain(std::iter::repeat_n(
                CgWindowInfo {
                    owner_pid: 1,
                    bounds: Rect::new(0.0, 0.0, 100.0, 100.0),
                },
                3,
            ))
            .collect();

        let owners = pointer_window_owners(&stacked, 10.0, 10.0);
        assert_eq!(owners.len(), POINTER_OWNER_CANDIDATES);
        assert_eq!(owners[0], 1);
        assert_eq!(owners.iter().filter(|pid| **pid == 1).count(), 1);
    }

    #[test]
    fn nothing_there_reads_as_no_window_rather_than_a_raw_code() {
        // Both "no such attribute" and "no value" mean there is nothing to act
        // on, and the UI has a sentence for that. Anything else keeps its code,
        // so a hung app stays retryable.
        assert!(matches!(
            map_attr_err(kAXErrorAttributeUnsupported),
            Error::NoFocusedWindow
        ));
        assert!(matches!(
            map_attr_err(kAXErrorNoValue),
            Error::NoFocusedWindow
        ));
        assert!(matches!(map_attr_err(-25204), Error::Ax(-25204)));
        assert!(map_attr_err(-25204).retryable());
    }

    #[test]
    fn focused_window_candidates_are_the_normal_level_windows_behind_us() {
        let own_pid = 42;
        let windows = [
            (25, 1_184), // Control Center status item
            (24, 605),   // the menu bar
            (20, 1_827), // the Dock's full-screen surface
            (0, own_pid),
            (0, 1_796),
            (0, 1_830),
            (0, 1_796), // the same application again
        ];

        assert_eq!(
            frontmost_other_app_pids(&windows, own_pid),
            vec![1_796, 1_830]
        );
    }

    #[test]
    fn focused_window_candidates_stop_at_the_cap() {
        let stacked: Vec<(i32, pid_t)> = (0..FOCUSED_WINDOW_CANDIDATES + 4)
            .map(|i| (0, i as pid_t + 1))
            .collect();

        assert_eq!(
            frontmost_other_app_pids(&stacked, 0).len(),
            FOCUSED_WINDOW_CANDIDATES
        );
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
