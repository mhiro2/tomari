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
            _ => ErrorCode::Other,
        };
        Self::new(code, e.to_string())
    }
}
