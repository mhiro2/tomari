//! Errors produced by the keyboard crate.

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("invalid accelerator `{input}`: {reason}")]
    InvalidAccelerator { input: String, reason: String },
}

impl Error {
    pub(crate) fn invalid(input: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidAccelerator {
            input: input.into(),
            reason: reason.into(),
        }
    }
}
