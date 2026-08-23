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
const kAXErrorAttributeUnsupported: AXError = -25_205;
const kAXErrorNoValue: AXError = -25_212;
const kAXValueTypeCGPoint: AXValueType = 1;
const kAXValueTypeCGSize: AXValueType = 2;
const AX_MESSAGING_TIMEOUT_SECS: f32 = 0.1;
const LABEL_READ_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(5);

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

/// Accessibility strings that may describe one menu bar item. Keeping label
/// policy separate from AX calls makes the incomplete values seen in the wild
/// straightforward to exercise in unit tests.
#[derive(Default)]
struct ItemLabels {
    title: Option<String>,
    description: Option<String>,
    help: Option<String>,
    identifier: Option<String>,
    role_description: Option<String>,
}

struct LabelRead {
    labels: ItemLabels,
    failure: Option<AXError>,
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
        // Control Center advertises disabled modules as zero-area menu-bar
        // items. Their placeholder positions can overlap another display's
        // menu bar, so geometry checks alone would present them as a row of
        // duplicate "Control Center" entries.
        if !usable_item_frame(position, size) {
            continue;
        }
        let center_x = position.x + size.width / 2.0;
        let center_y = position.y + size.height / 2.0;
        // Multiple displays may each advertise menu extras. Only compare items
        // occupying the same active menu bar as Tomari's divider.
        if !is_on_scanned_menu(context, center_x, center_y) {
            continue;
        }

        let Ok(specific_name) = (unsafe { item_label(element, owner) }) else {
            // A transient AX failure is not evidence that the item has no
            // specific name. Omitting it from one best-effort snapshot is less
            // misleading than presenting the owner name for several items.
            continue;
        };
        let name = specific_name.unwrap_or_else(|| owner.name.clone());
        let owner_name = (!same_label(&name, &owner.name)).then(|| owner.name.clone());
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

fn usable_item_frame(position: CGPoint, size: CGSize) -> bool {
    position.x.is_finite()
        && position.y.is_finite()
        && size.width.is_finite()
        && size.height.is_finite()
        && size.width > 0.0
        && size.height > 0.0
}

fn generic_label(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "menu extra" | "menu bar item" | "status item" | "status menu"
    )
}

fn same_label(left: &str, right: &str) -> bool {
    left.trim().eq_ignore_ascii_case(right.trim())
}

fn first_usable_label(
    owner: &Owner,
    role_description: Option<&str>,
    values: impl IntoIterator<Item = Option<String>>,
) -> Option<String> {
    values.into_iter().find_map(|value| {
        let value = value?;
        let trimmed = value.trim();
        let is_owner = same_label(trimmed, &owner.name)
            || owner
                .bundle_id
                .as_deref()
                .is_some_and(|bundle_id| same_label(trimmed, bundle_id));
        let is_role = role_description.is_some_and(|role| same_label(trimmed, role));
        (!trimmed.is_empty() && !generic_label(trimmed) && !is_owner && !is_role)
            .then(|| trimmed.to_owned())
    })
}

fn system_identifier_label(value: &str) -> Option<String> {
    let suffix = value.strip_prefix("com.apple.menuextra.")?;
    let label = match suffix.to_ascii_lowercase().as_str() {
        "airport" | "wifi" => "Wi-Fi".to_owned(),
        "controlcenter" => "Control Center".to_owned(),
        "nowplaying" => "Now Playing".to_owned(),
        "textinput" => "Input Menu".to_owned(),
        "timemachine" => "Time Machine".to_owned(),
        _ => {
            let mut characters = suffix.chars();
            let first = characters.next()?;
            first.to_uppercase().chain(characters).collect()
        }
    };
    Some(label)
}

fn description_label(owner: &Owner, identifier: Option<&str>, value: String) -> String {
    let is_control_center = owner.bundle_id.as_deref() == Some("com.apple.controlcenter");
    if is_control_center && identifier.and_then(system_identifier_label).is_some() {
        return value
            .split_once(',')
            .map_or(value.as_str(), |(label, _)| label)
            .trim()
            .to_owned();
    }
    value.trim().to_owned()
}

fn resolve_label(owner: &Owner, labels: ItemLabels) -> Option<String> {
    let role_description = labels.role_description.as_deref();
    let system_identifier = labels
        .identifier
        .as_deref()
        .and_then(system_identifier_label);
    let description = labels
        .description
        .map(|value| description_label(owner, labels.identifier.as_deref(), value));
    let raw_identifier = if system_identifier.is_none() {
        labels.identifier
    } else {
        None
    };
    first_usable_label(
        owner,
        role_description,
        [
            labels.title,
            description,
            system_identifier,
            labels.help,
            raw_identifier,
        ],
    )
}

