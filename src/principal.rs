//! Who is calling: a resolved identity.
//!
//! The `actor` on every event comes from here, never from the request body.
//! The stdio transport is local trust; the HTTP transports must present a
//! bearer token.
//!
//! Two audiences resolve differently and must not share a resolver. The MCP
//! transport accepts the admin token or a member token; the REST control
//! surface is the human's, so it accepts only the admin token. Widening one to
//! serve the other would hand members the control surface.
//!
//! A request that arrives over a paired phone's Tendril session carries no
//! token: the session is the credential. The phone transport runs the request
//! inside [`as_phone`], and every resolver here answers it as the operator on
//! that phone. Nothing a network client sends can set that scope.

use axum::http::HeaderMap;

use crate::config::Config;
use crate::error::{CredentialReason, Error, Result};
use crate::store::identity;

/// The header a caller names its agent in.
///
/// The value is canonicalised to one slug per client ([`crate::agent_name`])
/// and is the agent of the caller's writes, beside the member its token names.
/// It names no member, grants nothing and is ignored for the admin token.
pub const AGENT_ID_HEADER: &str = "x-agent-id";

/// The longest name a caller may declare in the header.
const DECLARED_NAME_MAX: usize = 64;

/// Whether `name` is acceptable as a declared agent name.
///
/// ASCII letters and digits with `.`, `_` and `-` after the first character,
/// at most 64 of them: nothing a header or a log line would have to escape.
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

tokio::task_local! {
    /// The paired phone the request in flight came from. Set only by the
    /// phone transport, around one request.
    static PHONE: String;
}

/// Run `request` as the operator on the phone `device_id`.
pub async fn as_phone<F: std::future::Future>(device_id: String, request: F) -> F::Output {
    PHONE.scope(device_id, request).await
}

/// The phone the request in flight came from, when it came from one.
fn phone() -> Option<String> {
    PHONE.try_with(Clone::clone).ok()
}

/// Whether the request in flight came over a paired phone's session.
pub fn on_phone() -> bool {
    PHONE.try_with(|_| ()).is_ok()
}

/// A resolved caller.
#[derive(Debug, Clone)]
pub struct Principal {
    /// Whether the caller is the human admin.
    pub is_admin: bool,
    /// The member the caller acts as: the token's member, `local` for embedded
    /// stdio, `loopback` on the open loopback hub, none for the human.
    pub member: Option<String>,
    /// The canonical agent slug, `cli` when the caller named none.
    pub agent: String,
    /// The author kind every write by this caller records.
    pub kind: crate::author::AuthorKind,
    /// Where the credential stands.
    pub credential: CredentialState,
    /// The hash of the bearer presented, for closing its streams on
    /// revocation.
    pub token_hash: Option<String>,
    /// The paired phone the operator called from, when they did.
    pub device: Option<String>,
}

/// Where a caller's credential stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialState {
    /// An active member token, the admin token, embedded stdio, or the open
    /// loopback hub.
    Active,
    /// A pending enrolment token: the caller sees the enrol surface only.
    Pending,
    /// No bearer on a networked hub: the caller sees the enrol surface only.
    Anonymous,
}

/// The identity a caller writes under, as every layer outside this module and
/// `src/mcp` reads it. Code there uses only these methods, never the fields,
/// so the fields can change without moving the store or the REST handlers.
impl Principal {
    /// A member with a token the hub knows.
    pub fn for_member(member: &str, token_hash: &str, credential: CredentialState) -> Self {
        Self {
            is_admin: false,
            member: Some(member.to_string()),
            agent: crate::agent_name::CLI.to_string(),
            kind: crate::author::AuthorKind::Participant,
            credential,
            token_hash: Some(token_hash.to_string()),
            device: None,
        }
    }

    /// A member on the open loopback hub, which has no token.
    pub fn for_loopback(member: &str) -> Self {
        Self {
            token_hash: None,
            ..Self::for_member(member, "", CredentialState::Active)
        }
    }

    /// The human admin: the admin token, or the REST surface on an open
    /// loopback hub.
    pub fn human() -> Self {
        Self {
            is_admin: true,
            member: None,
            agent: crate::agent_name::CLI.to_string(),
            kind: crate::author::AuthorKind::Human,
            credential: CredentialState::Active,
            token_hash: None,
            device: None,
        }
    }

    /// The operator on one of their paired phones: the human, as the PWA
    /// is, with the phone named for the log. The phone transport keeps the
    /// operator's own controls from it before a handler runs.
    pub fn operator_phone(device_id: &str) -> Self {
        Self {
            device: Some(device_id.to_string()),
            ..Self::human()
        }
    }

