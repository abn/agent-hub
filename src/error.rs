//! The process error type, its machine-readable codes, and the result alias.

use thiserror::Error;

/// Machine-readable error codes shared by the MCP tools and the REST API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    InvalidArgument,
    Unauthenticated,
    Forbidden,
    NotFound,
    Conflict,
    PayloadTooLarge,
    RateLimited,
    Unavailable,
    Internal,
}

impl ErrorCode {
    /// The wire value of the code.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArgument => "invalid_argument",
            Self::Unauthenticated => "unauthenticated",
            Self::Forbidden => "forbidden",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::PayloadTooLarge => "payload_too_large",
            Self::RateLimited => "rate_limited",
            Self::Unavailable => "unavailable",
            Self::Internal => "internal",
        }
    }
}

/// Errors that can stop or degrade the hub.
#[derive(Debug, Error)]
pub enum Error {
    /// Bad or missing configuration.
    #[error("configuration error: {0}")]
    Config(String),
    /// The storage engine failed.
    #[error("engine error: {0}")]
    Engine(String),
    /// The filesystem failed.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// A caller supplied something the hub rejected.
    #[error("{0}")]
    InvalidArgument(String),
    /// A caller is not authenticated.
    #[error("{0}")]
    Unauthenticated(String),
    /// A caller is authenticated but not allowed.
    #[error("{0}")]
    Forbidden(String),
    /// The target does not exist.
    #[error("{0}")]
    NotFound(String),
    /// The request conflicts with current state.
    #[error("{0}")]
    Conflict(String),
    /// The request body or payload exceeds a limit.
    #[error("{0}")]
    PayloadTooLarge(String),
    /// A write was refused because it would push a count past its cap.
    #[error("{0}")]
    RateLimited(String),
    /// The hub cannot serve the request yet, though the request is well formed.
    #[error("{0}")]
    Unavailable(String),
}

impl Error {
    /// The machine-readable code for this error.
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::InvalidArgument(_) => ErrorCode::InvalidArgument,
            Self::Unauthenticated(_) => ErrorCode::Unauthenticated,
            Self::Forbidden(_) => ErrorCode::Forbidden,
            Self::NotFound(_) => ErrorCode::NotFound,
            Self::Conflict(_) => ErrorCode::Conflict,
            Self::PayloadTooLarge(_) => ErrorCode::PayloadTooLarge,
            Self::RateLimited(_) => ErrorCode::RateLimited,
            Self::Unavailable(_) | Self::Engine(_) => ErrorCode::Unavailable,
            Self::Config(_) | Self::Io(_) => ErrorCode::Internal,
        }
    }

    /// Whether a caller may sensibly retry the same request.
    pub fn retryable(&self) -> bool {
        // A cap refusal clears only when the human resolves or prunes an item,
        // which the server cannot time, so a blind retry is not sensible.
        matches!(self.code(), ErrorCode::Unavailable)
    }
}

/// Convenience result alias.
pub type Result<T> = std::result::Result<T, Error>;
