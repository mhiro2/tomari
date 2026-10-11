//! Quartz display configuration and optional native Apple brightness control.
//!
//! Mode arrays own their elements. Inventory only borrows them; configuration
//! explicitly retains selected modes before releasing their owning arrays.

use std::collections::HashMap;
use std::ffi::{c_int, c_void};
use std::ptr;
use std::sync::{Mutex, OnceLock};

use crate::locks::MutexExt;
use objc2::MainThreadMarker;
use objc2_app_kit::NSScreen;
use objc2_foundation::{NSNumber, ns_string};

use core_foundation::array::CFArray;
use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use core_graphics::display::{self as cg, CGConfigureOption, CGDisplay};

use super::configuration::DisplaySetting;
use super::model::*;

static DISPLAY_NAMES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

pub(super) fn screen_identity(screen: &NSScreen) -> Option<String> {
    let number = screen
        .deviceDescription()
        .objectForKey(ns_string!("NSScreenNumber"))?;
    let number = number.downcast_ref::<NSNumber>()?;
    identity(number.unsignedIntValue()).ok()
}

pub fn refresh_names() {
    let Some(main) = MainThreadMarker::new() else {
        return;
    };
    let mut names = HashMap::new();
    for screen in NSScreen::screens(main) {
        if let Some(id) = screen_identity(&screen) {
            names.insert(id, screen.localizedName().to_string());
        }
    }
    *DISPLAY_NAMES.get_or_init(Mutex::default).lock_safe() = names;
}

#[derive(Default)]
pub struct NativeDisplayBackend;

type ModeRef = *mut core_graphics::sys::CGDisplayMode;

struct OwnedMode(ModeRef);
impl Drop for OwnedMode {
    fn drop(&mut self) {
        unsafe { cg::CGDisplayModeRelease(self.0) };
    }
}

fn requested_mode(display: u32, mode_id: i32) -> Result<OwnedMode> {
    let current = unsafe { cg::CGDisplayCopyDisplayMode(display) };
    if !current.is_null() {
        let current = OwnedMode(current);
        if mode_info(current.0).id == mode_id {
            return Ok(current);
        }
    }
    let array = modes(display)?;
    let pointer = array
        .iter()
        .map(|pointer| *pointer as ModeRef)
        .find(|pointer| !pointer.is_null() && usable(*pointer) && mode_info(*pointer).id == mode_id)
        .ok_or_else(|| {
            DisplayError::new(
                DisplayErrorCode::ModeUnavailable,
                "the requested display mode is no longer available",
            )
        })?;
    // Retain a borrowed array element before dropping its owning array.
    unsafe { cg::CGDisplayModeRetain(pointer) };
    Ok(OwnedMode(pointer))
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGDisplayCreateUUIDFromDisplayID(display: u32) -> *const c_void;
    static kCGDisplayShowDuplicateLowResolutionModes: core_foundation::string::CFStringRef;
    fn CGGetOnlineDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFUUIDCreateString(
        allocator: *const c_void,
        uuid: *const c_void,
    ) -> core_foundation::string::CFStringRef;
}

fn system_error(operation: &str, code: i32) -> DisplayError {
    DisplayError::new(
        DisplayErrorCode::System,
        format!("{operation} failed ({code})"),
    )
}

fn online() -> Result<Vec<u32>> {
    // The cap prevents a malformed native count from controlling allocation.
    let mut ids = vec![0; 128];
    let mut count = 0;
    let code = unsafe { CGGetOnlineDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) };
    if code != 0 || count as usize > ids.len() {
        return Err(system_error("display enumeration", code));
    }
    ids.truncate(count as usize);
    Ok(ids)
}

fn identity(id: u32) -> Result<String> {
    let uuid = unsafe { CGDisplayCreateUUIDFromDisplayID(id) };
    if uuid.is_null() {
        return Err(DisplayError::new(
            DisplayErrorCode::Unavailable,
            "display identity is unavailable",
        ));
    }
    let uuid = unsafe { CFType::wrap_under_create_rule(uuid) };
    let string = unsafe { CFUUIDCreateString(ptr::null(), uuid.as_CFTypeRef()) };
    if string.is_null() {
        return Err(DisplayError::new(
            DisplayErrorCode::Unavailable,
            "display identity could not be read",
        ));
    }
    Ok(unsafe { CFString::wrap_under_create_rule(string) }.to_string())
}