unsafe fn item_label(element: CFTypeRef, owner: &Owner) -> Result<Option<String>, AXError> {
    if let Some(label) = unsafe { resolved_element_label(element, owner) }? {
        return Ok(Some(label));
    }

    // Some status-item implementations put the accessible label on a shallow
    // button or image rather than the AXMenuBarItem itself.
    let Some(children) = (unsafe { checked_copy_attr_with_retry(element, "AXChildren") })? else {
        return Ok(None);
    };
    if unsafe { CFGetTypeID(children.0) } != unsafe { CFArrayGetTypeID() } {
        return Ok(None);
    }
    let array = children.0 as CFArrayRef;
    let count = unsafe { CFArrayGetCount(array) };
    for index in 0..count {
        let child = unsafe { CFArrayGetValueAtIndex(array, index) } as CFTypeRef;
        if child.is_null() {
            continue;
        }
        let role = unsafe { checked_string_attr_with_retry(child, "AXRole") }?;
        if !matches!(
            role.as_deref(),
            Some("AXButton" | "AXImage" | "AXStaticText")
        ) {
            continue;
        }
        if let Some(label) = unsafe { resolved_element_label(child, owner) }? {
            return Ok(Some(label));
        }
    }
    Ok(None)
}

unsafe fn resolved_element_label(
    element: CFTypeRef,
    owner: &Owner,
) -> Result<Option<String>, AXError> {
    let first = unsafe { read_labels(element) };
    if let Some(label) = resolve_label(owner, first.labels) {
        return Ok(Some(label));
    }
    if first.failure.is_none() {
        return Ok(None);
    }

    std::thread::sleep(LABEL_READ_RETRY_DELAY);
    let retry = unsafe { read_labels(element) };
    if let Some(label) = resolve_label(owner, retry.labels) {
        return Ok(Some(label));
    }
    retry.failure.map_or(Ok(None), Err)
}

unsafe fn read_labels(element: CFTypeRef) -> LabelRead {
    let mut failure = None;
    let mut read = |name| match unsafe { checked_string_attr(element, name) } {
        Ok(value) => value,
        Err(error) => {
            failure.get_or_insert(error);
            None
        }
    };
    let labels = ItemLabels {
        title: read("AXTitle"),
        description: read("AXDescription"),
        help: read("AXHelp"),
        identifier: read("AXIdentifier"),
        role_description: read("AXRoleDescription"),
    };
    LabelRead { labels, failure }
}

unsafe fn checked_string_attr(element: CFTypeRef, name: &str) -> Result<Option<String>, AXError> {
    let Some(value) = (unsafe { checked_copy_attr(element, name) })? else {
        return Ok(None);
    };
    if unsafe { CFGetTypeID(value.0) } != unsafe { CFStringGetTypeID() } {
        return Ok(None);
    }
    let string = unsafe { CFString::wrap_under_get_rule(value.0 as CFStringRef) };
    Ok(Some(string.to_string()))
}

unsafe fn checked_string_attr_with_retry(
    element: CFTypeRef,
    name: &str,
) -> Result<Option<String>, AXError> {
    match unsafe { checked_string_attr(element, name) } {
        Ok(value) => Ok(value),
        Err(_) => {
            std::thread::sleep(LABEL_READ_RETRY_DELAY);
            unsafe { checked_string_attr(element, name) }
        }
    }
}

unsafe fn checked_copy_attr_with_retry(
    element: CFTypeRef,
    name: &str,
) -> Result<Option<CFOwned>, AXError> {
    match unsafe { checked_copy_attr(element, name) } {
        Ok(value) => Ok(value),
        Err(_) => {
            std::thread::sleep(LABEL_READ_RETRY_DELAY);
            unsafe { checked_copy_attr(element, name) }
        }
    }
}

unsafe fn checked_copy_attr(element: CFTypeRef, name: &str) -> Result<Option<CFOwned>, AXError> {
    let attribute = CFString::new(name);
    let mut value: CFTypeRef = std::ptr::null();
    let error = unsafe {
        AXUIElementCopyAttributeValue(element, attribute.as_concrete_TypeRef(), &mut value)
    };
    let value = (!value.is_null()).then(|| CFOwned(value));
    if missing_attribute_error(error) {
        return Ok(None);
    }
    if error != kAXErrorSuccess {
        return Err(error);
    }
    Ok(value)
}

