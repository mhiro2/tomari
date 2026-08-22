//! Best-effort inventory of the menu bar's real status items.
//!
//! AppKit exposes only status items owned by this process. Accessibility fills
//! the read-only gap: each running application's `AXExtrasMenuBar` contains its
//! menu extras, including items currently occluded by a notch. The API does not
//! promise durable identifiers or movement, so this module only snapshots the
//! current layout; macOS' ⌘-drag arrangement remains authoritative.

#![allow(non_upper_case_globals, non_camel_case_types)]

use std::ffi::c_void;

use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use core_foundation_sys::array::{
    CFArrayGetCount, CFArrayGetTypeID, CFArrayGetValueAtIndex, CFArrayRef,
};
use core_foundation_sys::base::{CFGetTypeID, CFRelease, CFTypeRef};
use core_foundation_sys::string::{CFStringGetTypeID, CFStringRef};
use core_graphics::geometry::{CGPoint, CGSize};
use objc2_app_kit::NSWorkspace;

use super::status::ScanContext;
use super::{MenuBarItem, MenuBarItemZone};

type AXError = i32;
type AXValueType = u32;
type pid_t = i32;

const kAXErrorSuccess: AXError = 0;
const kAXValueTypeCGPoint: AXValueType = 1;
const kAXValueTypeCGSize: AXValueType = 2;
const AX_MESSAGING_TIMEOUT_SECS: f32 = 0.1;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateApplication(pid: pid_t) -> CFTypeRef;
    fn AXUIElementCopyAttributeValue(
        element: CFTypeRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementSetMessagingTimeout(element: CFTypeRef, timeout_in_seconds: f32) -> AXError;
    fn AXValueGetValue(value: CFTypeRef, value_type: AXValueType, value_ptr: *mut c_void) -> u8;
}

/// Releases a Create/Copy-returned Core Foundation object.
struct CFOwned(CFTypeRef);

impl Drop for CFOwned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}

/// One running application's stable presentation metadata for this scan.
struct Owner {
    pid: pid_t,
    name: String,
    bundle_id: Option<String>,
}

/// Scan every running application. `AXExtrasMenuBar` is intentionally queried
/// directly instead of walking the full AX tree: the direct attribute is both
/// faster and the form used by modern macOS.
pub fn scan(context: ScanContext) -> Vec<MenuBarItem> {
    let running = NSWorkspace::sharedWorkspace().runningApplications();
    let own_pid = std::process::id() as pid_t;
    let mut items = Vec::new();

    for application in running.iter() {
        let pid = application.processIdentifier();
        if pid <= 0 || pid == own_pid {
            continue;
        }
        let bundle_id = application
            .bundleIdentifier()
            .map(|value| value.to_string())
            .filter(|value| !value.is_empty());
        let name = application
            .localizedName()
            .map(|value| value.to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| bundle_id.clone())
            .unwrap_or_else(|| format!("Process {pid}"));
        let owner = Owner {
            pid,
            name,
            bundle_id,
        };

        let root = unsafe { AXUIElementCreateApplication(pid) };
        if root.is_null() {
            continue;
        }
        let root = CFOwned(root);
        unsafe {
            AXUIElementSetMessagingTimeout(root.0, AX_MESSAGING_TIMEOUT_SECS);
        }

        let modern = unsafe { copy_attr(root.0, "AXExtrasMenuBar") };
        if let Some(extras) = modern {
            collect_children(extras.0, &owner, context, &mut items);
        } else {
            // Older and unusual implementations expose `AXMenuExtra` directly
            // under the application root rather than in AXExtrasMenuBar.
            collect_children(root.0, &owner, context, &mut items);
        }
    }

    items.sort_by(|left, right| {
        zone_order(left.zone)
            .cmp(&zone_order(right.zone))
            .then_with(|| left.position.total_cmp(&right.position))
    });
    items
}

fn zone_order(zone: MenuBarItemZone) -> u8 {
    match zone {
        MenuBarItemZone::Hidden => 0,
        MenuBarItemZone::Visible => 1,
    }
}

fn collect_children(
    parent: CFTypeRef,
    owner: &Owner,
    context: ScanContext,
    output: &mut Vec<MenuBarItem>,
) {
    let Some(children) = (unsafe { copy_attr(parent, "AXChildren") }) else {
        return;
    };
    if unsafe { CFGetTypeID(children.0) } != unsafe { CFArrayGetTypeID() } {
        return;
    }
    let array = children.0 as CFArrayRef;
    let count = unsafe { CFArrayGetCount(array) };
    for index in 0..count {
        let element = unsafe { CFArrayGetValueAtIndex(array, index) } as CFTypeRef;
        if element.is_null() {
            continue;
        }
        let role = unsafe { string_attr(element, "AXRole") };
        if !matches!(role.as_deref(), Some("AXMenuBarItem" | "AXMenuExtra")) {
            continue;
        }
        let Some((position, size)) = (unsafe { element_frame(element) }) else {
            continue;
        };
        let center_x = position.x + size.width / 2.0;
        let center_y = position.y + size.height / 2.0;
        // Multiple displays may each advertise menu extras. Only compare items
        // occupying the same active menu bar as Tomari's divider.
        if !is_on_scanned_menu(context, center_x, center_y) {
            continue;
        }

        // Control Center commonly exposes an empty AXTitle and puts the useful
        // label (Wi-Fi, Battery, Clock, ...) in AXDescription. Reject unusable
        // values *before* choosing one so an empty earlier attribute cannot
        // suppress a useful later one.
        let specific_name = first_usable_label(
            ["AXDescription", "AXTitle", "AXHelp", "AXIdentifier"]
                .into_iter()
                .map(|attribute| unsafe { string_attr(element, attribute) }),
        );
        let name = specific_name.unwrap_or_else(|| owner.name.clone());
        let owner_name = (name != owner.name).then(|| owner.name.clone());
        let zone = zone_for_x(context, center_x);
        let identity = unsafe { string_attr(element, "AXIdentifier") }
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| format!("{center_x:.1}"));
        let owner_identity = owner
            .bundle_id
            .clone()
            .unwrap_or_else(|| owner.pid.to_string());
        let id = format!("{owner_identity}:{identity}:{index}");
        output.push(MenuBarItem {
            id,
            name,
            owner_name,
            bundle_id: owner.bundle_id.clone(),
            zone,
            position: center_x,
        });
    }
}

