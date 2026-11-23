//! Identity tools: report who the caller is, and enrol a caller that is not a
//! member yet.
//!
//! The personal space comes from the identity store; the human and an open
//! loopback hub have none of their own to report beyond the member they act
//! as.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ErrorData};
use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, tool, tool_router};
use serde::Deserialize;
use serde_json::json;

use crate::error::{CredentialReason, Error};
use crate::principal::CredentialState;
use crate::store::identity;

use super::session_arg::Mode;
use super::whoami::{ActiveIdentity, WhoAmI, WhoAmISession};
use super::{HubServer, SessionOnlyParams, to_error_data};

/// The longest `reason` an enrolment may carry, as on the REST route.
const REASON_CHARS_MAX: usize = 200;

/// Arguments for `enrol`.
#[derive(Debug, Deserialize, JsonSchema)]
struct EnrolParams {
    /// The member id to ask for: lowercase letters, digits, `.`, `_` and `-`.
    /// Omitted, the hub derives one from `host`, else from your agent.
    #[serde(default)]
    member: Option<String>,
    /// A name a person recognises, such as "Arun's laptop".
    #[serde(default)]
    display_name: Option<String>,
    /// Why you want to join: one line, 200 characters at most.
    reason: String,
    /// The operating system you run on, for the operator's approval screen.
    #[serde(default)]
    os: Option<String>,
    /// The machine you run on, for the operator's approval screen.
    #[serde(default)]
    host: Option<String>,
}

#[tool_router(router = identity_router, vis = "pub")]
impl HubServer {
    #[tool(
        description = "Report who the caller is: its member, agent and kind, the session it named, the project and session name to start with, its personal space, whether it is the admin, and the URL of the agent guide. The discovery call: it proves the token resolves and points an agent at the guide. A caller with no credential gets state \"anonymous\" and may call enrol; one waiting for approval gets state \"pending\"."
    )]
    async fn whoami(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<SessionOnlyParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let hub_url = self.hub_url(&context);
        let answer = match principal.credential {
            CredentialState::Anonymous => WhoAmI::Anonymous {
                hub_url,
                enrol: true,
            },
            CredentialState::Pending => WhoAmI::Pending {
                member: principal.member().unwrap_or_default().to_string(),
                hub_url,
            },
            CredentialState::Active => {
                let resolved = self
                    .resolve_session(
                        &context,
                        &principal,
                        params.session.as_ref(),
                        None,
                        Mode::Read,
                    )
                    .await
                    .map_err(to_error_data)?;
                let session = resolved.session.as_ref().map(|session| WhoAmISession {
                    id: session.id.clone(),
                    name: session.session_name.clone(),
                    project_id: session.project_id.clone(),
                });
                let personal_project = match principal.member() {
                    Some(member) => identity::get_member(&self.state.db, member)
                        .await
                        .map_err(to_error_data)?
                        .map(|member| member.personal_project_id),
                    None => None,
                };
                let default_project = session
                    .as_ref()
                    .map(|session| session.project_id.clone())
                    .or_else(|| personal_project.clone());
                let today = time::OffsetDateTime::now_utc().date();
                WhoAmI::Active(Box::new(ActiveIdentity {
                    skill_url: format!("{hub_url}/bootstrap/SKILL.md"),
                    hub_url,
                    member: principal.member().map(str::to_string),
                    agent: principal.member().map(|_| principal.agent().to_string()),
                    kind: principal.kind(),
                    session,
                    default_project,
                    suggested_session: crate::agent_name::suggested_session(
                        principal.agent(),
                        today,
                    ),
                    personal_project,
                    admin: principal.is_admin(),
                    actor: principal.writer(None).actor().to_string(),
                }))
            }
        };
        Ok(CallToolResult::structured(
            serde_json::to_value(answer)
                .map_err(|err| to_error_data(Error::Config(err.to_string())))?,
        ))
    }

    #[tool(
        description = "Ask to join this hub. For a caller that has no credential: it records a pending member and returns a pending token. The operator approves it in the app; until then whoami with that token says pending. Save the url, the member and the token under [[credential]] in ~/.config/agent-hub/config.toml, or set HUB_URL and HUB_TOKEN, then reconnect. Approval may take hours."
    )]
    async fn enrol(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<EnrolParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        if principal.credential == CredentialState::Pending {
            return Err(to_error_data(Error::Credential(
                CredentialReason::TokenPending,
                format!(
                    "'{}' is already waiting for approval",
                    principal.member().unwrap_or_default()
                ),
            )));
        }
        if !self.state.config.enrol_enabled {
            return Err(to_error_data(Error::Credential(
                CredentialReason::EnrolDisabled,
                "enrolment is off on this hub; ask the operator for a token".to_string(),
            )));
        }
        let reason = params.reason.trim();
        if reason.is_empty() {
            return Err(to_error_data(Error::InvalidArgument(
                "reason must be non-empty".to_string(),
            )));
        }
        if reason.contains(['\n', '\r']) {
            return Err(to_error_data(Error::InvalidArgument(
                "reason must not contain newlines".to_string(),
            )));
        }
        if reason.chars().count() > REASON_CHARS_MAX {
            return Err(to_error_data(Error::InvalidArgument(format!(
                "reason must be at most {REASON_CHARS_MAX} characters"
            ))));
        }
        let host = params
            .host
            .as_deref()
            .map(str::trim)
            .filter(|h| !h.is_empty());
        let os = params
            .os
            .as_deref()
            .map(str::trim)
            .filter(|o| !o.is_empty());
        let member = match params
            .member
            .as_deref()
            .map(str::trim)
            .filter(|member| !member.is_empty())
        {
            Some(member) => member.to_string(),
            None => derive_member(host, principal.agent()),
        };
        let display_name = params
            .display_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map_or_else(|| member.clone(), str::to_string);
        let hub_url = self.hub_url(&context);
        let source = self.peer_source(&context);
        let (enrolled, issued) = identity::enrol_member(
            &self.state.db,
            &identity::Enrolment {
                member: &member,
                display_name: &display_name,
                source: &source,
                why: reason,
                agent: principal.agent(),
                os,
                host,
            },
            self.state.config.enrol_pending_max,
        )
        .await
        .map_err(to_error_data)?;
        self.state.notify_waiting();
        Ok(CallToolResult::structured(json!({
            "status": "pending",
            "member": enrolled.id,
            "token": issued.token,
            "status_url": format!("{hub_url}/api/v1/enrol/status"),
            "store": "Save url, member and token under [[credential]] in \
                      ~/.config/agent-hub/config.toml, or set HUB_URL and HUB_TOKEN, then \
                      reconnect. Approval may take hours.",
        })))
    }
}

