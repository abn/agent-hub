//! The process error type and result alias.

use thiserror::Error;

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
}

/// Convenience result alias.
pub type Result<T> = std::result::Result<T, Error>;