    /// Embedded stdio: the local administrator.
    pub fn local() -> Self {
        Self {
            is_admin: true,
            member: Some(crate::author::LOCAL_MEMBER.to_string()),
            agent: crate::agent_name::CLI.to_string(),
            kind: crate::author::AuthorKind::Local,
            credential: CredentialState::Active,
            token_hash: None,
            device: None,
        }
    }

    /// A caller with no bearer on a networked hub: it may only enrol.
    pub fn anonymous() -> Self {
        Self {
            is_admin: false,
            member: None,
            agent: crate::agent_name::CLI.to_string(),
            kind: crate::author::AuthorKind::Human,
            credential: CredentialState::Anonymous,
            token_hash: None,
            device: None,
        }
    }

    /// The same caller, named by its agent. The slug is already canonical.
    pub fn with_agent(mut self, agent: &str) -> Self {
        self.agent = agent.to_string();
        self
    }

    /// The member the caller acts as: the token's member, `local` for embedded
    /// stdio, `loopback` on the open loopback hub, none for the human.
    pub fn member(&self) -> Option<&str> {
        self.member.as_deref()
    }

    /// The canonical agent slug the caller writes under.
    pub fn agent(&self) -> &str {
        &self.agent
    }

    /// The author kind every write by this caller records.
    pub fn kind(&self) -> crate::author::AuthorKind {
        self.kind
    }

    /// Where the caller's credential stands.
    pub fn credential(&self) -> CredentialState {
        self.credential
    }

    /// The hash of the bearer the caller presented, for closing its streams on
    /// revocation. None for the admin, stdio and the open loopback hub.
    pub fn token_hash(&self) -> Option<&str> {
        self.token_hash.as_deref()
    }

    /// Whether the caller is the operator.
    pub fn is_admin(&self) -> bool {
        self.is_admin
    }

    /// The paired phone the operator called from, when they did.
    pub fn device(&self) -> Option<&str> {
        self.device.as_deref()
    }

    /// Refuse a caller whose credential is not active: a pending member and a
    /// caller with no bearer reach nothing but the enrol surface.
    pub fn require_active(&self) -> Result<()> {
        match self.credential {
            CredentialState::Active => Ok(()),
            CredentialState::Pending => Err(Error::Credential(
                CredentialReason::TokenPending,
                "this member is waiting for approval".to_string(),
            )),
            CredentialState::Anonymous => Err(Error::Credential(
                CredentialReason::TokenMissing,
                "a bearer token is required".to_string(),
            )),
        }
    }

    /// The writer for one write, in the session the call resolved (or none).
    ///
    /// The store records `writer.actor()` in the `actor` column and the author
    /// record beside it.
    pub fn writer(&self, session_id: Option<&str>) -> crate::author::Writer {
        use crate::author::{Author, Writer};
        let session = session_id.map(str::to_string);
        match (self.kind, self.member.as_deref()) {
            (crate::author::AuthorKind::Participant, Some(member)) => {
                Writer::call(Author::Participant {
                    member: member.to_string(),
                    agent: self.agent.clone(),
                    session,
                })
            }
            (crate::author::AuthorKind::Local, _) => Writer::call(Author::Local {
                agent: self.agent.clone(),
                session,
            }),
            _ => Writer::human(),
        }
    }

    /// The delivery key of the caller: cursors and subscriptions use it. None
    /// for the human, who reads the PWA rather than cursors.
    pub fn participant(&self) -> Option<crate::author::ParticipantKey> {
        match self.kind {
            crate::author::AuthorKind::Participant | crate::author::AuthorKind::Local => self
                .member
                .as_ref()
                .map(|member| crate::author::ParticipantKey {
                    member: member.clone(),
                    agent: self.agent.clone(),
                }),
            _ => None,
        }
    }
}

/// Resolves a bearer token to a principal.
pub struct Auth {
    admin_token: Option<String>,
    /// Whether the bind is loopback, which is the only place the hub resolves
    /// a principal without a token.
    loopback: bool,
}

impl Auth {
    /// Build the resolver from configuration.
    pub fn from_config(config: &Config) -> Self {
        Self {
            admin_token: config.admin_token.clone(),
            loopback: config.bind.ip().is_loopback(),
        }
    }

    /// The principal for the local stdio transport, which needs no token.
    ///
    /// stdio is a process the operator launched on the node, so it is the
    /// local administrator: it reaches everything and writes as `local`. Only
    /// stdio is treated this way; a token transport is resolved through
    /// [`Auth::resolve_agent`] or [`Auth::require_admin`].
    pub fn local(&self) -> Principal {
        Principal::local()
    }

