//! The session-start bundle: resolve, brief, bind, read the index, subscribe,
//! and render the `<agent-hub>` block.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::hub::{CLOSE_BUDGET, Conn, Hub, STARTUP_BUDGET, dedupe};
use super::render::{Anchor, ContextInput, Item, ago, context_block, notice_line, sanitize};
use super::resolve::{ResolveInput, Resolved, gather_git, read_top_level_file, resolve_context};
use crate::client::Failure;

/// The longest all of git's questions may take together.
const GIT_BUDGET: Duration = Duration::from_secs(4);

/// What a started session hands the hook.
#[derive(Debug)]
pub struct Started {
    pub block: String,
    pub project_id: String,
    pub session_name: String,
    pub session_id: Option<String>,
    pub actor: String,
}

/// What one bundle run needs to know about its caller.
pub struct Request<'a> {
    pub hub: &'a Hub,
    pub harness_name: &'a str,
    pub agent_name: &'a str,
    /// A line for the block when the hooks and the harness's MCP connection
    /// run under different agent names.
    pub name_note: Option<String>,
    pub harness_session_id: &'a str,
    pub cwd: &'a str,
    pub env: &'a dyn Fn(&str) -> Option<String>,
    pub now_secs: i64,
    /// When the whole start, the connection's open and close included, is cut
    /// off. What was gathered by then is used.
    pub deadline: Instant,
}

#[derive(Default)]
struct Gathered {
    actor: Option<String>,
    resolved: Option<Resolved>,
    brief: Option<Value>,
    started: Option<Value>,
    index: Option<String>,
    pending: Vec<Item>,
    failure: Option<Failure>,
}

fn split_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| line.trim_end().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

fn section_total(section: Option<&Value>) -> usize {
    let items = section
        .and_then(|section| section.get("items"))
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let more = section
        .and_then(|section| section.get("more"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    items + more
}

fn counted(count: usize, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

fn section_items<'a>(brief: &'a Value, section: &str, take: usize) -> Vec<&'a Value> {
    brief
        .get(section)
        .and_then(|section| section.get("items"))
        .and_then(Value::as_array)
        .map(|items| items.iter().take(take).collect())
        .unwrap_or_default()
}

fn text<'a>(item: &'a Value, key: &str) -> Option<&'a str> {
    item.get(key).and_then(Value::as_str)
}

/// The brief as a few lines: a count line and the newest items.
pub fn brief_lines(brief: Option<&Value>) -> Vec<String> {
    let Some(brief) = brief else {
        return Vec::new();
    };
    let answers = section_total(brief.get("answers"));
    let events = section_total(brief.get("events"));
    let stale = section_total(brief.get("stale_pages"));
    let mut parts = Vec::new();
    if answers > 0 {
        parts.push(counted(answers, "answer"));
    }
    if events > 0 {
        parts.push(counted(events, "event"));
    }
    if stale > 0 {
        parts.push(counted(stale, "stale page"));
    }
    let summary = if parts.is_empty() {
        "nothing new".to_string()
    } else {
        parts.join(", ")
    };
    let mut lines = vec![format!("Since your last look: {summary}.")];
    for item in section_items(brief, "answers", 3) {
        let kind = text(item, "kind").unwrap_or_default();
        let outcome = text(item, "outcome").unwrap_or_default();
        let what = text(item, "summary")
            .or_else(|| text(item, "id"))
            .unwrap_or_default();
        lines.push(format!("- {kind} {outcome}: {what}"));
    }
    for item in section_items(brief, "events", 6) {
        let kind = text(item, "kind").unwrap_or_default();
        let open = if item.get("open").and_then(Value::as_bool) == Some(true) {
            " open"
        } else {
            ""
        };
        let what = text(item, "summary")
            .or_else(|| text(item, "id"))
            .unwrap_or_default();
        lines.push(format!("- [{kind}]{open} {what}"));
    }
    lines
}

