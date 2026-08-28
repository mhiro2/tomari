//! macOS keystroke synthesis via Core Graphics events, used to realize the
//! `SwitchIme` and `SendKeystroke` actions. Posting key events requires the
//! Accessibility permission (the same one window management uses).

use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTapLocation, CGEventTapProxy, EventField,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use tomari_core::ImeMode;
use tomari_keyboard::accelerator;

use crate::eventtap::SYNTHETIC_MARKER;

/// Where a synthesized keystroke enters the event stream.
#[derive(Clone, Copy)]
pub enum Sink {
    /// At the HID level, as if typed — from a hotkey, the tray, a URL. Goes
    /// through every event tap, ours included (which ignores it by its marker).
    Hid,
    /// From inside our event tap's callback, through the tap's proxy. The
    /// events are inserted into the stream at the tap's own position, ahead of
    /// every event that has not yet passed through it — so a keystroke posted
    /// while handling a modifier tap is guaranteed to reach the app before the
    /// character key the user types next, however busy the rest of Tomari is.
    /// Events posted this way never come back through the posting tap.
    Tap(CGEventTapProxy),
}

/// Post a key-down/key-up pair for `keycode` with `flags`.
///
/// Both events are built and tagged before either is posted, so a failure to
/// allocate the key-up cannot leave the app holding an unmatched key-down.
fn post(keycode: u16, flags: CGEventFlags, sink: Sink) -> Result<(), String> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|()| "failed to create CGEventSource".to_string())?;
    let down = CGEvent::new_keyboard_event(source.clone(), keycode, true)
        .map_err(|()| "failed to create key-down event".to_string())?;
    let up = CGEvent::new_keyboard_event(source, keycode, false)
        .map_err(|()| "failed to create key-up event".to_string())?;
    for event in [&down, &up] {
        event.set_flags(flags);
        // Tag synthesized events so our own event tap ignores them (no feedback).
        event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, SYNTHETIC_MARKER);
    }
    for event in [down, up] {
        match sink {
            Sink::Hid => event.post(CGEventTapLocation::HID),
            Sink::Tap(proxy) => event.post_from_tap(proxy),
        }
    }
    Ok(())
}

/// Switch the input method by posting the JIS 英数 (0x66) / かな (0x68) keys.
pub fn switch_ime(mode: ImeMode, sink: Sink) -> Result<(), String> {
    let keycode = match mode {
        ImeMode::Alphanumeric => 0x66,
        ImeMode::Kana => 0x68,
    };
    post(keycode, CGEventFlags::empty(), sink)
}

/// Synthesize the keystroke described by an accelerator string (e.g. "Escape").
pub fn send_accelerator(accel: &str, sink: Sink) -> Result<(), String> {
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
    post(keycode, flags, sink)
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
        // F13–F20. macOS defines no virtual keycodes past F20, which is why
        // the accelerator parser also stops there.
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
    /// in a `SendKeystroke` — must resolve to a keycode here, or the action
    /// would parse and save yet fail at send time. The parser caps function
    /// keys at F20 precisely because macOS defines no virtual keycode past it,
    /// so there is no accepted gap: coverage must be total. The key set comes
    /// from the parser itself (`all_canonical_keys`), so a key added there
    /// fails here until this map handles it.
    #[test]
    fn keysend_covers_every_parser_accepted_key() {
        for key in accelerator::all_canonical_keys() {
            let parsed = accelerator::parse(&key).unwrap();
            assert_eq!(parsed.key, key, "`{key}` must already be canonical");
            assert!(
                key_to_event(&key).is_some(),
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
