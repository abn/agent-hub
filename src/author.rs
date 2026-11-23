//! Who wrote a row: the typed author record and where the write came from.
//!
//! Every write-bearing row carries one [`Author`] and one [`Origin`]. The
//! `actor` text column keeps an opaque key ([`Author::key`]) so readers that
//! group or match on it see one stable string; it is never displayed and never
//! parsed back into a member and an agent.

use serde::{Deserialize, Serialize};

/// The member id embedded stdio writes under. It is a reserved id, so no
/// enrolled member can hold it.
pub const LOCAL_MEMBER: &str = "local";

/// The `actor` key a human write carries.
pub const HUMAN_KEY: &str = "human";

/// The `actor` key the hub's own acts carry, kept so old readers see the same
/// key on question expiry.
pub const HUB_KEY: &str = "hub";

/// The four author kinds, as the `author_kind` column stores them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthorKind {
    Participant,
    Human,
    Local,
    Legacy,
}

impl AuthorKind {
    /// The column and wire value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Participant => "participant",
            Self::Human => "human",
            Self::Local => "local",
            Self::Legacy => "legacy",
        }
    }

    /// Parse a column or filter value.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "participant" => Some(Self::Participant),
            "human" => Some(Self::Human),
            "local" => Some(Self::Local),
            "legacy" => Some(Self::Legacy),
            _ => None,
        }
    }
}

/// The author of a write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Author {
    /// A member's agent, optionally inside a session.
    Participant {
        member: String,
        agent: String,
        session: Option<String>,
    },
    /// The operator, through the PWA, the REST surface or the admin token.
    Human,
    /// Embedded stdio, the local administrator.
    Local {
        agent: String,
        session: Option<String>,
    },
    /// A row written before members existed, with the label it was written
    /// under.
    Legacy { label: String },
}

impl Author {
    /// The opaque key the `actor` column stores.
    pub fn key(&self) -> &str {
        match self {
            Self::Participant { member, .. } => member,
            Self::Human => HUMAN_KEY,
            Self::Local { .. } => LOCAL_MEMBER,
            Self::Legacy { label } => label,
        }
    }

    /// The `author_kind` column value.
    pub fn kind(&self) -> AuthorKind {
        match self {
            Self::Participant { .. } => AuthorKind::Participant,
            Self::Human => AuthorKind::Human,
            Self::Local { .. } => AuthorKind::Local,
            Self::Legacy { .. } => AuthorKind::Legacy,
        }
    }

    /// The member column: the member for a participant, `local` for embedded
    /// stdio, none otherwise.
    pub fn member(&self) -> Option<&str> {
        match self {
            Self::Participant { member, .. } => Some(member),
            Self::Local { .. } => Some(LOCAL_MEMBER),
            Self::Human | Self::Legacy { .. } => None,
        }
    }

    /// The agent column.
    pub fn agent(&self) -> Option<&str> {
        match self {
            Self::Participant { agent, .. } | Self::Local { agent, .. } => Some(agent),
            Self::Human | Self::Legacy { .. } => None,
        }
    }

    /// The session the write happened in, when the author carries one.
    pub fn session(&self) -> Option<&str> {
        match self {
            Self::Participant { session, .. } | Self::Local { session, .. } => session.as_deref(),
            Self::Human | Self::Legacy { .. } => None,
        }
    }

    /// The delivery key of a participant or local author.
    pub fn participant(&self) -> Option<ParticipantKey> {
        match self {
            Self::Participant { member, agent, .. } => Some(ParticipantKey {
                member: member.clone(),
                agent: agent.clone(),
            }),
            Self::Local { agent, .. } => Some(ParticipantKey {
                member: LOCAL_MEMBER.to_string(),
                agent: agent.clone(),
            }),
            Self::Human | Self::Legacy { .. } => None,
        }
    }

    /// The same author placed in another session, or in none.
    pub fn in_session(self, session_id: Option<String>) -> Self {
        match self {
            Self::Participant { member, agent, .. } => Self::Participant {
                member,
                agent,
                session: session_id,
            },
            Self::Local { agent, .. } => Self::Local {
                agent,
                session: session_id,
            },
            other => other,
        }
    }

    /// Rebuild an author from its stored columns.
    ///
    /// An unknown or missing kind reads as legacy, with the `actor` column as
    /// the label, so a row no release understood still renders.
    pub fn from_columns(
        kind: Option<&str>,
        member: Option<&str>,
        agent: Option<&str>,
        session: Option<&str>,
        actor: &str,
    ) -> Self {
        let session = session.map(str::to_string);
        match kind.and_then(AuthorKind::parse) {
            Some(AuthorKind::Participant) => Self::Participant {
                member: member.unwrap_or(actor).to_string(),
                agent: agent.unwrap_or_default().to_string(),
                session,
            },
            Some(AuthorKind::Human) => Self::Human,
            Some(AuthorKind::Local) => Self::Local {
                agent: agent.unwrap_or_default().to_string(),
                session,
            },
            Some(AuthorKind::Legacy) | None => Self::Legacy {
                label: actor.to_string(),
            },
        }
    }

