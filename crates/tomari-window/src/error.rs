//! Errors produced by the window crate.

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("accessibility permission has not been granted")]
    PermissionDenied,

    #[error("no focused window to act on")]
    NoFocusedWindow,

    #[error("accessibility API error (code {0})")]
    Ax(i32),

    #[error("window management is not supported on this platform")]
    Unsupported,

    /// A frame write failed part-way. `step` is the write that failed and
    /// `cause` why; `rollback` says whether the steps already applied were
    /// undone — and, if not, why not — so a caller can tell a window left
    /// exactly where it was from one left moved-but-not-resized.
    #[error("{}", partial_apply_message(step, cause, rollback))]
    PartialApply {
        step: FrameStep,
        cause: Box<Error>,
        rollback: RollbackOutcome,
    },
}

/// One write of the position → size → position sequence a frame is applied
/// as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameStep {
    /// The first position write.
    Origin,
    /// The size write.
    Size,
    /// The second position write, correcting an origin the first left clamped.
    FinalOrigin,
}

impl std::fmt::Display for FrameStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Origin => "position",
            Self::Size => "size",
            Self::FinalOrigin => "final position",
        })
    }
}

/// What became of the writes already applied when a later one failed.
#[derive(Debug, PartialEq)]
pub enum RollbackOutcome {
    /// Nothing had been applied yet; the window is exactly where it was.
    NotNeeded,
    /// The applied writes were undone; the window is back where it was.
    RolledBack,
    /// Undoing failed at `step`: the window is left partially applied.
    Failed { step: FrameStep, cause: Box<Error> },
    /// The pre-move frame could not be read, so there was nothing to undo to.
    NoOriginal,
}

fn partial_apply_message(step: &FrameStep, cause: &Error, rollback: &RollbackOutcome) -> String {
    let rollback = match rollback {
        RollbackOutcome::NotNeeded => "nothing to roll back".to_string(),
        RollbackOutcome::RolledBack => "rolled back".to_string(),
        RollbackOutcome::Failed { step, cause } => {
            format!("rollback failed at {step}: {cause}; the window is left partially moved")
        }
        RollbackOutcome::NoOriginal => {
            "no rollback (the starting frame could not be read)".to_string()
        }
    };
    format!("could not set the window {step}: {cause} ({rollback})")
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
        match self {
            Self::NoFocusedWindow | Self::Ax(AX_INVALID_UI_ELEMENT) => true,
            Self::PartialApply { cause, .. } => cause.window_gone(),
            _ => false,
        }
    }

    /// The Accessibility error at the root of this one — for a partial apply,
    /// the write that failed — so callers keying on a code see through the
    /// wrapper.
    pub fn root(&self) -> &Error {
        match self {
            Self::PartialApply { cause, .. } => cause.root(),
            other => other,
        }
    }

    /// Whether repeating a read may succeed without any state having changed.
    pub fn retryable(&self) -> bool {
        matches!(self.root(), Self::Ax(AX_CANNOT_COMPLETE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_partial_apply_answers_for_its_root_cause() {
        let gone = Error::PartialApply {
            step: FrameStep::Size,
            cause: Box::new(Error::Ax(-25202)),
            rollback: RollbackOutcome::RolledBack,
        };
        assert!(gone.window_gone());
        assert!(!gone.retryable());
        let busy = Error::PartialApply {
            step: FrameStep::FinalOrigin,
            cause: Box::new(Error::Ax(-25204)),
            rollback: RollbackOutcome::Failed {
                step: FrameStep::Origin,
                cause: Box::new(Error::Ax(-25204)),
            },
        };
        assert!(busy.retryable());
        assert_eq!(busy.root(), &Error::Ax(-25204));
        let text = busy.to_string();
        assert!(text.contains("final position"), "{text}");
        assert!(text.contains("rollback failed at position"), "{text}");
    }

    #[test]
    fn only_cannot_complete_is_retryable() {
        assert!(Error::Ax(-25204).retryable());
        assert!(!Error::Ax(-25202).retryable());
        assert!(!Error::NoFocusedWindow.retryable());
    }
}
