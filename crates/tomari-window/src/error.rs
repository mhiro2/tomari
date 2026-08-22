//! Errors produced by the window crate.

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("accessibility permission has not been granted")]
    PermissionDenied,

    #[error("no focused window to act on")]
    NoFocusedWindow,

    #[error("accessibility API error (code {0})")]
    Ax(i32),

    #[error("window management is not supported on this platform")]
    Unsupported,
}

/// `kAXErrorInvalidUIElement`: the AX element is no longer valid, i.e. the
/// window (or its application) is gone.
const AX_INVALID_UI_ELEMENT: i32 = -25202;
/// `kAXErrorCannotComplete`: the AX server could not finish a message, most
/// commonly because the target application did not answer before the timeout.
const AX_CANNOT_COMPLETE: i32 = -25204;

impl Error {
    /// Whether this error means the target window no longer exists, as opposed
    /// to a transient failure that may succeed on retry.
    pub fn window_gone(&self) -> bool {
        matches!(
            self,
            Self::NoFocusedWindow | Self::Ax(AX_INVALID_UI_ELEMENT)
        )
    }

    /// Whether repeating a read may succeed without any state having changed.
    pub fn retryable(&self) -> bool {
        matches!(self, Self::Ax(AX_CANNOT_COMPLETE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_cannot_complete_is_retryable() {
        assert!(Error::Ax(-25204).retryable());
        assert!(!Error::Ax(-25202).retryable());
        assert!(!Error::NoFocusedWindow.retryable());
    }
}