    /// The human admin for a matching admin token, if one is presented, or
    /// the operator on a paired phone for a request the phone transport runs.
    fn admin(&self, token: Option<&str>) -> Option<Principal> {
        if let Some(device) = phone() {
            return Some(Principal::operator_phone(&device));
        }
        match (&self.admin_token, token) {
            (Some(expected), Some(presented)) if constant_time_eq(expected, presented) => {
                Some(Principal::human())
            }
            _ => None,
        }
    }

    /// Resolve a token for the MCP transport: the admin token, or a member
    /// token looked up in the identity store.
    ///
    /// A pending member's token is a principal in the pending state: it may
    /// enrol and ask who it is, nothing else. A revoked or replaced token and
    /// an unknown one are refused with their own reason.
    pub async fn resolve_agent(
        &self,
        db: &turso::Database,
        token: Option<&str>,
    ) -> Result<Principal> {
        if let Some(principal) = self.admin(token) {
            return Ok(principal);
        }
        let Some(presented) = token else {
            return Ok(Principal::anonymous());
        };
        let hash = identity::hash_token(presented);
        match identity::resolve_member_token(db, &hash).await? {
            Some(resolved) => match resolved.state {
                identity::TokenState::Active => Ok(Principal::for_member(
                    &resolved.member,
                    &resolved.token_hash,
                    CredentialState::Active,
                )),
                identity::TokenState::Pending => Ok(Principal::for_member(
                    &resolved.member,
                    &resolved.token_hash,
                    CredentialState::Pending,
                )),
                identity::TokenState::Revoked => Err(Error::Credential(
                    CredentialReason::TokenRevoked,
                    "this member's token was revoked or replaced".to_string(),
                )),
            },
            None => Err(Error::Credential(
                CredentialReason::TokenUnknown,
                "the bearer token is not recognised".to_string(),
            )),
        }
    }

    /// [`Auth::resolve_agent`] for a surface that needs a member or the admin,
    /// with the agent the request names in [`AGENT_ID_HEADER`], else `cli`.
    ///
    /// The header is not read for the admin token, which acts as the human. A
    /// request with no token, or with a pending member's, is refused here.
    pub async fn resolve_member_as(
        &self,
        db: &turso::Database,
        token: Option<&str>,
        headers: &HeaderMap,
    ) -> Result<Principal> {
        let principal = self.resolve_agent(db, token).await?;
        principal.require_active()?;
        if principal.kind != crate::author::AuthorKind::Participant {
            return Ok(principal);
        }
        let agent = agent_from_header(headers)?;
        Ok(match agent {
            Some(agent) => principal.with_agent(&agent),
            None => principal,
        })
    }

    /// Resolve a token for the enrolment status route: accepts both active and
    /// pending tokens.
    pub async fn resolve_enrol(
        &self,
        db: &turso::Database,
        token: Option<&str>,
    ) -> Result<Principal> {
        let principal = self.resolve_agent(db, token).await?;
        if principal.credential == CredentialState::Anonymous {
            return Err(Error::Credential(
                CredentialReason::TokenMissing,
                "a bearer token is required".to_string(),
            ));
        }
        Ok(principal)
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
            return Ok(Principal::human());
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

/// The agent a request declares in [`AGENT_ID_HEADER`], canonicalised.
///
/// None when the header is absent or blank. A name that is not a plain token
/// is refused, so a caller that meant to name its agent learns it did not.
pub fn agent_from_header(headers: &HeaderMap) -> Result<Option<String>> {
    let Some(name) = declared_name(headers)? else {
        return Ok(None);
    };
    if !is_declared_name(name) {
        return Err(Error::InvalidArgument(format!(
            "X-Agent-Id must be 1 to {DECLARED_NAME_MAX} characters: letters and digits, \
             then letters, digits, '.', '_' or '-'"
        )));
    }
    Ok(Some(crate::agent_name::canonical(name)))
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

    #[tokio::test]
    async fn only_the_phone_transport_makes_a_caller_the_operator_on_a_phone() {
        let auth = Auth {
            admin_token: Some("secret".to_string()),
            loopback: false,
        };
        assert!(auth.require_admin(None).is_err(), "no token, no phone");
        let on_phone = as_phone("dev_0011223344556677".to_string(), async {
            auth.require_admin(None).expect("the phone is the operator")
        })
        .await;
        assert!(on_phone.is_admin());
        assert_eq!(on_phone.device(), Some("dev_0011223344556677"));
        let browser = auth.require_admin(Some("secret")).expect("admin token");
        assert_eq!(browser.device(), None);
        assert!(
            auth.require_admin(None).is_err(),
            "the scope ends with the request"
        );
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
