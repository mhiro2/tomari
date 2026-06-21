//! HID-level Caps Lock remapping, used to make Caps Lock usable as a managed
//! modifier at all.
//!
//! macOS delivers Caps Lock as a *lock*: one `flagsChanged` toggle per physical
//! press, no key-up, and the AlphaShift lock (LED, upper-case) is applied below
//! the event tap. So an event tap alone can neither tell when Caps is released
//! nor stop it locking. The fix is to remap the Caps Lock HID usage to an unused
//! ordinary key — **F18** — via the OS `UserKeyMapping` facility (the mechanism
//! behind `hidutil`, documented in Apple TN2450). The remap happens *before* the
//! lock is interpreted, so Caps never locks; F18 is an ordinary key, so it emits
//! real key-down/up the tap can treat as the Caps modifier ([`crate::eventtap`]).
//!
//! We shell out to `/usr/bin/hidutil` rather than call the private
//! `IOHIDEventSystemClient` API. Setting the property replaces the *whole*
//! `UserKeyMapping` list, so [`apply`] and [`clear`] read the current list first
//! and write it back with only our Caps Lock → F18 entry added or removed —
//! a user's own pre-existing `hidutil` mappings (another key remap, say) survive
//! rather than being wiped. The mapping is per-user, needs no elevated
//! privileges, and persists until reboot or removal — so we reconcile it on
//! every tap (re)start and clear it on quit.

use std::process::Command;

/// Full HID usage (`0x7_0000_0000 | usage`) of Caps Lock.
const CAPS_USAGE: u64 = 0x7_0000_0039;
/// Full HID usage of F18 — an ordinary key with no default macOS binding,
/// which Caps Lock is remapped onto.
const F18_USAGE: u64 = 0x7_0000_006D;

/// The virtual keycode F18 arrives as once Caps Lock is remapped to it. The tap
/// treats this keycode as the Caps Lock modifier.
pub const F18_KEYCODE: i64 = 79;

