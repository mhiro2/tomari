//! The external control surface: the `tomari://` URL scheme that lets launchers
//! like Raycast and Alfred drive Tomari from outside the app.
//!
//! This module is the security boundary between the open-ended internal
//! [`AppAction`] vocabulary and what an arbitrary process (or web page) is
//! allowed to invoke. [`ExternalAction`] is a deliberately small allowlist —
//! window placement only — and is the *only* way a deep link turns into an
//! [`AppAction`]. Adding a new `AppAction` variant therefore cannot accidentally
//! expose it: a matching `ExternalAction` and parser arm have to be added here
//! on purpose.
//!
//! The grammar is versioned from day one: `tomari://v1/<verb>[/<arg>]`. Parsing
//! is strict — an unknown version, verb, argument, a query string, fragment,
//! userinfo or port are all rejected rather than ignored, so a typo fails
//! loudly instead of silently doing the wrong thing.

use crate::domain::{AppAction, DisplayDirection, WindowPreset};

/// The current (and only) deep-link grammar version, carried as the URL host:
/// `tomari://v1/...`.
const API_VERSION: &str = "v1";

/// Upper bound on the length of a deep-link URL we will even attempt to parse,
/// as a cheap guard against a pathological input.
const MAX_URL_LEN: usize = 512;

/// An action an external caller is permitted to invoke through the URL scheme.
///
/// This is intentionally a strict subset of [`AppAction`]: window placement
/// that acts on the focused window. It bounds *what* an external caller may do
/// — not *who* may call (any local process or web page that can open a URL
/// can). These actions are low-risk — focused-window placement, all undoable —
/// so they ship enabled during the canary behind `external_window_actions_enabled`;
/// a stable release should default that switch off. Launching apps, synthesizing
/// keystrokes, IME switching and leader mode are deliberately absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalAction {
    /// Snap the focused window to a preset, applied exactly (never cycling).
    Snap(WindowPreset),
    /// Move the focused window to a neighboring display.
    MoveDisplay(DisplayDirection),
    /// Undo the last window move.
    Undo,
    /// Show or hide the menu-bar panel.
    TogglePanel,
}

impl From<ExternalAction> for AppAction {
    fn from(action: ExternalAction) -> Self {
        match action {
            // Deterministic by design: a repeated `tomari://v1/snap/left-half`
            // must land on the left half every time, not advance the cycle.
            ExternalAction::Snap(preset) => AppAction::SnapWindowExact(preset),
            ExternalAction::MoveDisplay(direction) => AppAction::MoveWindowToDisplay(direction),
            ExternalAction::Undo => AppAction::UndoWindow,
            ExternalAction::TogglePanel => AppAction::TogglePanel,
        }
    }
}

/// Why a `tomari://` URL could not be turned into an [`ExternalAction`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeepLinkError {
    #[error("not a tomari:// URL")]
    WrongScheme,
    #[error("malformed URL: {0}")]
    Malformed(String),
    #[error("URL is too long")]
    TooLong,
    #[error("URL must not contain {0}")]
    DisallowedComponent(&'static str),
    #[error("unsupported API version `{0}` (expected `{API_VERSION}`)")]
    UnknownVersion(String),
    #[error("unknown command `{0}`")]
    UnknownVerb(String),
    #[error("`{0}` takes no further path segments")]
    UnexpectedArguments(String),
    #[error("`{0}` is missing its argument")]
    MissingArgument(String),
    #[error("unknown window preset `{0}`")]
    UnknownPreset(String),
    #[error("unknown display direction `{0}` (expected `next` or `prev`)")]
    UnknownDirection(String),
}