    /// The wire record, given the session's name when the author carries one.
    pub fn view(&self, session_name: Option<&str>) -> AuthorView {
        let session = self.session().map(|id| SessionLabel {
            id: id.to_string(),
            name: session_name.map(str::to_string),
        });
        match self {
            Self::Participant { member, agent, .. } => AuthorView {
                kind: AuthorKind::Participant,
                member: Some(member.clone()),
                agent: Some(agent.clone()),
                session,
                label: None,
            },
            Self::Human => AuthorView {
                kind: AuthorKind::Human,
                member: None,
                agent: None,
                session: None,
                label: None,
            },
            Self::Local { agent, .. } => AuthorView {
                kind: AuthorKind::Local,
                member: None,
                agent: Some(agent.clone()),
                session,
                label: None,
            },
            Self::Legacy { label } => AuthorView {
                kind: AuthorKind::Legacy,
                member: None,
                agent: None,
                session: None,
                label: Some(label.clone()),
            },
        }
    }
}

/// Whether a write is a caller's or the hub's own act.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Origin {
    /// A caller made the write. Stored as NULL.
    #[default]
    Call,
    /// The hub acted on the operator's policy, such as question expiry.
    Hub,
}

impl Origin {
    /// The `origin` column and wire value: NULL for a call.
    pub fn as_column(self) -> Option<&'static str> {
        match self {
            Self::Call => None,
            Self::Hub => Some(HUB_KEY),
        }
    }

    /// Parse the column.
    pub fn from_column(value: Option<&str>) -> Self {
        match value {
            Some(HUB_KEY) => Self::Hub,
            _ => Self::Call,
        }
    }
}

/// Everything a store write records about who made it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Writer {
    pub author: Author,
    pub origin: Origin,
}

impl Writer {
    /// A write a caller made.
    pub fn call(author: Author) -> Self {
        Self {
            author,
            origin: Origin::Call,
        }
    }

    /// A write the human made.
    pub fn human() -> Self {
        Self::call(Author::Human)
    }

    /// The hub's own act on the operator's behalf.
    pub fn hub() -> Self {
        Self {
            author: Author::Human,
            origin: Origin::Hub,
        }
    }

    /// The `actor` column: [`Author::key`], except `hub` for the hub's acts.
    pub fn actor(&self) -> &str {
        match (&self.author, self.origin) {
            (Author::Human, Origin::Hub) => HUB_KEY,
            (author, _) => author.key(),
        }
    }
}

/// Transitional: a bare actor label is a legacy writer. Lets the store take
/// `impl Into<Writer>` while callers still pass labels; it goes once every
/// caller passes a writer.
impl From<&str> for Writer {
    fn from(label: &str) -> Self {
        if label == HUMAN_KEY {
            return Self::human();
        }
        Self::call(Author::Legacy {
            label: label.to_string(),
        })
    }
}

/// Transitional, as `From<&str>`.
impl From<&String> for Writer {
    fn from(label: &String) -> Self {
        Self::from(label.as_str())
    }
}

impl From<&Writer> for Writer {
    fn from(writer: &Writer) -> Self {
        writer.clone()
    }
}

/// The delivery key of one participant: cursors and subscriptions are keyed
/// by it. Agent `''` is a migrated participant whose agent was not recorded.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ParticipantKey {
    pub member: String,
    pub agent: String,
}

impl ParticipantKey {
    /// Transitional: a bare label keys as a member with no agent. It goes
    /// with the last caller that keys by label.
    pub fn from_label(label: &str) -> Self {
        Self {
            member: label.to_string(),
            agent: String::new(),
        }
    }

    /// Whether an audience entry matches this key: an entry with agent `''`
    /// matches every agent of its member.
    pub fn matches(&self, entry: &ParticipantKey) -> bool {
        self.member == entry.member && (entry.agent.is_empty() || entry.agent == self.agent)
    }
}

/// A filter over authors: the `member`, `agent` and `kind` query parameters,
/// and a subscription's `author_filter` column (`{"member","agent"}`,
/// `{"kind":"human"}`, or `{"label"}` for an unplaced legacy filter). Every
/// field set must match; an empty filter matches every author.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
    /// A canonical slug; the caller canonicalises what it was given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<AuthorKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl AuthorFilter {
    /// Whether the filter matches every author.
    pub fn is_empty(&self) -> bool {
        self.member.is_none() && self.agent.is_none() && self.kind.is_none() && self.label.is_none()
    }

    /// Whether an author passes the filter.
    pub fn matches(&self, author: &Author) -> bool {
        self.member
            .as_deref()
            .is_none_or(|member| author.member() == Some(member))
            && self
                .agent
                .as_deref()
                .is_none_or(|agent| author.agent() == Some(agent))
            && self.kind.is_none_or(|kind| author.kind() == kind)
            && self.label.as_deref().is_none_or(
                |label| matches!(author, Author::Legacy { label: held } if held == label),
            )
    }
}