fn anchor_view(anchor: Option<&Value>, now_secs: i64) -> Option<Anchor> {
    let anchor = anchor.filter(|anchor| anchor.is_object())?;
    let mut lines = split_lines(text(anchor, "summary").unwrap_or_default());
    let next = match anchor.get("next_steps") {
        Some(Value::Array(steps)) => split_lines(
            &steps
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        Some(Value::String(steps)) => split_lines(steps),
        _ => Vec::new(),
    };
    if !next.is_empty() {
        lines.push("Next:".to_string());
        lines.extend(next);
    }
    Some(Anchor {
        phase: text(anchor, "phase").unwrap_or_default().to_string(),
        age: ago(text(anchor, "created_at").unwrap_or_default(), now_secs),
        lines,
    })
}

async fn gather(request: &Request<'_>, conn: &mut Conn, gathered: &mut Gathered) {
    let git = gather_git(
        request.cwd,
        (Instant::now() + GIT_BUDGET).min(request.deadline),
    );
    let who = conn.call("whoami", json!({})).await.ok();
    let actor = who
        .as_ref()
        .and_then(|who| text(who, "actor"))
        .unwrap_or(request.agent_name)
        .to_string();
    let personal = who
        .as_ref()
        .and_then(|who| text(who, "personal_project"))
        .unwrap_or(&actor)
        .to_string();
    gathered.actor = Some(actor);

    let mut env = std::collections::HashMap::new();
    for key in ["HUB_PROJECT", "HUB_SESSION"] {
        if let Some(value) = (request.env)(key) {
            env.insert(key.to_string(), value);
        }
    }
    let mut input = ResolveInput {
        harness: request.harness_name.to_string(),
        harness_session_id: request.harness_session_id.to_string(),
        env,
        files: read_top_level_file(git.top_level.as_deref().filter(|_| git.is_repo)),
        git,
        known_projects: Vec::new(),
        personal_project: personal,
    };
    let mut resolved = resolve_context(&input);
    if !resolved.project_is_pinned() && input.git.is_repo {
        input.known_projects = request.hub.projects().await.unwrap_or_default();
        resolved = resolve_context(&input);
    }
    let project_id = resolved.project_id.clone();
    let session_name = resolved.session_name.clone();
    gathered.resolved = Some(resolved);

    gathered.brief = conn
        .call("session_brief", json!({"project_id": project_id}))
        .await
        .ok();
    match conn
        .call(
            "session_start",
            json!({"project_id": project_id, "session_name": session_name}),
        )
        .await
    {
        Ok(started) => gathered.started = Some(started),
        Err(failure) => {
            gathered.failure = Some(failure);
            return;
        }
    }
    let session_id = gathered
        .started
        .as_ref()
        .and_then(|started| text(started, "session_id"))
        .map(str::to_string);

    gathered.index = conn
        .call(
            "brain_get",
            json!({"path": "/fs/index.md", "store": "project", "project_id": project_id}),
        )
        .await
        .ok()
        .and_then(|page| text(&page, "content").map(str::to_string));

    if let Some(session_id) = &session_id {
        conn.call(
            "notify_subscribe",
            json!({
                "kinds": "comment,page",
                "project_id": project_id,
                "about_self": true,
                "session": {"session_id": session_id},
            }),
        )
        .await
        .ok();
        let mut drained = conn.drain(session_id).await.unwrap_or_default();
        gathered.pending.append(&mut drained);
    }
}

/// Run the bundle and render the block.
///
/// A call that fails costs its own part of the block. Only a hub that cannot be
/// reached, or a session that cannot be started, fails the whole bundle.
pub async fn start(request: &Request<'_>) -> Result<Started, Failure> {
    let mut conn = request.hub.open(STARTUP_BUDGET).await?;
    let mut gathered = Gathered::default();
    let gather_until = request
        .deadline
        .checked_sub(CLOSE_BUDGET)
        .unwrap_or(request.deadline);
    let finished = tokio::time::timeout_at(
        tokio::time::Instant::from_std(gather_until),
        gather(request, &mut conn, &mut gathered),
    )
    .await;
    let mut pending = conn.take_trailers();
    conn.close().await;
    pending.append(&mut gathered.pending);
    let pending = dedupe(pending);

    let (Some(resolved), Some(started)) = (gathered.resolved.as_ref(), gathered.started.as_ref())
    else {
        return Err(gathered.failure.unwrap_or_else(|| {
            let reason = if finished.is_err() {
                "starting the session took too long"
            } else {
                "the hub did not start a session"
            };
            Failure::Unavailable(reason.to_string())
        }));
    };
    let actor = gathered
        .actor
        .clone()
        .unwrap_or_else(|| request.agent_name.to_string());
    let session_id = text(started, "session_id").map(str::to_string);
    let handoff = text(started, "handoff")
        .map(split_lines)
        .unwrap_or_default();
    let block = context_block(&ContextInput {
        project_id: resolved.project_id.clone(),
        session_name: resolved.session_name.clone(),
        session_id: session_id.clone(),
        actor: actor.clone(),
        context_lines: resolved
            .context_lines()
            .into_iter()
            .chain(request.name_note.clone())
            .collect(),
        brief: brief_lines(gathered.brief.as_ref()),
        anchor: anchor_view(started.get("anchor"), request.now_secs),
        handoff,
        kb_index: gathered
            .index
            .as_deref()
            .map(split_lines)
            .unwrap_or_default(),
        pending: pending.iter().map(notice_line).collect(),
    });
    Ok(Started {
        block: block.text,
        project_id: resolved.project_id.clone(),
        session_name: resolved.session_name.clone(),
        session_id,
        actor,
    })
}

/// The instruction a subagent gets in place of a session of its own.
pub fn subagent_text(session_id: Option<&str>) -> String {
    match session_id.map(sanitize) {
        Some(id) => format!(
            "You share the parent's hub connection and session {id}. Do not call session_start. \
             Write with `session: {{\"session_id\": \"{id}\"}}`."
        ),
        None => "You share the parent's hub connection and session. Do not call session_start."
            .to_string(),
    }
}

/// What a compaction has to keep.
pub fn keep_instruction(project_id: &str, session_name: &str, session_id: Option<&str>) -> String {
    let id = session_id
        .map(|id| format!(" (id {})", sanitize(id)))
        .unwrap_or_default();
    format!(
        "Keep: hub project {}, session {}{id}, the current phase and open hub items.",
        sanitize(project_id),
        sanitize(session_name)
    )
}
