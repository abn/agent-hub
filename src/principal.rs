//! Who is calling: a resolved identity and its trust level.
//!
//! The `actor` on every event comes from here, never from the request body.
//! The stdio transport is local trust; the HTTP transports must present a
//! bearer token.
//!
//! Two audiences resolve differently and must not share a resolver. The MCP
//! transport accepts the admin token or a per-agent token; the REST control
//! surface is the human's, so it accepts only the admin token. Widening one to
//! serve the other would hand agents the control surface.

use crate::config::Config;
use crate::error::{Error, Result};
use crate::store::identity;

/// Trust level of a principal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    Trusted,
    Untrusted,
}

/// A resolved caller.
#[derive(Debug, Clone)]
pub struct Principal {
    /// The identity recorded as the actor on events.
    pub actor: String,
    /// Whether the caller is trusted.
    pub trust: Trust,
    /// The agent id, when the caller is an agent rather than the human admin.
    pub agent_id: Option<String>,
    /// Whether the caller is the human admin.
    pub is_admin: bool,
}

/// Resolves a bearer token to a principal.
pub struct Auth {
    admin_token: Option<String>,
    local_actor: String,
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
            trust: Trust::Trusted,
            agent_id: None,
            is_admin: true,
        }
    }

    /// The human admin for a matching admin token, if one is presented.
    fn admin(&self, token: Option<&str>) -> Option<Principal> {
        match (&self.admin_token, token) {
            (Some(expected), Some(presented)) if constant_time_eq(expected, presented) => {
                Some(Principal {
                    actor: "human".to_string(),
                    trust: Trust::Trusted,
                    agent_id: None,
                    is_admin: true,
                })
            }
            _ => None,
        }
    }

    /// Resolve a token for the MCP transport: the admin token, or a per-agent
    /// token looked up in the identity store.
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
            Some((agent_id, trust)) => Ok(Principal {
                actor: agent_id.clone(),
                trust,
                agent_id: Some(agent_id),
                is_admin: false,
            }),
            None => Err(Error::Unauthenticated(
                "the bearer token is not recognised".to_string(),
            )),
        }
    }

    /// Resolve a token for the REST control surface: the admin token only.
    pub fn require_admin(&self, token: Option<&str>) -> Result<Principal> {
        if let Some(principal) = self.admin(token) {
            return Ok(principal);
        }
        match self.admin_token {
            Some(_) => Err(Error::Unauthenticated(
                "a valid admin bearer token is required".to_string(),
            )),
            None => Err(Error::Unauthenticated(
                "no admin token is configured, so the control surface is disabled".to_string(),
            )),
        }
    }
}

/// Compare two tokens without an early exit on the first differing byte.
fn constant_time_eq(expected: &str, presented: &str) -> bool {
    let (expected, presented) = (expected.as_bytes(), presented.as_bytes());
    if expected.len() != presented.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in expected.iter().zip(presented) {
        diff |= a ^ b;
    }
    diff == 0
}