fn set_mapping(json: &str) -> Result<(), String> {
    let output = Command::new("/usr/bin/hidutil")
        .args(["property", "--set", json])
        .output()
        .map_err(|e| format!("failed to run hidutil: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "hidutil exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// Read the current `UserKeyMapping` entries as `(src, dst)` usage pairs.
/// `None` when `hidutil` could not be run at all — distinct from an empty list,
/// so callers never mistake "unreadable" for "no mappings" and clobber the
/// user's own remaps.
fn read_entries() -> Option<Vec<(u64, u64)>> {
    let output = Command::new("/usr/bin/hidutil")
        .args(["property", "--get", "UserKeyMapping"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(parse_entries(&String::from_utf8_lossy(&output.stdout)))
}

/// Parse every `UserKeyMapping` entry out of `hidutil property --get` text into
/// `(src, dst)` usage pairs. Splitting on `}` yields one block per entry (the
/// trailing fragment carries neither field and is dropped).
fn parse_entries(text: &str) -> Vec<(u64, u64)> {
    text.split('}')
        .filter_map(|entry| {
            let src = entry_field(entry, "HIDKeyboardModifierMappingSrc")?;
            let dst = entry_field(entry, "HIDKeyboardModifierMappingDst")?;
            Some((src, dst))
        })
        .collect()
}

/// The entry list with our Caps Lock → F18 mapping ensured present: any existing
/// entry whose *source* is Caps Lock is replaced (we own that source), every
/// other mapping is kept untouched.
fn with_caps_mapping(mut entries: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    entries.retain(|&(src, _)| src != CAPS_USAGE);
    entries.push((CAPS_USAGE, F18_USAGE));
    entries
}

/// The entry list with *only* our Caps Lock → F18 mapping removed, leaving every
/// other mapping in place. `None` when ours is not present, so the caller can
/// skip a redundant write.
fn without_caps_mapping(entries: Vec<(u64, u64)>) -> Option<Vec<(u64, u64)>> {
    if !entries.contains(&(CAPS_USAGE, F18_USAGE)) {
        return None;
    }
    Some(
        entries
            .into_iter()
            .filter(|&pair| pair != (CAPS_USAGE, F18_USAGE))
            .collect(),
    )
}

/// Serialize `(src, dst)` pairs into the JSON `hidutil property --set` expects.
fn serialize_mapping(entries: &[(u64, u64)]) -> String {
    let body = entries
        .iter()
        .map(|(src, dst)| {
            format!(
                r#"{{"HIDKeyboardModifierMappingSrc":{src:#x},"HIDKeyboardModifierMappingDst":{dst:#x}}}"#
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(r#"{{"UserKeyMapping":[{body}]}}"#)
}

/// Remap Caps Lock to F18 at the HID level, preserving the user's other
/// mappings. Errors if the current list cannot be read, rather than overwriting
/// it blind.
pub fn apply() -> Result<(), String> {
    let entries = read_entries().ok_or("could not read current hidutil key mappings")?;
    set_mapping(&serialize_mapping(&with_caps_mapping(entries)))
}

/// Remove our Caps Lock → F18 mapping, restoring Caps Lock's native behavior
/// while leaving every other `UserKeyMapping` entry the user set in place.
pub fn clear() -> Result<(), String> {
    let entries = read_entries().ok_or("could not read current hidutil key mappings")?;
    match without_caps_mapping(entries) {
        // Ours was present: write back the list without it.
        Some(remaining) => set_mapping(&serialize_mapping(&remaining)),
        // Ours was not there — nothing of ours to remove, so leave the list be.
        None => Ok(()),
    }
}

/// Whether our Caps Lock → F18 remap is currently in effect, read from the live
/// system property so it is correct even across a crash that skipped [`clear`].
pub fn is_active() -> bool {
    let Ok(output) = Command::new("/usr/bin/hidutil")
        .args(["property", "--get", "UserKeyMapping"])
        .output()
    else {
        return false;
    };
    maps_caps_to_f18(&String::from_utf8_lossy(&output.stdout))
}

/// Whether `hidutil`'s `UserKeyMapping` text contains a single entry mapping
/// Caps Lock to F18 — checked *structurally*, so the two usages appearing in
/// unrelated entries (e.g. `Caps → X` plus `Y → F18`) is not mistaken for ours.
fn maps_caps_to_f18(text: &str) -> bool {
    // Each entry is a `{ … }` block carrying one Src and one Dst; splitting on
    // `}` yields one block per entry (the trailing fragment carries neither).
    text.split('}').any(|entry| {
        entry_field(entry, "HIDKeyboardModifierMappingSrc") == Some(CAPS_USAGE)
            && entry_field(entry, "HIDKeyboardModifierMappingDst") == Some(F18_USAGE)
    })
}

/// The usage value of `key` within one `UserKeyMapping` entry, if present.
fn entry_field(entry: &str, key: &str) -> Option<u64> {
    let after_key = entry.get(entry.find(key)? + key.len()..)?;
    let after_eq = after_key.get(after_key.find('=')? + 1..)?;
    let value = after_eq.get(..after_eq.find(';')?)?;
    parse_usage(value)
}

/// Parse a HID usage printed by `hidutil`, which uses decimal or hex (`0x…`)
/// depending on the macOS version.
fn parse_usage(s: &str) -> Option<u64> {
    let t = s.trim();
    match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => t.parse().ok(),
    }
}

/// Bring the HID remap into line with whether Caps Lock should be managed: apply
/// it when it should be and is not yet, clear it when it should not be but a
/// stale one (e.g. left by a crash) is present. Reading the live state first
/// avoids both a redundant `hidutil` spawn and clobbering an unrelated mapping
/// when there is nothing of ours to remove.
///
/// Returns whether the remap is actually in effect afterwards — the *real*
/// state, not the request — so a caller can gate F18 handling on it even when
/// `hidutil` fails. On a failed transition the live state is re-read.
#[must_use]
pub fn reconcile(should_manage: bool) -> bool {
    let active = is_active();
    match (should_manage, active) {
        (true, false) => match apply() {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(error = %e, "failed to apply caps-lock HID remap");
                is_active()
            }
        },
        (false, true) => match clear() {
            Ok(()) => false,
            Err(e) => {
                tracing::warn!(error = %e, "failed to clear caps-lock HID remap");
                is_active()
            }
        },
        // Already in the desired state.
        _ => active,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for a user's own pre-existing remap (some other key), used to
    /// prove merge/remove never touch mappings that are not ours.
    const OTHER_SRC: u64 = 0x7_0000_0004;
    const OTHER_DST: u64 = 0x7_0000_0005;

    #[test]
    fn parse_entries_reads_every_pair() {
        // The decimal shape `hidutil property --get` prints, with two entries.
        let text = format!(
            "(\n  {{\n    HIDKeyboardModifierMappingSrc = {CAPS_USAGE};\n    \
             HIDKeyboardModifierMappingDst = {F18_USAGE};\n  }}\n  {{\n    \
             HIDKeyboardModifierMappingSrc = {OTHER_SRC};\n    \
             HIDKeyboardModifierMappingDst = {OTHER_DST};\n  }}\n)"
        );
        assert_eq!(
            parse_entries(&text),
            vec![(CAPS_USAGE, F18_USAGE), (OTHER_SRC, OTHER_DST)]
        );
    }

    #[test]
    fn apply_merge_preserves_a_foreign_mapping() {
        // Turning Caps management on must keep the user's other remap.
        let merged = with_caps_mapping(vec![(OTHER_SRC, OTHER_DST)]);
        assert!(merged.contains(&(OTHER_SRC, OTHER_DST)));
        assert!(merged.contains(&(CAPS_USAGE, F18_USAGE)));
    }

    #[test]
    fn apply_merge_replaces_a_conflicting_caps_source() {
        // A pre-existing Caps Lock remap to something else is ours to own:
        // replace it, leaving exactly one entry for the Caps source.
        let merged = with_caps_mapping(vec![(CAPS_USAGE, OTHER_DST), (OTHER_SRC, OTHER_DST)]);
        assert_eq!(
            merged.iter().filter(|&&(src, _)| src == CAPS_USAGE).count(),
            1
        );
        assert!(merged.contains(&(CAPS_USAGE, F18_USAGE)));
        assert!(merged.contains(&(OTHER_SRC, OTHER_DST)));
    }

    #[test]
    fn clear_removes_only_our_entry() {
        let remaining =
            without_caps_mapping(vec![(CAPS_USAGE, F18_USAGE), (OTHER_SRC, OTHER_DST)]).unwrap();
        assert_eq!(remaining, vec![(OTHER_SRC, OTHER_DST)]);
    }

    #[test]
    fn clear_is_a_noop_when_ours_is_absent() {
        // A foreign Caps remap is not ours to remove, nor is an empty list.
        assert!(
            without_caps_mapping(vec![(CAPS_USAGE, OTHER_DST), (OTHER_SRC, OTHER_DST)]).is_none()
        );
        assert!(without_caps_mapping(vec![]).is_none());
    }

    #[test]
    fn serialize_empty_is_an_empty_list() {
        assert_eq!(serialize_mapping(&[]), r#"{"UserKeyMapping":[]}"#);
    }

    #[test]
    fn serialize_emits_each_entry_as_hex() {
        // The exact shape `hidutil property --set` previously received for our
        // lone entry, so the merge path keeps feeding hidutil what it accepts.
        assert_eq!(
            serialize_mapping(&[(CAPS_USAGE, F18_USAGE)]),
            r#"{"UserKeyMapping":[{"HIDKeyboardModifierMappingSrc":0x700000039,"HIDKeyboardModifierMappingDst":0x70000006d}]}"#
        );
    }

    #[test]
    fn detects_our_caps_to_f18_entry_decimal() {
        // The shape `hidutil property --get` prints (decimal usages).
        let text = "(\n    {\n        HIDKeyboardModifierMappingDst = 30064771181;\n        \
             HIDKeyboardModifierMappingSrc = 30064771129;\n    }\n)";
        assert!(maps_caps_to_f18(text));
    }

    #[test]
    fn detects_our_entry_in_hex() {
        let text = "({HIDKeyboardModifierMappingSrc = 0x700000039; \
             HIDKeyboardModifierMappingDst = 0x70000006d;})";
        assert!(maps_caps_to_f18(text));
    }

    #[test]
    fn empty_or_null_is_not_active() {
        assert!(!maps_caps_to_f18("(null)"));
        assert!(!maps_caps_to_f18("(\n)"));
        assert!(!maps_caps_to_f18(""));
    }

    #[test]
    fn caps_and_f18_in_separate_entries_is_not_ours() {
        // Caps mapped elsewhere AND F18 used as some other key's target: neither
        // entry is Caps→F18, so this must not read as our remap.
        let text = "(\n  {\n    HIDKeyboardModifierMappingSrc = 30064771129;\n    \
             HIDKeyboardModifierMappingDst = 30064771072;\n  }\n  {\n    \
             HIDKeyboardModifierMappingSrc = 30064771070;\n    \
             HIDKeyboardModifierMappingDst = 30064771181;\n  }\n)";
        assert!(!maps_caps_to_f18(text));
    }
}
