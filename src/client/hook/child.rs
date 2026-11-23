//! A subagent's hub session: its name, and the exclusive start that finds the
//! name free.
//!
//! The hub is the authority on names. A new child is started exclusively under
//! the parent, and a name that is already held is refused, so the next one is
//! tried. A name that exists is never reused, since it may belong to another
//! subagent, live or ended. A child the hook has already mapped resumes its own
//! name instead.

use serde_json::Value;

use super::hub::{Conn, reason};
use super::resolve::slug;
use super::state::Child;
use crate::client::Failure;

/// The reason the hub gives for a name that is already held.
const NAME_TAKEN: &str = "session_name_taken";

/// How many letters and digits of the harness's id a child name keeps.
const SHORT_ID_LEN: usize = 8;
/// How long the type segment of a child name may be.
const TYPE_MAX: usize = 24;
/// How long one segment of a session name may be.
const SEGMENT_MAX: usize = 64;

/// The segment of a child name that says what kind of subagent it is: the
/// type, slugged and cut to 24 characters.
pub fn type_slug(agent_type: Option<&str>) -> String {
    let slug: String = slug(agent_type.unwrap_or_default())
        .chars()
        .take(TYPE_MAX)
        .collect();
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "agent".to_string()
    } else {
        slug.to_string()
    }
}

/// The first 8 letters and digits of the harness's id for a subagent.
pub fn short_id(agent_id: &str) -> String {
    agent_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(SHORT_ID_LEN)
        .collect()
}

/// The names a child may try, in order: the plain one, then `-2` to `-9`, then
/// the type with the whole harness id.
pub fn candidates(agent_type: Option<&str>, agent_id: &str) -> Vec<String> {
    let kind = type_slug(agent_type);
    let short = short_id(agent_id);
    let base = if short.is_empty() {
        kind.clone()
    } else {
        format!("{kind}-{short}")
    };
    let mut names = vec![base.clone()];
    names.extend((2..=9).map(|n| format!("{base}-{n}")));
    let room = SEGMENT_MAX.saturating_sub(kind.len() + 1);
    let whole: String = slug(agent_id).chars().take(room).collect();
    let whole = whole.trim_end_matches('-');
    if !whole.is_empty() {
        names.push(format!("{kind}-{whole}"));
    }
    names
}

/// Whatever starts a session by name: the hook's connection, or a recording.
pub trait Starter {
    /// `session_start` for `session_name`, exclusive or not.
    fn start(
        &mut self,
        session_name: &str,
        exclusive: bool,
    ) -> impl std::future::Future<Output = Result<Value, Failure>>;
}

/// A hub connection that starts sessions in one project.
pub struct OnConn<'a> {
    pub conn: &'a mut Conn,
    pub project_id: &'a str,
}

impl Starter for OnConn<'_> {
    async fn start(&mut self, session_name: &str, exclusive: bool) -> Result<Value, Failure> {
        self.conn
            .call(
                "session_start",
                serde_json::json!({
                    "project_id": self.project_id,
                    "session_name": session_name,
                    "exclusive": exclusive,
                }),
            )
            .await
    }
}

fn child_of(name: String, started: &Value) -> Child {
    Child {
        name,
        session_id: started
            .get("session_id")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

/// Start a new child under `parent`, on the first name the hub finds free.
pub async fn start_new(
    starter: &mut impl Starter,
    parent: &str,
    agent_type: Option<&str>,
    agent_id: &str,
) -> Result<Child, Failure> {
    for candidate in candidates(agent_type, agent_id) {
        let name = format!("{parent}/{candidate}");
        match starter.start(&name, true).await {
            Ok(started) => return Ok(child_of(name, &started)),
            Err(failure) if reason(&failure) == Some(NAME_TAKEN) => {}
            Err(failure) => return Err(failure),
        }
    }
    Err(Failure::Failed(
        "every child session name is taken".to_string(),
    ))
}

/// Start a child the hook has mapped before again, under its own name, which
/// the hub finds or reopens.
pub async fn resume(starter: &mut impl Starter, known: &Child) -> Result<Child, Failure> {
    let started = starter.start(&known.name, false).await?;
    Ok(child_of(known.name.clone(), &started))
}
