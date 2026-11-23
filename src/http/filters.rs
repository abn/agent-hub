//! The author and session filters the listing routes share: `member`,
//! `agent`, `kind` and `session`.
//!
//! The feed hands them to the store as an [`AuthorFilter`]. A route whose rows
//! the store does not filter by author applies them to the rows it is about to
//! send, by the `author` record every row carries.

use serde_json::Value;

use crate::author::{AuthorFilter, AuthorKind};
use crate::error::{Error, Result};

/// What a caller asked to see, by author and session.
#[derive(Debug, Default, Clone)]
pub struct Params {
    pub member: Option<String>,
    /// A canonical slug.
    pub agent: Option<String>,
    pub kind: Option<AuthorKind>,
    pub session: Option<String>,
}

impl Params {
    /// Take one query pair if it is one of these filters. True when it was.
    pub fn take(&mut self, key: &str, value: &str) -> Result<bool> {
        let value = value.trim();
        match key {
            "member" => self.member = Some(value.to_string()).filter(|v| !v.is_empty()),
            "agent" => {
                self.agent =
                    (!value.is_empty()).then(|| crate::agent_name::canonical(value).to_string());
            }
            "kind" => {
                if !value.is_empty() {
                    self.kind = Some(AuthorKind::parse(value).ok_or_else(|| {
                        Error::InvalidArgument(format!(
                            "unknown author kind '{value}': use human, legacy, participant or local"
                        ))
                    })?);
                }
            }
            "session" | "session_id" => {
                self.session = Some(value.to_string()).filter(|v| !v.is_empty());
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Whether no filter was asked for.
    pub fn is_empty(&self) -> bool {
        self.member.is_none()
            && self.agent.is_none()
            && self.kind.is_none()
            && self.session.is_none()
    }

    /// The author part, for a store that filters by it.
    pub fn author(&self) -> AuthorFilter {
        AuthorFilter {
            member: self.member.clone(),
            agent: self.agent.clone(),
            kind: self.kind,
            label: None,
        }
    }

    /// Whether a row passes: its `author` record and its session. A row with
    /// no `author` passes no author filter.
    pub fn keeps(&self, row: &Value) -> bool {
        let author = row.get("author");
        let field = |name: &str| author.and_then(|a| a.get(name)).and_then(Value::as_str);
        let session_of = |row: &Value| {
            row.get("session_id")
                .and_then(Value::as_str)
                .or_else(|| row.pointer("/session/id").and_then(Value::as_str))
                .or_else(|| row.pointer("/author/session/id").and_then(Value::as_str))
                .map(str::to_string)
        };
        self.member
            .as_deref()
            .is_none_or(|member| field("member") == Some(member))
            && self
                .agent
                .as_deref()
                .is_none_or(|agent| field("agent") == Some(agent))
            && self
                .kind
                .is_none_or(|kind| field("kind") == Some(kind.as_str()))
            && self
                .session
                .as_deref()
                .is_none_or(|session| session_of(row).as_deref() == Some(session))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn params(pairs: &[(&str, &str)]) -> Params {
        let mut params = Params::default();
        for (key, value) in pairs {
            assert!(params.take(key, value).expect("a filter"));
        }
        params
    }

    #[test]
    fn an_agent_is_canonicalised_and_a_bad_kind_is_refused() {
        let found = params(&[("agent", "claude-code")]);
        assert_eq!(found.agent.as_deref(), Some("claude-code"));
        assert!(Params::default().take("kind", "robot").is_err());
        assert!(!Params::default().take("limit", "5").expect("not a filter"));
    }

    #[test]
    fn a_row_passes_when_every_filter_set_matches() {
        let row = json!({
            "author": {"kind": "participant", "member": "laptop", "agent": "claude-code"},
            "session_id": "s1",
        });
        assert!(params(&[("member", "laptop"), ("kind", "participant")]).keeps(&row));
        assert!(params(&[("session", "s1")]).keeps(&row));
        assert!(!params(&[("member", "desktop")]).keeps(&row));
        assert!(!params(&[("kind", "human")]).keeps(&row));
        assert!(!params(&[("member", "laptop")]).keeps(&json!({"id": "x"})));
        assert!(Params::default().keeps(&json!({"id": "x"})));
    }
}
