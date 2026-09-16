//! Size limits shared by the stores and the surfaces.

use crate::error::{Error, Result};

/// Maximum bytes of a serialised event payload.
pub const EVENT_PAYLOAD_BYTES_MAX: usize = 256 * 1024;

/// Maximum characters of an event summary.
pub const EVENT_SUMMARY_CHARS_MAX: usize = 1024;

/// Maximum bytes of an HTTP request body.
pub const REQUEST_BODY_BYTES_MAX: usize = 4 * 1024 * 1024;

/// Maximum bytes of an artifact blob.
pub const ARTIFACT_BYTES_MAX: usize = 50 * 1024 * 1024;

/// Maximum feed page size.
pub const FEED_LIMIT_MAX: i64 = 500;

/// Default feed page size.
pub const FEED_LIMIT_DEFAULT: i64 = 50;

/// Reject an event whose summary or payload is over the cap.
pub fn check_event(summary: &str, payload_bytes: usize) -> Result<()> {
    if summary.chars().count() > EVENT_SUMMARY_CHARS_MAX {
        return Err(Error::PayloadTooLarge(format!(
            "event summary exceeds {EVENT_SUMMARY_CHARS_MAX} characters"
        )));
    }
    if payload_bytes > EVENT_PAYLOAD_BYTES_MAX {
        return Err(Error::PayloadTooLarge(format!(
            "event payload exceeds {EVENT_PAYLOAD_BYTES_MAX} bytes"
        )));
    }
    Ok(())
}

/// Reject an artifact blob over the cap.
pub fn check_artifact(bytes: usize) -> Result<()> {
    if bytes > ARTIFACT_BYTES_MAX {
        return Err(Error::PayloadTooLarge(format!(
            "artifact exceeds {ARTIFACT_BYTES_MAX} bytes"
        )));
    }
    Ok(())
}