impl HubServer {
    /// The origin the caller reached the hub by.
    fn hub_url(&self, context: &RequestContext<RoleServer>) -> String {
        crate::http::origin::request_origin(
            &self.state.config,
            context
                .extensions
                .get::<axum::http::request::Parts>()
                .map(|parts| &parts.headers)
                .unwrap_or(&axum::http::HeaderMap::new()),
        )
    }

    /// Who an enrolment is from: the socket peer, and the forwarded client
    /// only when that peer is a configured trusted proxy.
    fn peer_source(&self, context: &RequestContext<RoleServer>) -> String {
        let Some(parts) = context.extensions.get::<axum::http::request::Parts>() else {
            return "local".to_string();
        };
        let Some(peer) = parts
            .extensions
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|connect| connect.0)
        else {
            return "unknown".to_string();
        };
        if self.state.config.trusted_proxies.contains(&peer.ip()) {
            let forwarded = parts
                .headers
                .get("x-forwarded-for")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.rsplit(',').next())
                .or_else(|| {
                    parts
                        .headers
                        .get("x-real-ip")
                        .and_then(|value| value.to_str().ok())
                })
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if let Some(client) = forwarded {
                return client.to_string();
            }
        }
        peer.ip().to_string()
    }
}

/// The member id to ask for when the caller named none: the `host` as a slug,
/// else the agent with four random hex digits.
fn derive_member(host: Option<&str>, agent: &str) -> String {
    let slug = |text: &str| {
        let mut out = String::new();
        for c in text.chars() {
            if c.is_ascii_alphanumeric() {
                out.push(c.to_ascii_lowercase());
            } else if matches!(c, '-' | '_' | '.' | ' ') && !out.ends_with('-') && !out.is_empty() {
                out.push('-');
            }
        }
        out.trim_matches('-').chars().take(40).collect::<String>()
    };
    let from_host = host.map(slug).filter(|slug| !slug.is_empty());
    match from_host {
        Some(slug) => slug,
        None => {
            let id = crate::store::next_id();
            let tail: String = id
                .chars()
                .rev()
                .filter(char::is_ascii_alphanumeric)
                .take(4)
                .collect::<String>()
                .to_ascii_lowercase();
            let agent = slug(agent);
            let agent = if agent.is_empty() {
                "agent".to_string()
            } else {
                agent
            };
            format!("{agent}-{tail}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::derive_member;

    #[test]
    fn a_member_is_derived_from_the_host_or_the_agent() {
        assert_eq!(
            derive_member(Some("Arun's Laptop"), "claude-code"),
            "aruns-laptop"
        );
        let from_agent = derive_member(None, "claude-code");
        assert!(from_agent.starts_with("claude-code-"), "{from_agent}");
        assert_eq!(from_agent.len(), "claude-code-".len() + 4);
        assert!(crate::store::identity::valid_new_member_id(&from_agent));
    }
}
