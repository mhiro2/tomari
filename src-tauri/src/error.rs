//! The error type returned across the Tauri command boundary.
//!
//! Commands used to reject with a bare English `String`, which the i18n'd UI
//! then showed verbatim. [`CmdError`] instead carries a machine-readable
//! [`ErrorCode`]: the frontend localizes the frequent cases (missing
//! permission, no focused window, shortcut conflict) and falls back to the
//! (developer-facing, English) `message` for the long tail.

use serde::Serialize;

/// A stable classification of a command failure. Only the frequent, actionable
/// cases get their own variant; everything else is [`ErrorCode::Other`], whose
/// `message` is shown as-is.
///
/// Adding a variant is a three-sided change: mirror it in the frontend's
/// `CmdErrorCode` union (`src/lib/types.ts`) and give it en/ja translations
/// (`src/lib/errors.ts` + `src/lib/i18n.tsx`) — otherwise the Japanese UI
/// shows the raw English `message`. The wire-string test below and the
/// frontend's compile-time exhaustiveness check both fail until the mirror
/// is complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    /// Accessibility permission is required (window control or keystroke synthesis).
    PermissionRequired,
    /// There is no focused window to act on.
    NoFocusedWindow,
    /// A global shortcut could not be registered — typically a conflict with
    /// another app.
    ShortcutConflict,
    /// The focused application has no remembered position to restore.
    PlacementNotFound,
    /// The panel still described a different focused window when an action was
    /// requested; the UI should refresh instead of applying it elsewhere.
    WindowTargetChanged,
    /// The target application's Accessibility server did not answer the
    /// message, even after the safe read-only retry.
    WindowNotResponding,
    /// Anything else; `message` carries the detail.
    Other,
}

/// An error returned from a `#[tauri::command]`. Serializes to
/// `{ "code": "...", "message": "..." }` so the frontend can translate the
/// known `code`s and fall back to `message` otherwise.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CmdError {
    pub code: ErrorCode,
    pub message: String,
}

impl CmdError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// An uncategorized error whose `message` the UI shows verbatim.
    pub fn other(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Other, message)
    }

    pub fn permission_required(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::PermissionRequired, message)
    }

    pub fn shortcut_conflict(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ShortcutConflict, message)
    }

    pub fn placement_not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::PlacementNotFound, message)
    }

    pub fn window_target_changed(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::WindowTargetChanged, message)
    }
}

impl std::fmt::Display for CmdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CmdError {}

impl From<String> for CmdError {
    fn from(message: String) -> Self {
        Self::other(message)
    }
}

impl From<&str> for CmdError {
    fn from(message: &str) -> Self {
        Self::other(message)
    }
}

impl From<tomari_core::Error> for CmdError {
    fn from(e: tomari_core::Error) -> Self {
        Self::other(e.to_string())
    }
}

impl From<tomari_window::Error> for CmdError {
    fn from(e: tomari_window::Error) -> Self {
        use tomari_window::Error;
        let code = match &e {
            Error::PermissionDenied => ErrorCode::PermissionRequired,
            Error::NoFocusedWindow => ErrorCode::NoFocusedWindow,
            Error::Ax(-25204) => ErrorCode::WindowNotResponding,
            _ => ErrorCode::Other,
        };
        Self::new(code, e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire string each code serializes to — the contract the frontend's
    /// `CmdErrorCode` union mirrors. An exhaustive match, so a new variant
    /// fails to compile here until it is added (and thereby to the mirror
    /// checklist in the enum's doc comment).
    fn wire_string(code: ErrorCode) -> &'static str {
        match code {
            ErrorCode::PermissionRequired => "permissionRequired",
            ErrorCode::NoFocusedWindow => "noFocusedWindow",
            ErrorCode::ShortcutConflict => "shortcutConflict",
            ErrorCode::PlacementNotFound => "placementNotFound",
            ErrorCode::WindowTargetChanged => "windowTargetChanged",
            ErrorCode::WindowNotResponding => "windowNotResponding",
            ErrorCode::Other => "other",
        }
    }

    /// Yields every variant as a chain starting from `None`. Unlike a plain
    /// array (which the compiler never re-checks), the match is exhaustive
    /// over the *previous* variant, so adding a variant fails to compile
    /// until it is linked into the chain — which is exactly what feeds it to
    /// the serialization assertion below.
    fn next_code(after: Option<ErrorCode>) -> Option<ErrorCode> {
        match after {
            None => Some(ErrorCode::PermissionRequired),
            Some(ErrorCode::PermissionRequired) => Some(ErrorCode::NoFocusedWindow),
            Some(ErrorCode::NoFocusedWindow) => Some(ErrorCode::ShortcutConflict),
            Some(ErrorCode::ShortcutConflict) => Some(ErrorCode::PlacementNotFound),
            Some(ErrorCode::PlacementNotFound) => Some(ErrorCode::WindowTargetChanged),
            Some(ErrorCode::WindowTargetChanged) => Some(ErrorCode::WindowNotResponding),
            Some(ErrorCode::WindowNotResponding) => Some(ErrorCode::Other),
            Some(ErrorCode::Other) => None,
        }
    }

    #[test]
    fn error_codes_serialize_to_the_frontend_contract() {
        let mut visited = 0;
        let mut code = next_code(None);
        while let Some(current) = code {
            let json = serde_json::to_string(&current).unwrap();
            assert_eq!(json, format!("\"{}\"", wire_string(current)));
            visited += 1;
            code = next_code(Some(current));
        }
        // The exact count catches a chain that skips a variant (a lazily
        // added `=> None` arm would otherwise hide the new variant from the
        // loop above).
        assert_eq!(visited, 7, "the chain must visit every variant once");
    }

    #[test]
    fn ax_cannot_complete_becomes_an_actionable_window_error() {
        let error = CmdError::from(tomari_window::Error::Ax(-25204));
        assert_eq!(error.code, ErrorCode::WindowNotResponding);
    }
}
