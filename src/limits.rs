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

/// Maximum bytes of a request body on the agent transport.
///
/// An artifact reaches the hub as a JSON string argument, so the transport has
/// to carry the artifact cap plus the escaping that JSON adds to it and the
/// rest of the call around it. What a tool then accepts stays with the tool's
/// own check; this only keeps the transport from refusing first.
pub const AGENT_BODY_BYTES_MAX: usize =
    ARTIFACT_BYTES_MAX + ARTIFACT_BYTES_MAX / 8 + REQUEST_BODY_BYTES_MAX;

/// Maximum bytes of one session brain value.
///
/// The agent transport is sized to carry an artifact, so it does not bound a
/// brain value; this does. A brain holds working state, not documents, and
/// keeping a value under the REST body ceiling also keeps a brain file inside
/// its own budget for any sane number of entries.
pub const BRAIN_VALUE_BYTES_MAX: usize = REQUEST_BODY_BYTES_MAX;

/// Maximum feed page size.
pub const FEED_LIMIT_MAX: i64 = 500;

/// Maximum rows a confined search reads before it stops, bounding the scan
/// while still reaching deeper than a page of visible hits.
pub const SEARCH_FETCH_MAX: i64 = 5000;

/// Default feed page size.
pub const FEED_LIMIT_DEFAULT: i64 = 50;

/// Ceilings on the open action items an agent may leave on the human.
///
/// An open item is one that still waits on the human: the inbox status is
/// `action` or `waiting`. A resolved item frees its slot. A cap of zero
/// disables that check, which is the operator's valve to turn the guard off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InboxCaps {
    /// Open items one actor may hold in one project.
    pub per_actor: i64,
    /// Open items every actor together may hold in one project.
    pub per_project: i64,
}

impl InboxCaps {
    /// Default open items one actor may hold in one project.
    pub const DEFAULT_PER_ACTOR: i64 = 100;
    /// Default open items every actor together may hold in one project.
    pub const DEFAULT_PER_PROJECT: i64 = 1000;

    /// Caps that enforce nothing.
    pub const fn disabled() -> Self {
        Self {
            per_actor: 0,
            per_project: 0,
        }
    }

    /// Whether either check is live.
    pub fn enabled(&self) -> bool {
        self.per_actor > 0 || self.per_project > 0
    }

    /// Parse both caps from their raw environment values.
    ///
    /// Absent or empty values take the defaults; a negative value or one that
    /// is not a whole number is a startup error, so a typo fails loudly rather
    /// than silently disabling the guard.
    pub fn parse(per_actor: Option<&str>, per_project: Option<&str>) -> Result<Self> {
        Ok(Self {
            per_actor: parse_cap(
                "HUB_INBOX_ACTION_PER_AGENT",
                per_actor,
                Self::DEFAULT_PER_ACTOR,
            )?,
            per_project: parse_cap(
                "HUB_INBOX_ACTION_PER_PROJECT",
                per_project,
                Self::DEFAULT_PER_PROJECT,
            )?,
        })
    }
}

fn parse_cap(name: &str, value: Option<&str>, default: i64) -> Result<i64> {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return Ok(default);
    };
    let cap = value
        .parse::<i64>()
        .map_err(|_| Error::Config(format!("{name} is not a whole number: {value}")))?;
    if cap < 0 {
        return Err(Error::Config(format!(
            "{name} must be zero or more, got {cap}"
        )));
    }
    Ok(cap)
}

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

/// Reject a session brain value over the cap.
pub fn check_brain_value(bytes: usize) -> Result<()> {
    if bytes > BRAIN_VALUE_BYTES_MAX {
        return Err(Error::PayloadTooLarge(format!(
            "brain value exceeds {BRAIN_VALUE_BYTES_MAX} bytes"
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