/// A session as the wire names it beside an author.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLabel {
    pub id: String,
    pub name: Option<String>,
}

/// The `author` record on every wire row.
///
/// `{"kind":"participant","member":..,"agent":..,"session":{id,name}|null}`,
/// `{"kind":"human"}`, `{"kind":"local","agent":..,"session":..}`,
/// `{"kind":"legacy","label":..}`. An old artifact version with no author is
/// `{"kind":"legacy","label":null}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AuthorView {
    pub kind: AuthorKind,
    #[serde(default)]
    pub member: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub session: Option<SessionLabel>,
    #[serde(default)]
    pub label: Option<String>,
}

impl AuthorView {
    /// The record an artifact version written before authors were kept shows.
    pub fn unrecorded() -> Self {
        Self {
            kind: AuthorKind::Legacy,
            member: None,
            agent: None,
            session: None,
            label: None,
        }
    }
}

impl Serialize for AuthorView {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("kind", &self.kind)?;
        match self.kind {
            AuthorKind::Participant => {
                map.serialize_entry("member", &self.member)?;
                map.serialize_entry("agent", &self.agent)?;
                map.serialize_entry("session", &self.session)?;
            }
            AuthorKind::Local => {
                map.serialize_entry("agent", &self.agent)?;
                map.serialize_entry("session", &self.session)?;
            }
            AuthorKind::Legacy => map.serialize_entry("label", &self.label)?,
            AuthorKind::Human => {}
        }
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_actor_key_is_the_member_or_the_reserved_word() {
        let participant = Author::Participant {
            member: "laptop".into(),
            agent: "claude-code".into(),
            session: None,
        };
        assert_eq!(participant.key(), "laptop");
        assert_eq!(Writer::human().actor(), "human");
        assert_eq!(Writer::hub().actor(), "hub");
        assert_eq!(
            Writer::call(Author::Local {
                agent: "cli".into(),
                session: None
            })
            .actor(),
            "local"
        );
    }

    #[test]
    fn columns_round_trip_and_unknown_kinds_read_as_legacy() {
        let author = Author::Participant {
            member: "laptop".into(),
            agent: "opencode".into(),
            session: Some("01J".into()),
        };
        let back = Author::from_columns(
            Some(author.kind().as_str()),
            author.member(),
            author.agent(),
            author.session(),
            author.key(),
        );
        assert_eq!(back, author);
        assert_eq!(
            Author::from_columns(Some("robot"), None, None, None, "laptop/claude"),
            Author::Legacy {
                label: "laptop/claude".into()
            }
        );
    }

    #[test]
    fn a_migrated_audience_entry_matches_every_agent_of_its_member() {
        let key = ParticipantKey {
            member: "laptop".into(),
            agent: "claude-code".into(),
        };
        assert!(key.matches(&ParticipantKey::from_label("laptop")));
        assert!(!key.matches(&ParticipantKey {
            member: "laptop".into(),
            agent: "opencode".into()
        }));
    }

    #[test]
    fn each_kind_writes_only_its_own_fields() {
        let human = serde_json::to_value(Author::Human.view(None)).expect("human");
        assert_eq!(human, serde_json::json!({"kind": "human"}));
        let local = serde_json::to_value(
            Author::Local {
                agent: "claude-code".into(),
                session: None,
            }
            .view(None),
        )
        .expect("local");
        assert_eq!(
            local,
            serde_json::json!({"kind": "local", "agent": "claude-code", "session": null})
        );
        let legacy = serde_json::to_value(AuthorView::unrecorded()).expect("legacy");
        assert_eq!(legacy, serde_json::json!({"kind": "legacy", "label": null}));
    }

    #[test]
    fn a_filter_matches_on_every_field_it_sets() {
        let author = Author::Participant {
            member: "laptop".into(),
            agent: "claude-code".into(),
            session: None,
        };
        assert!(AuthorFilter::default().matches(&author));
        let by_member = AuthorFilter {
            member: Some("laptop".into()),
            ..AuthorFilter::default()
        };
        assert!(by_member.matches(&author));
        let other_agent = AuthorFilter {
            agent: Some("opencode".into()),
            ..by_member.clone()
        };
        assert!(!other_agent.matches(&author));
        let legacy = AuthorFilter {
            label: Some("stray".into()),
            ..AuthorFilter::default()
        };
        assert!(legacy.matches(&Author::Legacy {
            label: "stray".into()
        }));
        assert!(!legacy.matches(&author));
    }

    #[test]
    fn origin_is_null_for_a_call() {
        assert_eq!(Origin::Call.as_column(), None);
        assert_eq!(Origin::from_column(Some("hub")), Origin::Hub);
    }
}