pub(super) fn resolve(id: &str) -> Result<CGDisplay> {
    let mut matches = Vec::new();
    for candidate in online()? {
        if identity(candidate).is_ok_and(|value| value == id) {
            matches.push(candidate);
        }
    }
    unique_display(&matches)
}

fn unique_display(matches: &[u32]) -> Result<CGDisplay> {
    match matches {
        [id] => Ok(CGDisplay::new(*id)),
        [] => Err(DisplayError::new(
            DisplayErrorCode::Disconnected,
            "the display is no longer connected",
        )),
        _ => Err(DisplayError::new(
            DisplayErrorCode::Unavailable,
            "macOS cannot uniquely identify the requested display",
        )),
    }
}

fn mode_info(mode: ModeRef) -> DisplayMode {
    // Every caller holds the owning CFArray or current-mode reference.
    unsafe {
        let width = cg::CGDisplayModeGetWidth(mode) as u32;
        let height = cg::CGDisplayModeGetHeight(mode) as u32;
        let pixel_width = cg::CGDisplayModeGetPixelWidth(mode) as u32;
        let pixel_height = cg::CGDisplayModeGetPixelHeight(mode) as u32;
        let rate = cg::CGDisplayModeGetRefreshRate(mode);
        DisplayMode {
            id: cg::CGDisplayModeGetIODisplayModeID(mode),
            width,
            height,
            pixel_width,
            pixel_height,
            refresh_rate: if rate.is_finite() && rate > 0.0 {
                rate
            } else {
                0.0
            },
            hi_dpi: pixel_width > width && pixel_height > height,
        }
    }
}

fn modes(id: u32) -> Result<CFArray<*const c_void>> {
    let options = CFDictionary::from_CFType_pairs(&[(
        unsafe { CFString::wrap_under_get_rule(kCGDisplayShowDuplicateLowResolutionModes) },
        CFBoolean::true_value(),
    )]);
    let array = unsafe { cg::CGDisplayCopyAllDisplayModes(id, options.as_concrete_TypeRef()) };
    if array.is_null() {
        return Err(DisplayError::new(
            DisplayErrorCode::ModeUnavailable,
            "display modes are unavailable",
        ));
    }
    Ok(unsafe { CFArray::wrap_under_create_rule(array) })
}

fn usable(mode: ModeRef) -> bool {
    let flags = unsafe { cg::CGDisplayModeGetIOFlags(mode) };
    flags
        & (cg::kDisplayModeNeverShowFlag
            | cg::kDisplayModeInterlacedFlag
            | cg::kDisplayModeNotGraphicsQualityFlag)
        == 0
        && unsafe { cg::CGDisplayModeGetWidth(mode) > 0 && cg::CGDisplayModeGetHeight(mode) > 0 }
}

impl DisplayBackend for NativeDisplayBackend {
    fn inventory(&mut self) -> Result<Vec<DisplayInfo>> {
        let mut displays = Vec::new();
        for id in online()? {
            let display = CGDisplay::new(id);
            let display_id = identity(id)?;
            let bounds = display.bounds();
            let current = unsafe { cg::CGDisplayCopyDisplayMode(id) };
            let current_mode = if current.is_null() {
                None
            } else {
                let info = mode_info(current);
                unsafe { cg::CGDisplayModeRelease(current) };
                Some(info)
            };
            let mut available = Vec::new();
            if let Ok(array) = modes(id) {
                for pointer in array.iter() {
                    let mode = *pointer as ModeRef;
                    if !mode.is_null() && usable(mode) {
                        available.push(mode_info(mode));
                    }
                }
            }
            if let Some(current) = &current_mode
                && !available.iter().any(|mode| mode.id == current.id)
            {
                available.push(current.clone());
            }
            available.sort_by(|a, b| {
                (a.width, a.height, a.hi_dpi)
                    .cmp(&(b.width, b.height, b.hi_dpi))
                    .then_with(|| a.refresh_rate.total_cmp(&b.refresh_rate))
                    .then(a.id.cmp(&b.id))
            });
            available.dedup_by_key(|mode| mode.id);
            displays.push(DisplayInfo {
                connection_id: id,
                name: DISPLAY_NAMES
                    .get_or_init(Mutex::default)
                    .lock_safe()
                    .get(&display_id)
                    .cloned()
                    .unwrap_or_else(|| {
                        if display.is_builtin() {
                            "Built-in display".into()
                        } else {
                            format!("External display {}", display.unit_number())
                        }
                    }),
                built_in: display.is_builtin(),
                main: display.is_main(),
                mirrored: display.is_in_mirror_set(),
                x: bounds.origin.x as i32,
                y: bounds.origin.y as i32,
                width: bounds.size.width as u32,
                height: bounds.size.height as u32,
                current_mode,
                modes: available,
                brightness: brightness_api().and_then(|api| api.get(id)),
                software_dimming: super::dimming::amount(&display_id),
                id: display_id,
            });
        }
        displays.sort_by(|a, b| {
            b.main
                .cmp(&a.main)
                .then(a.x.cmp(&b.x))
                .then(a.id.cmp(&b.id))
        });
        Ok(displays)
    }

