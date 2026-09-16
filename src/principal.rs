//! Who is calling: a resolved identity and its trust level.
//!
//! The `actor` on every event comes from here, never from the request body.
//! The stdio transport is local trust; the HTTP transport must present a
//! bearer token. Until full identity management lands, only the configured
//! admin token is accepted.

use crate::config::{Config, TrustDefault};
use crate::error::{Error, Result};

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
}

/// Resolves a bearer token to a principal.
pub struct Auth {
    admin_token: Option<String>,
    trust_default: TrustDefault,
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
            trust_default: config.trust_default,
            local_actor,
        }
    }

    /// The principal for the local stdio transport, which needs no token.
    pub fn local(&self) -> Principal {
        Principal {
            actor: self.local_actor.clone(),
            trust: trust_level(self.trust_default),
        }
    }

    /// Resolve an HTTP bearer token, if one is configured and matches.
    pub fn resolve_bearer(&self, token: Option<&str>) -> Result<Principal> {
        match (&self.admin_token, token) {
            (Some(expected), Some(presented)) if constant_time_eq(expected, presented) => {
                Ok(Principal {
                    actor: "human".to_string(),
                    trust: Trust::Trusted,
                })
            }
            (Some(_), _) => Err(Error::Unauthenticated(
                "a valid bearer token is required".to_string(),
            )),
            (None, _) => Err(Error::Unauthenticated(
                "no admin token is configured, so the HTTP transport is disabled".to_string(),
            )),
        }
    }
}

fn trust_level(default: TrustDefault) -> Trust {
    match default {
        TrustDefault::Trusted => Trust::Trusted,
        TrustDefault::Untrusted => Trust::Untrusted,
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
