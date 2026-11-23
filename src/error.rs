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

/// Why a credential was refused: the `details.reason` of an `unauthenticated`
/// (or, for enrolment, `forbidden`) error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialReason {
    /// No bearer on a networked hub.
    TokenMissing,
    /// The token's member is waiting for approval.
    TokenPending,
    /// The token was revoked or replaced by a reissue.
    TokenRevoked,
    /// The token is not one this hub issued.
    TokenUnknown,
    /// Self-enrolment is off on this hub.
    EnrolDisabled,
}

impl CredentialReason {
    /// The `details.reason` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TokenMissing => "token_missing",
            Self::TokenPending => "token_pending",
            Self::TokenRevoked => "token_revoked",
            Self::TokenUnknown => "token_unknown",
            Self::EnrolDisabled => "enrol_disabled",
        }
    }

    /// The code the reason travels under.
    pub fn code(self) -> ErrorCode {
        match self {
            Self::EnrolDisabled => ErrorCode::Forbidden,
            _ => ErrorCode::Unauthenticated,
        }
    }
}

/// Why a call naming a session was refused: the `details.reason` of the error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionReason {
    /// `{}` or `{project_id}` alone.
    Invalid,
    /// `session_id` given with a name or project that differs from the row.
    Mismatch,
    /// No session has that id, directly or through an alias.
    Unknown,
    /// A bare name matches more than one live session the caller is in.
    Ambiguous,
    /// A `store: "session"` write named no session.
    Required,
    /// A child name whose parent is missing or not live.
    ParentMissing,
    /// A new name with a UUID or ULID segment.
    NameOpaque,
    /// The session has ended.
    Ended,
    /// The session was pruned.
    Pruned,
    /// An exclusive start found the name already held.
    NameTaken,
    /// A brain write whose base is not the key's current version.
    StaleBase,
}

impl SessionReason {
    /// The `details.reason` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Invalid => "session_invalid",
            Self::Mismatch => "session_mismatch",
            Self::Unknown => "session_unknown",
            Self::Ambiguous => "session_ambiguous",
            Self::Required => "session_required",
            Self::ParentMissing => "session_parent_missing",
            Self::NameOpaque => "session_name_opaque",
            Self::Ended => "session_ended",
            Self::Pruned => "session_pruned",
            Self::NameTaken => "session_name_taken",
            Self::StaleBase => "stale_base",
        }
    }

    /// The code the reason travels under.
    pub fn code(self) -> ErrorCode {
        match self {
            Self::Unknown => ErrorCode::NotFound,
            Self::Ended | Self::Pruned | Self::NameTaken | Self::StaleBase => ErrorCode::Conflict,
            Self::Invalid
            | Self::Mismatch
            | Self::Ambiguous
            | Self::Required
            | Self::ParentMissing
            | Self::NameOpaque => ErrorCode::InvalidArgument,
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
    /// A credential was refused, with the reason a client acts on.
    #[error("{1}")]
    Credential(CredentialReason, String),
    /// A call naming a session was refused. The map carries extra details
    /// beside the reason, such as `session_id` or `version`.
    #[error("{1}")]
    Session(
        SessionReason,
        String,
        Option<serde_json::Map<String, serde_json::Value>>,
    ),
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
            Self::Credential(reason, _) => reason.code(),
            Self::Session(reason, _, _) => reason.code(),
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

    /// A machine-readable reason a caller can act on, where the code is too broad.
    pub fn reason(&self) -> Option<&'static str> {
        match self {
            Self::Credential(reason, _) => Some(reason.as_str()),
            Self::Session(reason, _, _) => Some(reason.as_str()),
            _ => None,
        }
    }

    /// The `details` object both surfaces send: `{"reason": .., ..extra}`, or
    /// none when the error carries no reason. MCP puts it in the error's
    /// `data.details`; REST puts it in the problem body's `details`.
    pub fn details(&self) -> Option<serde_json::Value> {
        let reason = self.reason()?;
        let mut details = match self {
            Self::Session(_, _, Some(extra)) => extra.clone(),
            _ => serde_json::Map::new(),
        };
        details.insert("reason".to_string(), serde_json::Value::from(reason));
        Some(serde_json::Value::Object(details))
    }

    /// Whether a caller may sensibly retry the same request.
    pub fn retryable(&self) -> bool {
        // A cap refusal clears only when the human resolves or prunes an item,
        // which the server cannot time, so a blind retry is not sensible.
        matches!(self.code(), ErrorCode::Unavailable)
    }

    /// Whether this error was caused by a database lock held by another process.
    pub fn is_locked(&self) -> bool {
        match self {
            Self::Engine(msg) => {
                let lower = msg.to_ascii_lowercase();
                lower.contains("locked by another process")
                    || lower.contains("locking error")
                    || lower.contains("database is locked")
            }
            _ => false,
        }
    }
}

/// Convenience result alias.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reasoned_error_carries_its_code_and_details() {
        let taken = Error::Session(
            SessionReason::NameTaken,
            "session 'feat-x' exists".to_string(),
            Some(serde_json::Map::from_iter([(
                "session_id".to_string(),
                serde_json::Value::from("01J"),
            )])),
        );
        assert_eq!(taken.code(), ErrorCode::Conflict);
        assert_eq!(
            taken.details(),
            Some(serde_json::json!({"reason": "session_name_taken", "session_id": "01J"}))
        );
        let revoked = Error::Credential(CredentialReason::TokenRevoked, "revoked".to_string());
        assert_eq!(revoked.code(), ErrorCode::Unauthenticated);
        assert_eq!(
            revoked.details(),
            Some(serde_json::json!({"reason": "token_revoked"}))
        );
        assert_eq!(Error::NotFound("x".to_string()).details(), None);
    }
}
