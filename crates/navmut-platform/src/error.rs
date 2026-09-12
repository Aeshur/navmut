use std::io;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PlatformError {
    #[error("{0} is unavailable on this platform")]
    Unsupported(&'static str),
    #[error("invalid {field}: {reason}")]
    Invalid { field: &'static str, reason: String },
    #[error("game connection failed: {0}")]
    Windows(String),
    #[error("helper process failed: {0}")]
    Helper(String),
    #[error("helper response outcome is unknown: {0}")]
    Unknown(String),
    #[error("bridge request failed: {0}")]
    Bridge(String),
    #[error("I/O failed: {0}")]
    Io(#[from] io::Error),
}

impl PlatformError {
    pub(crate) fn invalid(field: &'static str, reason: impl Into<String>) -> Self {
        Self::Invalid {
            field,
            reason: reason.into(),
        }
    }
}