    fn apply_configuration(&mut self, settings: &[DisplaySetting], permanent: bool) -> Result<()> {
        let connected = online()?;
        if connected.len() != settings.len() {
            return Err(DisplayError::new(
                DisplayErrorCode::Disconnected,
                "the connected displays changed before the transaction",
            ));
        }
        let mut prepared = Vec::new();
        for setting in settings {
            let display = resolve(&setting.display_id)?;
            if display.is_in_mirror_set() {
                return Err(DisplayError::new(
                    DisplayErrorCode::Mirrored,
                    "configure mirrored displays in System Settings",
                ));
            }
            let mode = requested_mode(display.id, setting.mode_id)?;
            prepared.push((display, mode, setting));
        }
        let anchor = CGDisplay::main();
        let config = anchor
            .begin_configuration()
            .map_err(|e| system_error("begin display configuration", e))?;
        for (display, mode, setting) in &prepared {
            let code = unsafe {
                cg::CGConfigureDisplayWithDisplayMode(config, display.id, mode.0, ptr::null())
            };
            if code != 0 {
                let _ = anchor.cancel_configuration(&config);
                return Err(system_error("configure display mode", code));
            }
            if let Err(code) = display.configure_display_origin(&config, setting.x, setting.y) {
                let _ = anchor.cancel_configuration(&config);
                return Err(system_error("configure display origin", code));
            }
        }
        // App-only changes are also reverted by WindowServer if Tomari exits
        // or crashes before the user confirms the preview.
        anchor
            .complete_configuration(
                &config,
                if permanent {
                    CGConfigureOption::ConfigurePermanently
                } else {
                    CGConfigureOption::ConfigureForAppOnly
                },
            )
            .map_err(|e| system_error("apply display configuration", e))
    }

    fn set_brightness(&mut self, display_id: &str, value: f64) -> Result<()> {
        let display = resolve(display_id)?;
        let api = brightness_api()
            .filter(|api| api.get(display.id).is_some())
            .ok_or_else(|| {
                DisplayError::new(
                    DisplayErrorCode::BrightnessUnavailable,
                    "native brightness is unavailable",
                )
            })?;
        let code = unsafe { (api.set)(display.id, value as f32) };
        if code == 0 {
            Ok(())
        } else {
            Err(system_error("set display brightness", code))
        }
    }
}

struct BrightnessApi {
    get: unsafe extern "C" fn(u32, *mut f32) -> c_int,
    set: unsafe extern "C" fn(u32, f32) -> c_int,
}

impl BrightnessApi {
    fn get(&self, id: u32) -> Option<f64> {
        let mut value = 0.0;
        let code = unsafe { (self.get)(id, &mut value) };
        (code == 0 && value.is_finite() && (0.0..=1.0).contains(&value)).then_some(value as f64)
    }
}

