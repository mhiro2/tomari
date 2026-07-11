//! Parsing, validation and normalization of global-shortcut accelerator
//! strings such as `"Cmd+Shift+R"`.
//!
//! Accelerators are normalized to a canonical form — modifiers in the fixed
//! order `Ctrl, Alt, Shift, Cmd` followed by a single key — so that the same
//! chord typed two different ways compares equal and round-trips cleanly into
//! Tauri's global-shortcut plugin.

use crate::error::{Error, Result};

/// A parsed accelerator: an ordered modifier set plus exactly one key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accelerator {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub cmd: bool,
    pub key: String,
}

impl Accelerator {
    /// Render the canonical `Mod+Mod+Key` string.
    pub fn to_canonical(&self) -> String {
        let mut parts = Vec::with_capacity(5);
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.cmd {
            parts.push("Cmd");
        }
        parts.push(&self.key);
        parts.join("+")
    }
}

/// Canonical modifier name for a token, or `None` if it is not a modifier.
fn modifier_of(token: &str) -> Option<&'static str> {
    match token.to_ascii_lowercase().as_str() {
        "cmd" | "command" | "super" | "win" | "meta" | "⌘" | "commandorcontrol" | "cmdorctrl" => {
            Some("Cmd")
        }
        "ctrl" | "control" | "^" | "⌃" => Some("Ctrl"),
        "alt" | "opt" | "option" | "⌥" => Some("Alt"),
        "shift" | "⇧" => Some("Shift"),
        _ => None,
    }
}

/// Normalize a non-modifier key token to its canonical spelling, or `None` if
/// it is not a recognized key.
fn normalize_key(token: &str) -> Option<String> {
    let lower = token.to_ascii_lowercase();

    // Function keys F1..F24.
    if let Some(num) = lower.strip_prefix('f')
        && let Ok(n) = num.parse::<u32>()
        && (1..=24).contains(&n)
    {
        return Some(format!("F{n}"));
    }

    // Single alphanumeric character.
    if token.chars().count() == 1 {
        let c = token.chars().next().unwrap();
        if c.is_ascii_alphanumeric() {
            return Some(c.to_ascii_uppercase().to_string());
        }
    }

    let named = match lower.as_str() {
        "left" => "Left",
        "right" => "Right",
        "up" => "Up",
        "down" => "Down",
        "space" => "Space",
        "tab" => "Tab",
        "enter" | "return" => "Enter",
        "esc" | "escape" => "Escape",
        "delete" | "del" => "Delete",
        "backspace" => "Backspace",
        "home" => "Home",
        "end" => "End",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        "plus" => "Plus",
        "minus" => "Minus",
        "equal" => "Equal",
        "comma" => "Comma",
        "period" => "Period",
        "slash" => "Slash",
        // Punctuation/symbol keys. Names match what global-hotkey's accelerator
        // parser (`global_hotkey::hotkey::parse_key`) accepts, since that is
        // the crate `shortcuts::register_all` hands the canonical string to.
        "semicolon" => "Semicolon",
        "quote" => "Quote",
        "bracketleft" => "BracketLeft",
        "bracketright" => "BracketRight",
        "backslash" => "Backslash",
        "backquote" => "Backquote",
        _ => return None,
    };
    Some(named.to_string())
}

/// Parse and normalize an accelerator string.
pub fn parse(input: &str) -> Result<Accelerator> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(Error::invalid(input, "empty accelerator"));
    }

    let mut acc = Accelerator {
        ctrl: false,
        alt: false,
        shift: false,
        cmd: false,
        key: String::new(),
    };
    let mut key: Option<String> = None;

    for raw in trimmed.split('+') {
        let token = raw.trim();
        if token.is_empty() {
            return Err(Error::invalid(input, "empty component between '+'"));
        }
        if let Some(m) = modifier_of(token) {
            match m {
                "Ctrl" => acc.ctrl = true,
                "Alt" => acc.alt = true,
                "Shift" => acc.shift = true,
                "Cmd" => acc.cmd = true,
                _ => unreachable!(),
            }
        } else if let Some(k) = normalize_key(token) {
            if key.is_some() {
                return Err(Error::invalid(input, "more than one non-modifier key"));
            }
            key = Some(k);
        } else {
            return Err(Error::invalid(
                input,
                format!("unrecognized component `{token}`"),
            ));
        }
    }

    match key {
        Some(k) => {
            acc.key = k;
            Ok(acc)
        }
        None => Err(Error::invalid(input, "no non-modifier key")),
    }
}

/// Normalize an accelerator string to its canonical form.
pub fn normalize(input: &str) -> Result<String> {
    parse(input).map(|a| a.to_canonical())
}

/// Whether the accelerator string is well-formed.
pub fn is_valid(input: &str) -> bool {
    parse(input).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_aliases_and_order() {
        assert_eq!(normalize("command+shift+r").unwrap(), "Shift+Cmd+R");
        assert_eq!(normalize("Option+Ctrl+Left").unwrap(), "Ctrl+Alt+Left");
        assert_eq!(normalize("CmdOrCtrl+K").unwrap(), "Cmd+K");
    }

    #[test]
    fn function_keys_and_named_keys() {
        assert_eq!(normalize("F5").unwrap(), "F5");
        assert_eq!(normalize("cmd+enter").unwrap(), "Cmd+Enter");
        assert_eq!(normalize("ctrl+alt+escape").unwrap(), "Ctrl+Alt+Escape");
    }

    #[test]
    fn whitespace_is_tolerated() {
        assert_eq!(normalize(" cmd + shift + k ").unwrap(), "Shift+Cmd+K");
    }

    #[test]
    fn idempotent() {
        let once = normalize("alt+cmd+right").unwrap();
        assert_eq!(normalize(&once).unwrap(), once);
    }

    #[test]
    fn rejects_invalid() {
        assert!(!is_valid(""));
        assert!(!is_valid("Cmd+"));
        assert!(!is_valid("Cmd+Shift")); // no key
        assert!(!is_valid("Cmd+A+B")); // two keys
        assert!(!is_valid("Cmd+Frobnicate"));
        assert!(!is_valid("F25"));
    }

    #[test]
    fn bare_key_is_valid() {
        assert!(is_valid("F1"));
        assert!(is_valid("A"));
    }

    #[test]
    fn symbol_keys() {
        assert_eq!(normalize("cmd+semicolon").unwrap(), "Cmd+Semicolon");
        assert_eq!(normalize("cmd+quote").unwrap(), "Cmd+Quote");
        assert_eq!(normalize("cmd+bracketleft").unwrap(), "Cmd+BracketLeft");
        assert_eq!(normalize("cmd+bracketright").unwrap(), "Cmd+BracketRight");
        assert_eq!(normalize("cmd+backslash").unwrap(), "Cmd+Backslash");
        assert_eq!(normalize("cmd+backquote").unwrap(), "Cmd+Backquote");
    }

    #[test]
    fn jis_intl_keys_are_not_supported() {
        // global-hotkey's accelerator parser (`global_hotkey::hotkey::parse_key`)
        // has no `IntlYen`/`IntlRo` variants, so accepting them here would save a
        // hotkey that fails to register.
        assert!(!is_valid("Cmd+IntlYen"));
        assert!(!is_valid("Cmd+IntlRo"));
    }
}
