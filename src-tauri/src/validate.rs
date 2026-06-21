//! Server-side validation of user-editable records that cross the command
//! boundary. The frontend already guards these, but the boundary must not trust
//! it: a malformed accelerator could break global-shortcut registration, a bare
//! letter would shadow normal typing once registered globally, and an unbounded
//! id/label could be persisted unchecked.

use tomari_core::Hotkey;
use tomari_keyboard::accelerator;

use crate::error::CmdError;

/// Upper bounds (in characters) on stored identifiers and labels — generous for
/// any real entry (a UUID-based id, a human label) while rejecting pathological
/// input.
const MAX_ID_LEN: usize = 128;
const MAX_LABEL_LEN: usize = 200;

/// Validate and canonicalize a hotkey before it is stored. Returns a sanitized
/// copy — trimmed id/label, normalized accelerator — or a [`CmdError`]
/// describing the first problem found.
pub fn sanitize_hotkey(hotkey: Hotkey) -> Result<Hotkey, CmdError> {
    let id = hotkey.id.trim();
    if id.is_empty() {
        return Err(CmdError::other("hotkey id must not be empty"));
    }
    if id.chars().count() > MAX_ID_LEN {
        return Err(CmdError::other(format!(
            "hotkey id is too long (max {MAX_ID_LEN} characters)"
        )));
    }

    let label = hotkey.label.trim();
    if label.is_empty() {
        return Err(CmdError::other("hotkey label must not be empty"));
    }
    if label.chars().count() > MAX_LABEL_LEN {
        return Err(CmdError::other(format!(
            "hotkey label is too long (max {MAX_LABEL_LEN} characters)"
        )));
    }

    // Parse here so an unregisterable string is rejected with a clear reason
    // rather than surfacing later as a misleading "conflict", and normalize to
    // the canonical spelling the rest of the app compares against.
    let parsed = accelerator::parse(&hotkey.accelerator)
        .map_err(|e| CmdError::other(format!("invalid shortcut: {e}")))?;

    // A global shortcut with no Ctrl/Alt/Cmd would shadow normal typing
    // system-wide (Shift alone is not enough); function keys may stand alone.
    // Mirrors the recorder's rule in `src/lib/recorder.ts`.
    if !(parsed.ctrl || parsed.alt || parsed.cmd || is_function_key(&parsed.key)) {
        return Err(CmdError::other(
            "a global shortcut needs ⌃, ⌥ or ⌘ — or a function key",
        ));
    }

    Ok(Hotkey {
        id: id.to_string(),
        label: label.to_string(),
        accelerator: parsed.to_canonical(),
        action: hotkey.action,
        enabled: hotkey.enabled,
    })
}

/// Whether a canonical key token is a function key (`F1`..=`F24`).
fn is_function_key(key: &str) -> bool {
    key.strip_prefix('F')
        .and_then(|n| n.parse::<u32>().ok())
        .is_some_and(|n| (1..=24).contains(&n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tomari_core::AppAction;

    fn hotkey(accelerator: &str) -> Hotkey {
        Hotkey {
            id: "hk-1".into(),
            label: "Test".into(),
            accelerator: accelerator.into(),
            action: AppAction::TogglePanel,
            enabled: true,
        }
    }

    #[test]
    fn normalizes_accelerator_to_canonical_form() {
        let out = sanitize_hotkey(hotkey("command+shift+r")).unwrap();
        assert_eq!(out.accelerator, "Shift+Cmd+R");
    }

    #[test]
    fn trims_id_and_label() {
        let mut hk = hotkey("Cmd+K");
        hk.id = "  hk-1  ".into();
        hk.label = "  Snap left  ".into();
        let out = sanitize_hotkey(hk).unwrap();
        assert_eq!(out.id, "hk-1");
        assert_eq!(out.label, "Snap left");
    }

    #[test]
    fn rejects_bare_letter_or_shift_only_chord() {
        // A lone letter, or one held only with Shift, would swallow plain typing
        // once registered as a global shortcut.
        assert!(sanitize_hotkey(hotkey("A")).is_err());
        assert!(sanitize_hotkey(hotkey("Shift+A")).is_err());
    }

    #[test]
    fn allows_bare_function_key_and_real_chords() {
        assert!(sanitize_hotkey(hotkey("F5")).is_ok());
        assert!(sanitize_hotkey(hotkey("Cmd+Alt+Left")).is_ok());
    }

    #[test]
    fn rejects_invalid_or_empty_accelerator() {
        assert!(sanitize_hotkey(hotkey("Cmd+Frobnicate")).is_err());
        assert!(sanitize_hotkey(hotkey("")).is_err());
    }

    #[test]
    fn rejects_empty_or_overlong_id_and_label() {
        let mut blank = hotkey("Cmd+K");
        blank.label = "   ".into();
        assert!(sanitize_hotkey(blank).is_err());

        let mut long_label = hotkey("Cmd+K");
        long_label.label = "x".repeat(MAX_LABEL_LEN + 1);
        assert!(sanitize_hotkey(long_label).is_err());

        let mut long_id = hotkey("Cmd+K");
        long_id.id = "x".repeat(MAX_ID_LEN + 1);
        assert!(sanitize_hotkey(long_id).is_err());
    }
}
