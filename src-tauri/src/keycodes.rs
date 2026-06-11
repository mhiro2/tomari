//! macOS virtual-keycode tables for the event tap.
//!
//! Two jobs: identify which modifier (and side) a `flagsChanged` keycode
//! represents, and pick the CGEvent flag bit / representative keycode to emit
//! when remapping a modifier.

use core_graphics::event::CGEventFlags;
use tomari_core::{KeySide, ModifierKey};

/// Identify the modifier key (and physical side) a `flagsChanged` keycode
/// represents, or `None` if the keycode is not a managed modifier.
pub fn modifier_for_keycode(keycode: i64) -> Option<(ModifierKey, KeySide)> {
    Some(match keycode {
        55 => (ModifierKey::Command, KeySide::Left),
        54 => (ModifierKey::Command, KeySide::Right),
        56 => (ModifierKey::Shift, KeySide::Left),
        60 => (ModifierKey::Shift, KeySide::Right),
        59 => (ModifierKey::Control, KeySide::Left),
        62 => (ModifierKey::Control, KeySide::Right),
        58 => (ModifierKey::Option, KeySide::Left),
        61 => (ModifierKey::Option, KeySide::Right),
        57 => (ModifierKey::CapsLock, KeySide::Either),
        63 => (ModifierKey::Function, KeySide::Either),
        _ => return None,
    })
}

/// The device-specific flag bit (`NX_DEVICE*KEYMASK` in IOKit's `IOLLEvent.h`)
/// a left/right modifier keycode contributes to an event's flags. A
/// `flagsChanged` event carries the bit while its key is physically down, so
/// the tap can derive down/up from the event itself instead of toggling
/// remembered state — which would invert if a tap restart swallowed a
/// transition mid-hold. `None` for Caps Lock and Fn, whose state lives in
/// other bits.
pub fn device_flag_for_keycode(keycode: i64) -> Option<u64> {
    Some(match keycode {
        59 => 0x0000_0001, // NX_DEVICELCTLKEYMASK
        56 => 0x0000_0002, // NX_DEVICELSHIFTKEYMASK
        60 => 0x0000_0004, // NX_DEVICERSHIFTKEYMASK
        55 => 0x0000_0008, // NX_DEVICELCMDKEYMASK
        54 => 0x0000_0010, // NX_DEVICERCMDKEYMASK
        58 => 0x0000_0020, // NX_DEVICELALTKEYMASK
        61 => 0x0000_0040, // NX_DEVICERALTKEYMASK
        62 => 0x0000_2000, // NX_DEVICERCTLKEYMASK
        _ => return None,
    })
}

/// The CGEvent flag bit a modifier contributes to an event's flags.
pub fn flag_for(modifier: ModifierKey) -> CGEventFlags {
    match modifier {
        ModifierKey::Command => CGEventFlags::CGEventFlagCommand,
        ModifierKey::Control => CGEventFlags::CGEventFlagControl,
        ModifierKey::Option => CGEventFlags::CGEventFlagAlternate,
        ModifierKey::Shift => CGEventFlags::CGEventFlagShift,
        ModifierKey::CapsLock => CGEventFlags::CGEventFlagAlphaShift,
        ModifierKey::Function => CGEventFlags::CGEventFlagSecondaryFn,
    }
}

/// A representative (left-side) virtual keycode to assign when rewriting a
/// `flagsChanged` event into a remapped modifier.
pub fn primary_keycode(modifier: ModifierKey) -> i64 {
    match modifier {
        ModifierKey::Command => 55,
        ModifierKey::Shift => 56,
        ModifierKey::Control => 59,
        ModifierKey::Option => 58,
        ModifierKey::CapsLock => 57,
        ModifierKey::Function => 63,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinguishes_left_and_right_command() {
        assert_eq!(
            modifier_for_keycode(55),
            Some((ModifierKey::Command, KeySide::Left))
        );
        assert_eq!(
            modifier_for_keycode(54),
            Some((ModifierKey::Command, KeySide::Right))
        );
    }

    #[test]
    fn caps_lock_and_fn_are_sideless() {
        assert_eq!(
            modifier_for_keycode(57),
            Some((ModifierKey::CapsLock, KeySide::Either))
        );
        assert_eq!(
            modifier_for_keycode(63),
            Some((ModifierKey::Function, KeySide::Either))
        );
    }

    #[test]
    fn sided_modifiers_have_distinct_device_flags() {
        // Every left/right modifier keycode must own a unique device bit;
        // Caps Lock and Fn (sideless) are intentionally not covered.
        let mut seen = std::collections::HashSet::new();
        for code in 0i64..=0x7F {
            match modifier_for_keycode(code) {
                Some((_, KeySide::Left | KeySide::Right)) => {
                    let bit = device_flag_for_keycode(code).expect("device bit");
                    assert!(seen.insert(bit), "duplicate device bit for {code}");
                }
                _ => assert_eq!(device_flag_for_keycode(code), None),
            }
        }
        assert_eq!(seen.len(), 8);
    }

    #[test]
    fn non_modifier_keycode_is_none() {
        // 'A' is a key, not a modifier.
        assert_eq!(modifier_for_keycode(0x00), None);
    }

    #[test]
    fn caps_lock_flag_is_alpha_shift() {
        assert_eq!(
            flag_for(ModifierKey::CapsLock),
            CGEventFlags::CGEventFlagAlphaShift
        );
        assert_eq!(
            flag_for(ModifierKey::Control),
            CGEventFlags::CGEventFlagControl
        );
    }
}
