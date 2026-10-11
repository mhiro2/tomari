//! Session-only software dimming with click-through AppKit panels.
//!
//! Panels belong exclusively to the main thread. The small numeric snapshot
//! allows background inventory reads without sending AppKit objects across
//! threads. Nothing is persisted or installed in the display's gamma tables.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSPanel, NSScreen, NSStatusWindowLevel};

use super::model::*;
use super::native;
use crate::locks::MutexExt;

struct Shade {
    panel: Retained<NSPanel>,
    amount: f64,
}
thread_local! {
    static SHADES: RefCell<HashMap<String, Shade>> = RefCell::new(HashMap::new());
}
static AMOUNTS: OnceLock<Mutex<HashMap<String, f64>>> = OnceLock::new();

pub fn amount(id: &str) -> f64 {
    AMOUNTS
        .get_or_init(Mutex::default)
        .lock_safe()
        .get(id)
        .copied()
        .unwrap_or(0.0)
}

pub fn any_active() -> bool {
    !AMOUNTS.get_or_init(Mutex::default).lock_safe().is_empty()
}

pub fn validate(amount: f64) -> Result<()> {
    if !amount.is_finite() || !(0.0..=0.8).contains(&amount) {
        return Err(DisplayError::new(
            DisplayErrorCode::InvalidDimming,
            "software dimming must be between zero and eighty percent",
        ));
    }
    Ok(())
}

pub fn apply(id: &str, amount: f64) -> Result<()> {
    validate(amount)?;
    let main = MainThreadMarker::new().ok_or_else(|| {
        DisplayError::new(
            DisplayErrorCode::System,
            "software dimming requires the main thread",
        )
    })?;
    if amount == 0.0 {
        SHADES.with(|shades| {
            if let Some(shade) = shades.borrow_mut().remove(id) {
                shade.panel.orderOut(None);
            }
        });
        AMOUNTS.get_or_init(Mutex::default).lock_safe().remove(id);
        return Ok(());
    }
    let display = native::resolve(id)?;
    if display.is_in_mirror_set() {
        return Err(DisplayError::new(
            DisplayErrorCode::Mirrored,
            "mirrored displays share the same desktop image",
        ));
    }
    let screen = NSScreen::screens(main)
        .into_iter()
        .find(|screen| native::screen_identity(screen).as_deref() == Some(id))
        .ok_or_else(|| {
            DisplayError::new(
                DisplayErrorCode::Disconnected,
                "the display is no longer available",
            )
        })?;
    SHADES.with(|shades| {
        let mut shades = shades.borrow_mut();
        let shade = shades.entry(id.to_string()).or_insert_with(|| {
            let panel = crate::overlay::passive_panel(main, NSStatusWindowLevel + 1);
            panel.setBackgroundColor(Some(&NSColor::blackColor()));
            Shade { panel, amount }
        });
        shade.amount = amount;
        shade.panel.setFrame_display(screen.frame(), true);
        shade.panel.setAlphaValue(amount);
        shade.panel.orderFrontRegardless();
    });
    AMOUNTS
        .get_or_init(Mutex::default)
        .lock_safe()
        .insert(id.to_string(), amount);
    Ok(())
}

pub fn refresh() {
    let Some(main) = MainThreadMarker::new() else {
        return;
    };
    let screens: HashMap<_, _> = NSScreen::screens(main)
        .into_iter()
        .filter_map(|screen| native::screen_identity(&screen).map(|id| (id, screen)))
        .collect();
    SHADES.with(|shades| {
        let mut shades = shades.borrow_mut();
        shades.retain(|id, shade| {
            if let Some(screen) = screens.get(id)
                && native::resolve(id).is_ok_and(|display| !display.is_in_mirror_set())
            {
                shade.panel.setFrame_display(screen.frame(), true);
                true
            } else {
                shade.panel.orderOut(None);
                false
            }
        });
        *AMOUNTS.get_or_init(Mutex::default).lock_safe() = shades
            .iter()
            .map(|(id, shade)| (id.clone(), shade.amount))
            .collect();
    });
}

pub fn clear() {
    if MainThreadMarker::new().is_none() {
        return;
    }
    SHADES.with(|shades| {
        for (_, shade) in shades.borrow_mut().drain() {
            shade.panel.orderOut(None);
        }
    });
    AMOUNTS.get_or_init(Mutex::default).lock_safe().clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dimming_is_bounded_so_the_screen_always_remains_visible() {
        for value in [0.0, 0.4, 0.8] {
            assert!(validate(value).is_ok());
        }
        for value in [-0.01, 0.81, 1.0, f64::NAN, f64::INFINITY] {
            assert!(validate(value).is_err());
        }
    }
}