/// Parse a `tomari://v1/...` URL into the [`ExternalAction`] it requests.
///
/// Strict on purpose: anything the grammar does not explicitly allow — a query
/// string, fragment, userinfo, port, an unknown version/verb, or extra path
/// segments — is an error, not silently dropped.
pub fn parse_deep_link(input: &str) -> Result<ExternalAction, DeepLinkError> {
    if input.len() > MAX_URL_LEN {
        return Err(DeepLinkError::TooLong);
    }

    let url = url::Url::parse(input).map_err(|e| DeepLinkError::Malformed(e.to_string()))?;

    if url.scheme() != "tomari" {
        return Err(DeepLinkError::WrongScheme);
    }
    if url.fragment().is_some() {
        return Err(DeepLinkError::DisallowedComponent("a fragment"));
    }
    if url.query().is_some() {
        // No v1 verb takes query arguments yet; reject rather than ignore so a
        // future grammar can introduce them without changing today's meaning.
        return Err(DeepLinkError::DisallowedComponent("a query string"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(DeepLinkError::DisallowedComponent("userinfo"));
    }
    if url.port().is_some() {
        return Err(DeepLinkError::DisallowedComponent("a port"));
    }

    // The version travels as the host: `tomari://v1/...`.
    match url.host_str() {
        Some(API_VERSION) => {}
        Some(other) => return Err(DeepLinkError::UnknownVersion(other.to_string())),
        None => return Err(DeepLinkError::UnknownVersion(String::new())),
    }

    // Tolerate exactly one trailing slash (`.../undo/`) but reject any other
    // empty segment, so `v1//undo` or `snap//left-half` cannot slip past the
    // otherwise-strict grammar.
    let mut segments: Vec<&str> = url
        .path_segments()
        .map(|parts| parts.collect())
        .unwrap_or_default();
    if segments.last() == Some(&"") {
        segments.pop();
    }
    if segments.contains(&"") {
        return Err(DeepLinkError::DisallowedComponent("an empty path segment"));
    }

    match segments.as_slice() {
        ["snap", preset] => WindowPreset::from_kebab(preset)
            .map(ExternalAction::Snap)
            .ok_or_else(|| DeepLinkError::UnknownPreset((*preset).to_string())),
        ["move-display", direction] => DisplayDirection::from_kebab(direction)
            .map(ExternalAction::MoveDisplay)
            .ok_or_else(|| DeepLinkError::UnknownDirection((*direction).to_string())),
        ["undo"] => Ok(ExternalAction::Undo),
        ["toggle-panel"] => Ok(ExternalAction::TogglePanel),

        // Known verbs invoked with the wrong number of arguments get a precise
        // message instead of a generic "unknown command".
        ["snap"] => Err(DeepLinkError::MissingArgument("snap".into())),
        ["move-display"] => Err(DeepLinkError::MissingArgument("move-display".into())),
        ["snap", _, _, ..] => Err(DeepLinkError::UnexpectedArguments("snap".into())),
        ["move-display", _, _, ..] => {
            Err(DeepLinkError::UnexpectedArguments("move-display".into()))
        }
        ["undo", _, ..] => Err(DeepLinkError::UnexpectedArguments("undo".into())),
        ["toggle-panel", _, ..] => Err(DeepLinkError::UnexpectedArguments("toggle-panel".into())),

        [verb, ..] => Err(DeepLinkError::UnknownVerb((*verb).to_string())),
        [] => Err(DeepLinkError::UnknownVerb(String::new())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_snap_preset() {
        for preset in WindowPreset::ALL {
            let url = format!("tomari://v1/snap/{}", preset.as_kebab());
            assert_eq!(
                parse_deep_link(&url),
                Ok(ExternalAction::Snap(preset)),
                "round-trips {url}"
            );
        }
    }

    #[test]
    fn snap_maps_to_the_exact_variant() {
        // The whole point of ExternalAction::Snap: it must never cycle.
        let action: AppAction = parse_deep_link("tomari://v1/snap/left-half")
            .unwrap()
            .into();
        assert_eq!(action, AppAction::SnapWindowExact(WindowPreset::LeftHalf));
    }

    #[test]
    fn parses_move_display_both_directions() {
        assert_eq!(
            parse_deep_link("tomari://v1/move-display/next"),
            Ok(ExternalAction::MoveDisplay(DisplayDirection::Next))
        );
        assert_eq!(
            parse_deep_link("tomari://v1/move-display/prev"),
            Ok(ExternalAction::MoveDisplay(DisplayDirection::Prev))
        );
    }

    #[test]
    fn parses_argument_free_verbs() {
        assert_eq!(
            parse_deep_link("tomari://v1/undo"),
            Ok(ExternalAction::Undo)
        );
        assert_eq!(
            parse_deep_link("tomari://v1/toggle-panel"),
            Ok(ExternalAction::TogglePanel)
        );
    }

    #[test]
    fn tolerates_a_trailing_slash() {
        assert_eq!(
            parse_deep_link("tomari://v1/undo/"),
            Ok(ExternalAction::Undo)
        );
    }

    #[test]
    fn rejects_empty_path_segments() {
        // A trailing slash is fine, but a doubled or leading slash is not.
        for url in ["tomari://v1//undo", "tomari://v1/snap//left-half"] {
            assert_eq!(
                parse_deep_link(url),
                Err(DeepLinkError::DisallowedComponent("an empty path segment")),
                "rejects {url}"
            );
        }
    }

    #[test]
    fn rejects_the_wrong_scheme() {
        assert_eq!(
            parse_deep_link("https://v1/snap/left-half"),
            Err(DeepLinkError::WrongScheme)
        );
    }

    #[test]
    fn rejects_an_unknown_version() {
        assert_eq!(
            parse_deep_link("tomari://v2/undo"),
            Err(DeepLinkError::UnknownVersion("v2".into()))
        );
    }

    #[test]
    fn rejects_an_unknown_verb() {
        assert_eq!(
            parse_deep_link("tomari://v1/launch"),
            Err(DeepLinkError::UnknownVerb("launch".into()))
        );
    }

    #[test]
    fn rejects_an_unknown_preset() {
        assert_eq!(
            parse_deep_link("tomari://v1/snap/middle-ish"),
            Err(DeepLinkError::UnknownPreset("middle-ish".into()))
        );
    }

    #[test]
    fn rejects_an_unknown_direction() {
        assert_eq!(
            parse_deep_link("tomari://v1/move-display/up"),
            Err(DeepLinkError::UnknownDirection("up".into()))
        );
    }

    #[test]
    fn rejects_a_missing_argument() {
        assert_eq!(
            parse_deep_link("tomari://v1/snap"),
            Err(DeepLinkError::MissingArgument("snap".into()))
        );
    }

    #[test]
    fn rejects_extra_path_segments() {
        assert_eq!(
            parse_deep_link("tomari://v1/snap/left-half/again"),
            Err(DeepLinkError::UnexpectedArguments("snap".into()))
        );
        assert_eq!(
            parse_deep_link("tomari://v1/undo/now"),
            Err(DeepLinkError::UnexpectedArguments("undo".into()))
        );
    }

    #[test]
    fn rejects_a_query_string() {
        assert_eq!(
            parse_deep_link("tomari://v1/undo?force=1"),
            Err(DeepLinkError::DisallowedComponent("a query string"))
        );
    }

    #[test]
    fn rejects_a_fragment() {
        assert_eq!(
            parse_deep_link("tomari://v1/undo#frag"),
            Err(DeepLinkError::DisallowedComponent("a fragment"))
        );
    }

    #[test]
    fn rejects_userinfo_and_port() {
        assert_eq!(
            parse_deep_link("tomari://user@v1/undo"),
            Err(DeepLinkError::DisallowedComponent("userinfo"))
        );
        assert_eq!(
            parse_deep_link("tomari://v1:8080/undo"),
            Err(DeepLinkError::DisallowedComponent("a port"))
        );
    }

    #[test]
    fn rejects_an_overlong_url() {
        let long = format!("tomari://v1/snap/{}", "x".repeat(MAX_URL_LEN));
        assert_eq!(parse_deep_link(&long), Err(DeepLinkError::TooLong));
    }

    #[test]
    fn rejects_garbage() {
        assert!(matches!(
            parse_deep_link("not a url"),
            Err(DeepLinkError::Malformed(_))
        ));
    }
}
