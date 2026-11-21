//! Error mapping for `libghostty-vt` result codes.

use thiserror::Error;

use crate::ffi;

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum VtError {
    #[error("out of memory")]
    OutOfMemory,
    #[error("invalid value")]
    InvalidValue,
    #[error("out of space")]
    OutOfSpace,
    #[error("no value")]
    NoValue,
    #[error("i/o error")]
    IoError,
    #[error("limit exceeded")]
    LimitExceeded,
    #[error("rejected by safety check")]
    Rejected,
    #[error("unsupported by this libghostty-vt build")]
    Unsupported,
    #[error("unknown libghostty-vt error ({0})")]
    Unknown(i32),
}

impl VtError {
    pub fn from_ffi(code: ffi::GhosttyResult) -> Self {
        match code {
            ffi::GHOSTTY_OUT_OF_MEMORY => Self::OutOfMemory,
            ffi::GHOSTTY_INVALID_VALUE => Self::InvalidValue,
            ffi::GHOSTTY_OUT_OF_SPACE => Self::OutOfSpace,
            ffi::GHOSTTY_NO_VALUE => Self::NoValue,
            ffi::GHOSTTY_IO_ERROR => Self::IoError,
            ffi::GHOSTTY_LIMIT_EXCEEDED => Self::LimitExceeded,
            ffi::GHOSTTY_REJECTED => Self::Rejected,
            ffi::GHOSTTY_UNSUPPORTED => Self::Unsupported,
            other => Self::Unknown(other),
        }
    }
}

#[inline]
pub fn check(code: ffi::GhosttyResult) -> Result<(), VtError> {
    if code == ffi::GHOSTTY_SUCCESS { Ok(()) } else { Err(VtError::from_ffi(code)) }
}
