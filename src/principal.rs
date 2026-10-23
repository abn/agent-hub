//! Who is calling: a resolved identity.
//!
//! The `actor` on every event comes from here, never from the request body.
//! The stdio transport is local trust; the HTTP transports must present a
//! bearer token.
//!
//! Two audiences resolve differently and must not share a resolver. The MCP
//! transport accepts the admin token or a per-agent token; the REST control
//! surface is the human's, so it accepts only the admin token. Widening one to
//! serve the other would hand agents the control surface.

use axum::http::HeaderMap;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::store::identity;

/// The header an agent declares a name in.
///
/// Without a token on a loopback hub it names the agent itself (ADR 0031).
/// Beside a valid agent token it names a sub-identity of that agent, for
/// attribution only ([`Auth::resolve_agent_as`]).
pub const AGENT_ID_HEADER: &str = "x-agent-id";

/// The longest name a caller may declare beside a token.
const DECLARED_NAME_MAX: usize = 64;

/// Whether `name` is acceptable as a declared sub-identity.
///
/// ASCII letters and digits with `.`, `_` and `-` after the first character,
/// at most 64 of them. There is no `/`, which is what joins the token's agent
/// to the name, and nothing a header or a log line would have to escape.
pub fn is_declared_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        && name.len() <= DECLARED_NAME_MAX
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// The name a request declares in [`AGENT_ID_HEADER`], trimmed.
///
/// A missing or blank header declares nothing. A header that is not text, or
/// that appears more than once, is an error rather than a silent skip, so a
/// caller that meant to declare a name learns it did not.
pub fn declared_name(headers: &HeaderMap) -> Result<Option<&str>> {
    let mut values = headers.get_all(AGENT_ID_HEADER).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(Error::InvalidArgument(
            "X-Agent-Id must be sent once".to_string(),
        ));
    }
    let name = value
        .to_str()
        .map_err(|_| Error::InvalidArgument("X-Agent-Id must be plain text".to_string()))?
        .trim();
    Ok((!name.is_empty()).then_some(name))
}

/// A resolved caller.
#[derive(Debug, Clone)]
pub struct Principal {
    /// The identity recorded as the actor on events.
    pub actor: String,
    /// The agent id, when the caller is an agent rather than the human admin.
    pub agent_id: Option<String>,
    /// Whether the caller is the human admin.
    pub is_admin: bool,
    /// Whether the caller is in pending enrolment.
    pub is_pending: bool,
}

/// Resolves a bearer token to a principal.
pub struct Auth {
    admin_token: Option<String>,
    local_actor: String,
    /// Whether the bind is loopback, which is the only place the hub resolves
    /// a principal without a token.
    loopback: bool,
}

