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
    let (keycode, mut flags) =
        key_to_event(&parsed.key).ok_or_else(|| format!("no keycode for `{}`", parsed.key))?;

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

/// The keycode and any modifier flags implied by the key name alone. `Plus` is
/// Shift+`=` on the ANSI layout, so it carries an implied Shift; every other key
/// contributes no flags of its own. `None` when the key has no keycode.
fn key_to_event(key: &str) -> Option<(u16, CGEventFlags)> {
    let keycode = keycode_for(key)?;
    let flags = if key == "Plus" {
        CGEventFlags::CGEventFlagShift
    } else {
        CGEventFlags::empty()
    };
    Some((keycode, flags))
}

/// Map a normalized accelerator key (see `tomari_keyboard::accelerator`) to a
/// macOS ANSI virtual keycode. The set kept here must cover every key the
/// accelerator parser can emit, or a `SendKeystroke` would save yet fail at
/// send time.
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
        // F13–F20. macOS defines no virtual keycodes past F20, so F21–F24 (which
        // the parser still accepts) have no mapping and remain unsendable.
        "F13" => 0x69,
        "F14" => 0x6B,
        "F15" => 0x71,
        "F16" => 0x6A,
        "F17" => 0x40,
        "F18" => 0x4F,
        "F19" => 0x50,
        "F20" => 0x5A,
        // Punctuation (US ANSI). `Plus` shares the `=` key; `key_to_event` adds
        // its implied Shift.
        "Minus" => 0x1B,
        "Equal" | "Plus" => 0x18,
        "Comma" => 0x2B,
        "Period" => 0x2F,
        "Slash" => 0x2C,
        "Semicolon" => 0x29,
        "Quote" => 0x27,
        "BracketLeft" => 0x21,
        "BracketRight" => 0x1E,
        "Backslash" => 0x2A,
        "Backquote" => 0x32,
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

#[cfg(test)]
mod tests {
    use super::*;
    use tomari_keyboard::accelerator;

    /// Every key the accelerator parser can produce — and that Tomari can store
    /// in a `SendKeystroke` — must resolve to a keycode here, or the action would
    /// parse and save yet fail at send time. macOS defines virtual keycodes only
    /// through F20, so F21–F24 are the sole accepted gap.
    #[test]
    fn keysend_covers_parser_punctuation_and_function_keys() {
        for key in [
            "Plus",
            "Minus",
            "Equal",
            "Comma",
            "Period",
            "Slash",
            "Semicolon",
            "Quote",
            "BracketLeft",
            "BracketRight",
            "Backslash",
            "Backquote",
            "F13",
            "F14",
            "F15",
            "F16",
            "F17",
            "F18",
            "F19",
            "F20",
        ] {
            let parsed = accelerator::parse(key).unwrap();
            assert!(
                key_to_event(&parsed.key).is_some(),
                "no keycode for parser-accepted key `{key}`"
            );
        }
    }

    #[test]
    fn plus_is_shift_equal() {
        let (equal, equal_flags) = key_to_event("Equal").unwrap();
        let (plus, plus_flags) = key_to_event("Plus").unwrap();
        // Same physical key; `Plus` differs only by the implied Shift.
        assert_eq!(plus, equal);
        assert!(equal_flags.is_empty());
        assert!(plus_flags.contains(CGEventFlags::CGEventFlagShift));
    }
}