fn brightness_api() -> Option<&'static BrightnessApi> {
    static API: OnceLock<Option<BrightnessApi>> = OnceLock::new();
    API.get_or_init(|| unsafe {
        // Private native brightness entry points are optional. Keep their
        // library loaded for the process lifetime; never assume availability.
        let library = libc::dlopen(
            c"/System/Library/PrivateFrameworks/DisplayServices.framework/DisplayServices".as_ptr(),
            libc::RTLD_LAZY | libc::RTLD_LOCAL,
        );
        if library.is_null() {
            return None;
        }
        let get = libc::dlsym(library, c"DisplayServicesGetBrightness".as_ptr());
        let set = libc::dlsym(library, c"DisplayServicesSetBrightness".as_ptr());
        if get.is_null() || set.is_null() {
            libc::dlclose(library);
            return None;
        }
        Some(BrightnessApi {
            get: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(u32, *mut f32) -> c_int>(
                get,
            ),
            set: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(u32, f32) -> c_int>(set),
        })
    })
    .as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutation_resolution_never_picks_an_arbitrary_display_with_a_shared_identity() {
        assert_eq!(unique_display(&[42]).unwrap().id, 42);
        assert_eq!(
            unique_display(&[]).unwrap_err().code,
            DisplayErrorCode::Disconnected
        );
        assert_eq!(
            unique_display(&[42, 43]).unwrap_err().code,
            DisplayErrorCode::Unavailable
        );
    }

    fn peak_resident_bytes() -> u64 {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
        assert_eq!(
            unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) },
            0
        );
        // On macOS ru_maxrss is measured in bytes rather than KiB.
        unsafe { usage.assume_init() }.ru_maxrss.max(0) as u64
    }

    #[test]
    #[ignore = "long-running read-only native resource and inventory probe; duration is opt-in"]
    fn read_only_inventory_soak() {
        use std::time::{Duration, Instant};
        let seconds: u64 = std::env::var("TOMARI_DISPLAY_SOAK_SECONDS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(60);
        assert!((10..=21600).contains(&seconds));
        let mut backend = NativeDisplayBackend;
        // Exclude framework loading and initial allocator growth from the baseline.
        for _ in 0..20 {
            backend.inventory().unwrap();
        }
        let baseline = peak_resident_bytes();
        let started = Instant::now();
        let duration = Duration::from_secs(seconds);
        let mut report_at = Duration::ZERO;
        let mut reads = 0_u64;
        let mut errors = 0_u64;
        let mut max_latency = Duration::ZERO;
        while started.elapsed() < duration {
            let before = Instant::now();
            if backend.inventory().is_err() {
                errors += 1;
            }
            reads += 1;
            max_latency = max_latency.max(before.elapsed());
            let elapsed = started.elapsed();
            if elapsed >= report_at {
                println!(
                    "elapsed={}s reads={reads} errors={errors} max_latency={max_latency:?} peak_growth_bytes={}",
                    elapsed.as_secs(),
                    peak_resident_bytes().saturating_sub(baseline)
                );
                report_at = elapsed + Duration::from_secs(60);
            }
            std::thread::sleep(
                Duration::from_secs(1).min(duration.saturating_sub(started.elapsed())),
            );
        }
        let growth = peak_resident_bytes().saturating_sub(baseline);
        println!(
            "final: elapsed={}s reads={reads} errors={errors} max_latency={max_latency:?} peak_growth_bytes={growth}",
            started.elapsed().as_secs()
        );
        assert!(reads > errors, "the inventory never became readable");
        assert!(
            growth < 64 * 1024 * 1024,
            "native inventory retained more than 64 MiB beyond its warm baseline"
        );
    }

    #[test]
    #[ignore = "measures repeated read-only display inventory calls in a graphical session"]
    fn read_only_inventory_latency() {
        let mut backend = NativeDisplayBackend;
        let mut elapsed = Vec::new();
        for _ in 0..20 {
            let start = std::time::Instant::now();
            let displays = backend.inventory().unwrap();
            assert!(!displays.is_empty());
            elapsed.push(start.elapsed());
        }
        elapsed.sort();
        println!(
            "display inventory, 20 reads: min={:?}, median={:?}, max={:?}",
            elapsed[0], elapsed[10], elapsed[19]
        );
    }

    #[test]
    #[ignore = "reads the current macOS display inventory; run explicitly on a graphical session"]
    fn read_only_native_inventory() {
        let displays = NativeDisplayBackend.inventory().unwrap();
        assert!(!displays.is_empty());
        for display in &displays {
            assert!(!display.id.is_empty());
            if let Some(current) = &display.current_mode {
                assert!(current.width > 0 && current.height > 0);
                assert!(display.modes.iter().any(|mode| mode.id == current.id));
                let native = resolve(&display.id).unwrap();
                let retained = requested_mode(native.id, current.id).unwrap();
                assert_eq!(mode_info(retained.0), *current);
                if let Some(other) = display.modes.iter().find(|mode| mode.id != current.id) {
                    let retained = requested_mode(native.id, other.id).unwrap();
                    assert_eq!(mode_info(retained.0), *other);
                }
            }
        }
        println!("{}", serde_json::to_string_pretty(&displays).unwrap());
    }
}
