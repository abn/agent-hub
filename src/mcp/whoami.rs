//! The `whoami` answer, shared by the hub and the proxy's local fallback.

use serde::Serialize;

use crate::author::AuthorKind;

/// The session a whoami answer names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WhoAmISession {
    pub id: String,
    pub name: String,
    pub project_id: String,
}

/// The answer for an active credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActiveIdentity {
    pub hub_url: String,
    /// None for the human.
    pub member: Option<String>,
    pub agent: Option<String>,
    pub kind: AuthorKind,
    pub session: Option<WhoAmISession>,
    /// The session's project, else the personal space.
    pub default_project: Option<String>,
    /// `<agent>-<YYYY-MM-DD>`.
    pub suggested_session: String,
    pub personal_project: Option<String>,
    pub admin: bool,
    pub skill_url: String,
    /// The opaque key this caller's rows carry.
    pub actor: String,
}

/// A whoami answer, one shape per credential state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum WhoAmI {
    Active(Box<ActiveIdentity>),
    Pending {
        member: String,
        hub_url: String,
    },
    Anonymous {
        hub_url: String,
        /// Always true: the enrol tool is offered.
        enrol: bool,
    },
}

/// The proxy's local answer when the hub refused or could not be reached:
/// `{"state": <reason or "offline">, "hub_url": ..}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WhoAmIUnreachable {
    /// A credential reason such as `token_revoked`, or `offline`.
    pub state: String,
    pub hub_url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_state_carries_its_own_fields() {
        let pending = WhoAmI::Pending {
            member: "laptop".into(),
            hub_url: "https://hub.example.net".into(),
        };
        assert_eq!(
            serde_json::to_value(pending).expect("pending"),
            serde_json::json!({"state": "pending", "member": "laptop", "hub_url": "https://hub.example.net"})
        );
        let active = WhoAmI::Active(Box::new(ActiveIdentity {
            hub_url: "https://hub.example.net".into(),
            member: Some("laptop".into()),
            agent: Some("claude-code".into()),
            kind: AuthorKind::Participant,
            session: None,
            default_project: Some("homelab".into()),
            suggested_session: "claude-code-2026-11-23".into(),
            personal_project: Some("space-1".into()),
            admin: false,
            skill_url: "https://hub.example.net/bootstrap/SKILL.md".into(),
            actor: "laptop".into(),
        }));
        let value = serde_json::to_value(active).expect("active");
        assert_eq!(value["state"], "active");
        assert_eq!(value["kind"], "participant");
        assert!(value["session"].is_null());
    }
}