fn is_on_scanned_menu(context: ScanContext, center_x: f64, center_y: f64) -> bool {
    center_x >= context.screen_left
        && center_x <= context.screen_right
        && center_y >= context.menu_top - 4.0
        && center_y <= context.menu_bottom + 4.0
}

fn zone_for_x(context: ScanContext, center_x: f64) -> MenuBarItemZone {
    if center_x < context.divider_x {
        MenuBarItemZone::Hidden
    } else {
        MenuBarItemZone::Visible
    }
}

fn generic_label(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "menu extra" | "menu bar item" | "status item"
    )
}

fn first_usable_label(values: impl IntoIterator<Item = Option<String>>) -> Option<String> {
    values.into_iter().find_map(|value| {
        let value = value?;
        let trimmed = value.trim();
        (!trimmed.is_empty() && !generic_label(trimmed)).then(|| trimmed.to_owned())
    })
}

/// Copy a retained AX attribute. Missing and unsupported attributes are normal
/// during a best-effort scan and therefore collapse to `None`.
unsafe fn copy_attr(element: CFTypeRef, name: &str) -> Option<CFOwned> {
    let attribute = CFString::new(name);
    let mut value: CFTypeRef = std::ptr::null();
    let error = unsafe {
        AXUIElementCopyAttributeValue(element, attribute.as_concrete_TypeRef(), &mut value)
    };
    (error == kAXErrorSuccess && !value.is_null()).then(|| CFOwned(value))
}

unsafe fn string_attr(element: CFTypeRef, name: &str) -> Option<String> {
    let value = unsafe { copy_attr(element, name) }?;
    if unsafe { CFGetTypeID(value.0) } != unsafe { CFStringGetTypeID() } {
        return None;
    }
    let string = unsafe { CFString::wrap_under_get_rule(value.0 as CFStringRef) };
    Some(string.to_string())
}

unsafe fn element_frame(element: CFTypeRef) -> Option<(CGPoint, CGSize)> {
    let position = unsafe { copy_attr(element, "AXPosition") }?;
    let size = unsafe { copy_attr(element, "AXSize") }?;
    let mut point = CGPoint { x: 0.0, y: 0.0 };
    let mut dimensions = CGSize {
        width: 0.0,
        height: 0.0,
    };
    let got_position = unsafe {
        AXValueGetValue(
            position.0,
            kAXValueTypeCGPoint,
            (&mut point as *mut CGPoint).cast(),
        )
    };
    let got_size = unsafe {
        AXValueGetValue(
            size.0,
            kAXValueTypeCGSize,
            (&mut dimensions as *mut CGSize).cast(),
        )
    };
    (got_position != 0 && got_size != 0).then_some((point, dimensions))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_generic_accessibility_labels() {
        assert!(generic_label("Menu Extra"));
        assert!(generic_label(" status item "));
        assert!(!generic_label("Wi-Fi"));
    }

    #[test]
    fn skips_empty_and_generic_labels_before_using_description() {
        assert_eq!(
            first_usable_label([
                Some(String::new()),
                Some("Menu Bar Item".into()),
                Some("Wi-Fi".into()),
            ]),
            Some("Wi-Fi".into())
        );
    }

    #[test]
    fn orders_hidden_items_before_visible_items() {
        assert!(zone_order(MenuBarItemZone::Hidden) < zone_order(MenuBarItemZone::Visible));
    }

    #[test]
    fn classifies_items_around_the_divider() {
        let context = ScanContext {
            divider_x: 800.0,
            screen_left: 0.0,
            screen_right: 1_200.0,
            menu_top: 0.0,
            menu_bottom: 24.0,
        };

        assert_eq!(zone_for_x(context, 799.0), MenuBarItemZone::Hidden);
        assert_eq!(zone_for_x(context, 800.0), MenuBarItemZone::Visible);
        assert!(is_on_scanned_menu(context, 1_100.0, 12.0));
        assert!(!is_on_scanned_menu(context, 1_300.0, 12.0));
        assert!(!is_on_scanned_menu(context, 1_100.0, 50.0));
    }
}
