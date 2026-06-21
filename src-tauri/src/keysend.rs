//! macOS keystroke synthesis via Core Graphics events, used to realize the
//! `SwitchIme` and `SendKeystroke` actions. Posting key events requires the
//! Accessibility permission (the same one window management uses).

use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, EventField};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use tomari_core::ImeMode;
use tomari_keyboard::accelerator;

use crate::eventtap::SYNTHETIC_MARKER;

fn post(keycode: u16, flags: CGEventFlags) -> Result<(), String> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|()| "failed to create CGEventSource".to_string())?;
    let down = CGEvent::new_keyboard_event(source.clone(), keycode, true)
        .map_err(|()| "failed to create key-down event".to_string())?;
    down.set_flags(flags);
    // Tag synthesized events so our own event tap ignores them (no feedback).
    down.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, SYNTHETIC_MARKER);
    down.post(CGEventTapLocation::HID);

    let up = CGEvent::new_keyboard_event(source, keycode, false)
        .map_err(|()| "failed to create key-up event".to_string())?;
    up.set_flags(flags);
    up.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, SYNTHETIC_MARKER);
    up.post(CGEventTapLocation::HID);
    Ok(())
}

/// Switch the input method by posting the JIS 英数 (0x66) / かな (0x68) keys.
pub fn switch_ime(mode: ImeMode) -> Result<(), String> {
    let keycode = match mode {
        ImeMode::Alphanumeric => 0x66,
        ImeMode::Kana => 0x68,
    };
    post(keycode, CGEventFlags::empty())
}

/// Synthesize the keystroke described by an accelerator string (e.g. "Escape").
pub fn send_accelerator(accel: &str) -> Result<(), String> {
    let parsed = accelerator::parse(accel).map_err(|e| e.to_string())?;
    let keycode =
        keycode_for(&parsed.key).ok_or_else(|| format!("no keycode for `{}`", parsed.key))?;

    let mut flags = CGEventFlags::empty();
    if parsed.cmd {
        flags |= CGEventFlags::CGEventFlagCommand;
    }
    if parsed.ctrl {
        flags |= CGEventFlags::CGEventFlagControl;
    }
    if parsed.alt {
        flags |= CGEventFlags::CGEventFlagAlternate;
    }
    if parsed.shift {
        flags |= CGEventFlags::CGEventFlagShift;
    }
    post(keycode, flags)
}

/// Map a normalized accelerator key (see `tomari_keyboard::accelerator`) to a
/// macOS ANSI virtual keycode.
pub(crate) fn keycode_for(key: &str) -> Option<u16> {
    Some(match key {
        // Named keys.
        "Escape" => 0x35,
        "Enter" => 0x24,
        "Tab" => 0x30,
        "Space" => 0x31,
        "Delete" | "Backspace" => 0x33,
        "Left" => 0x7B,
        "Right" => 0x7C,
        "Down" => 0x7D,
        "Up" => 0x7E,
        "Home" => 0x73,
        "End" => 0x77,
        "PageUp" => 0x74,
        "PageDown" => 0x79,
        // Function keys.
        "F1" => 0x7A,
        "F2" => 0x78,
        "F3" => 0x63,
        "F4" => 0x76,
        "F5" => 0x60,
        "F6" => 0x61,
        "F7" => 0x62,
        "F8" => 0x64,
        "F9" => 0x65,
        "F10" => 0x6D,
        "F11" => 0x67,
        "F12" => 0x6F,
        // Letters (US ANSI layout).
        "A" => 0x00,
        "B" => 0x0B,
        "C" => 0x08,
        "D" => 0x02,
        "E" => 0x0E,
        "F" => 0x03,
        "G" => 0x05,
        "H" => 0x04,
        "I" => 0x22,
        "J" => 0x26,
        "K" => 0x28,
        "L" => 0x25,
        "M" => 0x2E,
        "N" => 0x2D,
        "O" => 0x1F,
        "P" => 0x23,
        "Q" => 0x0C,
        "R" => 0x0F,
        "S" => 0x01,
        "T" => 0x11,
        "U" => 0x20,
        "V" => 0x09,
        "W" => 0x0D,
        "X" => 0x07,
        "Y" => 0x10,
        "Z" => 0x06,
        // Digits.
        "0" => 0x1D,
        "1" => 0x12,
        "2" => 0x13,
        "3" => 0x14,
        "4" => 0x15,
        "5" => 0x17,
        "6" => 0x16,
        "7" => 0x1A,
        "8" => 0x1C,
        "9" => 0x19,
        _ => return None,
    })
}