impl Auth {
    /// Build the resolver from configuration.
    pub fn from_config(config: &Config) -> Self {
        let local_actor = std::env::var("HUB_AGENT_ID")
            .ok()
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| "local".to_string());
        Self {
            admin_token: config.admin_token.clone(),
            local_actor,
            loopback: config.bind.ip().is_loopback(),
        }
    }

    /// The principal for the local stdio transport, which needs no token.
    ///
    /// stdio is a process the operator launched on the node, so it is the
    /// human admin: it reaches everything and is attributed to the configured
    /// local actor. Only stdio is treated this way; a token transport is
    /// resolved through [`Auth::resolve_agent`] or [`Auth::require_admin`].
    pub fn local(&self) -> Principal {
        Principal {
            actor: self.local_actor.clone(),
            agent_id: None,
            is_admin: true,
            is_pending: false,
        }
    }

    /// The human admin for a matching admin token, if one is presented.
    fn admin(&self, token: Option<&str>) -> Option<Principal> {
        match (&self.admin_token, token) {
            (Some(expected), Some(presented)) if constant_time_eq(expected, presented) => {
                Some(Principal {
                    actor: "human".to_string(),
                    agent_id: None,
                    is_admin: true,
                    is_pending: false,
                })
            }
            _ => None,
        }
    }

    /// Resolve a token for the MCP transport: the admin token, or a per-agent
    /// token looked up in the identity store. A pending enrolment token is refused
    /// identically to an unrecognised token.
    pub async fn resolve_agent(
        &self,
        db: &turso::Database,
        token: Option<&str>,
    ) -> Result<Principal> {
        if let Some(principal) = self.admin(token) {
            return Ok(principal);
        }
        let presented = token
            .ok_or_else(|| Error::Unauthenticated("a bearer token is required".to_string()))?;
        let hash = identity::hash_token(presented);
        match identity::resolve_token(db, &hash).await? {
            Some((agent_id, state)) if state == "active" => Ok(Principal {
                actor: agent_id.clone(),
                agent_id: Some(agent_id),
                is_admin: false,
                is_pending: false,
            }),
            _ => Err(Error::Unauthenticated(
                "the bearer token is not recognised".to_string(),
            )),
        }
    }

    /// [`Auth::resolve_agent`], then record a declared name as attribution.
    ///
    /// A caller that presents an agent token and declares a name acts as the
    /// sub-identity `<agent>/<name>`: that label is the actor on its events,
    /// sessions and artifacts, and its own owner, cursor and subscription key.
    /// It changes nothing else. Authorization reads `agent_id`, which stays the
    /// token's agent, so the name grants no project, switches no agent and
    /// reaches no session the token's agent could not.
    ///
    /// The name is free-form within [`is_declared_name`] and needs no
    /// registration. The token is resolved first, so a bad token is refused
    /// whatever the header says. The header is not read at all for the admin
    /// token, which acts as the human, and a name equal to the token's own
    /// agent id is that agent. A name that fails the check is refused, and so
    /// is one whose label would be too long to record or would spell an
    /// existing agent's id.
    pub async fn resolve_agent_as(
        &self,
        db: &turso::Database,
        token: Option<&str>,
        headers: &HeaderMap,
    ) -> Result<Principal> {
        let mut principal = self.resolve_agent(db, token).await?;
        let Some(agent_id) = principal.agent_id.as_deref() else {
            return Ok(principal);
        };
        let Some(name) = declared_name(headers)? else {
            return Ok(principal);
        };
        if name == agent_id {
            return Ok(principal);
        }
        if !is_declared_name(name) {
            return Err(Error::InvalidArgument(format!(
                "X-Agent-Id must be 1 to {DECLARED_NAME_MAX} characters: letters and digits, \
                 then letters, digits, '.', '_' or '-'"
            )));
        }
        let label = format!("{agent_id}/{name}");
        if label.chars().count() > crate::store::comments::AUTHOR_CHARS_MAX {
            return Err(Error::InvalidArgument(format!(
                "the agent '{agent_id}' and the name together exceed {} characters",
                crate::store::comments::AUTHOR_CHARS_MAX
            )));
        }
        if identity::get_agent(db, &label).await?.is_some() {
            return Err(Error::Conflict(format!(
                "X-Agent-Id '{name}' would act as the existing agent '{label}'"
            )));
        }
        principal.actor = label;
        Ok(principal)
    }

    /// Resolve a token for the enrolment status route: accepts both active and
    /// pending tokens.
    pub async fn resolve_enrol(
        &self,
        db: &turso::Database,
        token: Option<&str>,
    ) -> Result<Principal> {
        let presented = token
            .ok_or_else(|| Error::Unauthenticated("a bearer token is required".to_string()))?;
        let hash = identity::hash_token(presented);
        match identity::resolve_token(db, &hash).await? {
            Some((agent_id, state)) => Ok(Principal {
                actor: agent_id.clone(),
                agent_id: Some(agent_id),
                is_admin: false,
                is_pending: state == "pending",
            }),
            None => Err(Error::Unauthenticated(
                "the bearer token is not recognised".to_string(),
            )),
        }
    }

    /// Resolve a token for the REST control surface: the admin token only.
    ///
    /// On loopback the control surface stays open without a token, the same way
    /// the local stdio transport is: every process on that machine is already
    /// the operator.
    pub fn require_admin(&self, token: Option<&str>) -> Result<Principal> {
        if let Some(principal) = self.admin(token) {
            return Ok(principal);
        }
        if self.loopback && self.admin_token.is_none() && token.is_none() {
            return Ok(Principal {
                actor: "human".to_string(),
                agent_id: None,
                is_admin: true,
                is_pending: false,
            });
        }
        match (self.admin_token.is_some(), token) {
            (true, _) => Err(Error::Unauthenticated(
                "a valid admin bearer token is required".to_string(),
            )),
            (false, _) => Err(Error::Unauthenticated(
                "no admin token is configured, so the control surface is disabled".to_string(),
            )),
        }
    }
}

/// Compare two tokens without leaking their byte length.
///
/// Both inputs are hashed first, so the comparison runs over two fixed-size
/// digests and their original lengths never take part in the decision.
fn constant_time_eq(expected: &str, presented: &str) -> bool {
    use sha2::{Digest, Sha256};

    let expected = Sha256::digest(expected.as_bytes());
    let presented = Sha256::digest(presented.as_bytes());

    let mut diff = 0u8;
    for (a, b) in expected.iter().zip(presented.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_declared_name_is_a_short_plain_token() {
        for good in [
            "claude",
            "Claude-Code",
            "pi_2",
            "agent.v1",
            "0",
            &"a".repeat(64),
        ] {
            assert!(is_declared_name(good), "{good:?}");
        }
        for bad in [
            "",
            " ",
            "has space",
            "a/b",
            "-lead",
            "_lead",
            ".lead",
            "caf\u{e9}",
            "tab\t",
            &"a".repeat(65),
        ] {
            assert!(!is_declared_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn the_header_is_trimmed_and_a_blank_one_declares_nothing() {
        let mut headers = HeaderMap::new();
        assert_eq!(declared_name(&headers).expect("absent"), None);
        headers.insert(AGENT_ID_HEADER, "  claude ".parse().expect("value"));
        assert_eq!(declared_name(&headers).expect("padded"), Some("claude"));
        headers.insert(AGENT_ID_HEADER, "   ".parse().expect("value"));
        assert_eq!(declared_name(&headers).expect("blank"), None);
    }

    #[test]
    fn a_header_sent_twice_is_refused() {
        let mut headers = HeaderMap::new();
        headers.append(AGENT_ID_HEADER, "a".parse().expect("value"));
        headers.append(AGENT_ID_HEADER, "b".parse().expect("value"));
        assert!(declared_name(&headers).is_err());
    }

    #[test]
    fn a_header_that_is_not_text_is_refused() {
        let mut headers = HeaderMap::new();
        headers.insert(
            AGENT_ID_HEADER,
            axum::http::HeaderValue::from_bytes(&[0xe9]).expect("value"),
        );
        assert!(declared_name(&headers).is_err());
    }
}