fn missing_attribute_error(error: AXError) -> bool {
    matches!(error, kAXErrorAttributeUnsupported | kAXErrorNoValue)
}

/// Copy a retained AX attribute. Missing and unsupported attributes are normal
/// during a best-effort scan and therefore collapse to `None`.
unsafe fn copy_attr(element: CFTypeRef, name: &str) -> Option<CFOwned> {
    unsafe { checked_copy_attr(element, name) }.ok().flatten()
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

    fn control_center() -> Owner {
        Owner {
            pid: 1,
            name: "Control Center".into(),
            bundle_id: Some("com.apple.controlcenter".into()),
        }
    }

    fn third_party_owner() -> Owner {
        Owner {
            pid: 2,
            name: "Example App".into(),
            bundle_id: Some("com.example.app".into()),
        }
    }

    #[test]
    fn recognizes_only_generic_accessibility_labels() {
        assert!(generic_label("Menu Extra"));
        assert!(generic_label(" status item "));
        assert!(generic_label("status menu"));
        assert!(!generic_label("Wi-Fi"));
    }

    #[test]
    fn skips_empty_generic_and_owner_labels() {
        let owner = control_center();
        assert_eq!(
            first_usable_label(
                &owner,
                Some("status menu"),
                [
                    Some(String::new()),
                    Some("status menu".into()),
                    Some("Control Center".into()),
                    Some("Wi-Fi".into()),
                ],
            ),
            Some("Wi-Fi".into())
        );
    }

    #[test]
    fn resolves_real_control_center_labels_without_dynamic_details() {
        let owner = control_center();

        assert_eq!(
            resolve_label(
                &owner,
                ItemLabels {
                    description: Some("Wi-Fi, connected, 3 bars".into()),
                    identifier: Some("com.apple.menuextra.wifi".into()),
                    ..Default::default()
                },
            ),
            Some("Wi-Fi".into())
        );
        assert_eq!(
            resolve_label(
                &owner,
                ItemLabels {
                    title: Some("Battery".into()),
                    description: Some("Control Center".into()),
                    ..Default::default()
                },
            ),
            Some("Battery".into())
        );
    }

    #[test]
    fn uses_the_menu_extra_identifier_after_an_owner_only_description() {
        assert_eq!(
            resolve_label(
                &control_center(),
                ItemLabels {
                    description: Some("Control Center".into()),
                    help: Some("Click to open Bluetooth settings".into()),
                    identifier: Some("com.apple.menuextra.bluetooth".into()),
                    ..Default::default()
                },
            ),
            Some("Bluetooth".into())
        );
    }

    #[test]
    fn preserves_third_party_descriptions_and_identifier_fallbacks() {
        let owner = third_party_owner();
        assert_eq!(
            resolve_label(
                &owner,
                ItemLabels {
                    description: Some("Song, Live".into()),
                    ..Default::default()
                },
            ),
            Some("Song, Live".into())
        );
        assert_eq!(
            resolve_label(
                &owner,
                ItemLabels {
                    identifier: Some("primary-status-item".into()),
                    ..Default::default()
                },
            ),
            Some("primary-status-item".into())
        );
    }

    #[test]
    fn distinguishes_missing_attributes_from_transient_failures() {
        assert!(missing_attribute_error(kAXErrorAttributeUnsupported));
        assert!(missing_attribute_error(kAXErrorNoValue));
        // kAXErrorCannotComplete is a transient messaging failure.
        assert!(!missing_attribute_error(-25_204));
    }

    #[test]
    fn leaves_owner_fallback_to_the_caller_when_no_specific_label_exists() {
        assert_eq!(
            resolve_label(
                &control_center(),
                ItemLabels {
                    description: Some("Control Center".into()),
                    identifier: Some("com.apple.menuextra.controlcenter".into()),
                    role_description: Some("status menu".into()),
                    ..Default::default()
                },
            ),
            None
        );
    }

    #[test]
    fn rejects_zero_area_and_non_finite_item_frames() {
        assert!(usable_item_frame(
            CGPoint { x: 10.0, y: 8.0 },
            CGSize {
                width: 22.0,
                height: 22.0,
            },
        ));
        assert!(!usable_item_frame(
            CGPoint { x: 0.0, y: 1_169.0 },
            CGSize {
                width: 0.0,
                height: 0.0,
            },
        ));
        assert!(!usable_item_frame(
            CGPoint {
                x: f64::NAN,
                y: 8.0,
            },
            CGSize {
                width: 22.0,
                height: 22.0,
            },
        ));
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
