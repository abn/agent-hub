//! The text rules of the hook contract: notice lines, the wake decision, the
//! `<agent-hub>` block and the harness wire formats. Everything here is pure,
//! so the golden fixtures drive it with no I/O. Lengths count Unicode scalar
//! values.

use serde::Deserialize;
use serde_json::{Value, json};

use super::input::{Event, Harness};

/// Seconds that must pass between two wakes of one session.
pub const WAKE_COOLDOWN_SECONDS: u64 = 300;
/// The most characters the `<agent-hub>` block may hold.
pub const BLOCK_CAP: usize = 2500;

const TITLE_MAX: usize = 120;
const LINE_MAX: usize = 240;
const BRIEF_MAX: usize = 12;
const ANCHOR_MAX: usize = 10;
const KB_MAX: usize = 15;

/// One item the hub reports as waiting for the caller.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Item {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub direct: bool,
}

impl Item {
    /// An item as the hub's trailer or peek spells it.
    pub fn from_wire(value: &Value) -> Option<Self> {
        let text = |key: &str| value.get(key).and_then(Value::as_str);
        Some(Self {
            id: text("id")?.to_string(),
            kind: text("kind")?.to_string(),
            title: text("title")
                .or_else(|| text("summary"))
                .unwrap_or_default()
                .to_string(),
            project: text("project")
                .or_else(|| text("project_id"))
                .unwrap_or_default()
                .to_string(),
            direct: value.get("direct").and_then(Value::as_bool) == Some(true),
        })
    }

    /// The items of a wire list, skipping any that name no id or kind.
    pub fn list_from_wire(value: Option<&Value>) -> Vec<Self> {
        value
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Self::from_wire).collect())
            .unwrap_or_default()
    }
}

/// Cut `text` to `max` characters, ending in `...` when it was cut.
pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max.saturating_sub(3)).collect();
    cut.push_str("...");
    cut
}

/// Collapse every run of ASCII whitespace to one space and trim one space from
/// each end.
pub fn flatten(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_run = false;
    for ch in text.chars() {
        if matches!(ch, ' ' | '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r') {
            if !in_run {
                out.push(' ');
            }
            in_run = true;
        } else {
            out.push(ch);
            in_run = false;
        }
    }
    let out = out.strip_prefix(' ').unwrap_or(&out);
    out.strip_suffix(' ').unwrap_or(out).to_string()
}

/// Make a hub-sourced field safe to print inside the `<agent-hub>` block: one
/// line, and unable to open or close a block of its own. Every `<agent-hub` and
/// `</agent-hub`, in any letter case, becomes `<agent_hub` and `</agent_hub`.
pub fn sanitize(text: &str) -> String {
    const TAG: &str = "agent-hub";
    let flat = flatten(text);
    let lower = flat.to_ascii_lowercase();
    let mut out = String::with_capacity(flat.len());
    let mut copied = 0;
    let mut from = 0;
    while let Some(found) = lower[from..].find('<') {
        let open = from + found;
        let name = if lower[open + 1..].starts_with('/') {
            open + 2
        } else {
            open + 1
        };
        if lower[name..].starts_with(TAG) {
            out.push_str(&flat[copied..name]);
            out.push_str("agent_hub");
            copied = name + TAG.len();
            from = copied;
        } else {
            from = open + 1;
        }
    }
    out.push_str(&flat[copied..]);
    out
}

/// The URL that opens a hub resource by id: `<hub>/id/<id>`.
pub fn resource_url(hub_url: &str, id: &str) -> String {
    format!(
        "{}/id/{}",
        sanitize(hub_url).trim_end_matches('/'),
        sanitize(id)
    )
}

/// One waiting item as a line.
pub fn notice_line(item: &Item) -> String {
    notice_line_at(item, None)
}

/// One waiting item as a line. With a hub URL the id is spelled as the
/// resource's URL.
pub fn notice_line_at(item: &Item, hub_url: Option<&str>) -> String {
    let reference = hub_url.map_or_else(|| sanitize(&item.id), |hub| resource_url(hub, &item.id));
    format!(
        "- [{}] {} ({}, {})",
        sanitize(&item.kind),
        clip(&sanitize(&item.title), TITLE_MAX),
        sanitize(&item.project),
        reference
    )
}

/// Which items may wake an idle agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeMode {
    Direct,
    All,
    Never,
}

impl WakeMode {
    /// The mode `HUB_WAKE` names. An unset or unrecognised value is `direct`.
    pub fn parse(value: Option<&str>) -> Self {
        match value {
            Some("all") => Self::All,
            Some("never") => Self::Never,
            _ => Self::Direct,
        }
    }
}

/// What a batch of waiting items does to an idle agent.
#[derive(Debug, PartialEq, Eq)]
pub struct Notices {
    /// A line per item, in the order given.
    pub lines: Vec<String>,
    /// Whether the batch wakes an idle agent.
    pub wake: bool,
    /// What `watch` prints before it exits: every item, direct ones first.
    pub watch_lines: Vec<String>,
}

/// Decide whether `items` wake an idle agent, and what a wake prints.
pub fn notices(
    mode: WakeMode,
    now: u64,
    last_wake_at: Option<u64>,
    items: &[Item],
    hub_url: Option<&str>,
) -> Notices {
    let eligible = match mode {
        WakeMode::All => !items.is_empty(),
        WakeMode::Direct => items.iter().any(|item| item.direct),
        WakeMode::Never => false,
    };
    let cooled = last_wake_at.is_none_or(|last| now.saturating_sub(last) >= WAKE_COOLDOWN_SECONDS);
    let wake = eligible && cooled;
    Notices {
        lines: items
            .iter()
            .map(|item| notice_line_at(item, hub_url))
            .collect(),
        wake,
        watch_lines: if wake {
            direct_first(items, hub_url)
        } else {
            Vec::new()
        },
    }
}

/// Every item as a line, direct items first and each group in the order given.
pub fn direct_first(items: &[Item], hub_url: Option<&str>) -> Vec<String> {
    items
        .iter()
        .filter(|item| item.direct)
        .chain(items.iter().filter(|item| !item.direct))
        .map(|item| notice_line_at(item, hub_url))
        .collect()
}

/// The last anchor a session left.
#[derive(Debug, Clone, Deserialize)]
pub struct Anchor {
    pub phase: String,
    pub age: String,
    #[serde(default)]
    pub lines: Vec<String>,
}

/// The pieces of the `<agent-hub>` block, as lines before any cap.
#[derive(Debug, Clone, Deserialize)]
pub struct ContextInput {
    pub project_id: String,
    pub session_name: String,
    #[serde(default)]
    pub session_id: Option<String>,
    /// The member the session acts as.
    pub member: String,
    /// The agent the hub knows this client by.
    pub agent: String,
    /// Whether the plugin adds `session` to the model's hub calls. Claude and
    /// OpenCode do; Antigravity has no such hook, so the model passes it.
    #[serde(default = "plugin_adds_session")]
    pub plugin_adds_session: bool,
    #[serde(default)]
    pub context_lines: Vec<String>,
    #[serde(default)]
    pub brief: Vec<String>,
    #[serde(default)]
    pub anchor: Option<Anchor>,
    #[serde(default)]
    pub handoff: Vec<String>,
    #[serde(default)]
    pub kb_index: Vec<String>,
    #[serde(default)]
    pub pending: Vec<String>,
    /// The hub's base URL, for the line that asks for resources by URL.
    #[serde(default)]
    pub hub_url: Option<String>,
}

fn plugin_adds_session() -> bool {
    true
}

/// The rendered block.
#[derive(Debug, PartialEq, Eq)]
pub struct Block {
    pub text: String,
    pub truncated: bool,
}

fn cut(lines: &[String], max: usize) -> Vec<String> {
    lines
        .iter()
        .take(max)
        .map(|line| clip(&sanitize(line), LINE_MAX))
        .collect()
}

fn header_line(input: &ContextInput) -> String {
    let id = input
        .session_id
        .as_deref()
        .map(|id| format!(" ({})", sanitize(id)))
        .unwrap_or_default();
    format!(
        "project {} | session {}{id} | you are {} \u{b7} {}",
        sanitize(&input.project_id),
        sanitize(&input.session_name),
        sanitize(&input.member),
        sanitize(&input.agent)
    )
}

fn bind_line(input: &ContextInput) -> String {
    let how = if input.plugin_adds_session {
        "on hub calls (the plugin adds it)"
    } else {
        "on every hub call"
    };
    format!(
        "Pass session: {{\"project_id\": \"{}\", \"name\": \"{}\"}} {how}.",
        sanitize(&input.project_id),
        sanitize(&input.session_name)
    )
}

fn url_line(input: &ContextInput) -> Option<String> {
    let hub = input.hub_url.as_deref().filter(|hub| !hub.is_empty())?;
    Some(format!(
        "Refer to hub resources by URL, {}/id/<id>, never a bare id.",
        sanitize(hub).trim_end_matches('/')
    ))
}

/// Render the full block, removing whole lines from the lowest-priority group
/// until it fits [`BLOCK_CAP`].
pub fn context_block(input: &ContextInput) -> Block {
    let mut head = vec![header_line(input)];
    head.extend(input.context_lines.iter().map(|line| sanitize(line)));
    head.push(bind_line(input));
    head.push("Do not call session_brief; this block is the brief.".to_string());
    head.extend(url_line(input));

    let mut brief = cut(&input.brief, BRIEF_MAX);
    let mut anchor = input
        .anchor
        .as_ref()
        .map(|anchor| cut(&anchor.lines, ANCHOR_MAX))
        .unwrap_or_default();
    let mut handoff = cut(&input.handoff, usize::MAX);
    let mut kb = cut(&input.kb_index, KB_MAX);
    let mut pending = cut(&input.pending, usize::MAX);

    let render = |brief: &[String],
                  anchor: &[String],
                  handoff: &[String],
                  kb: &[String],
                  pending: &[String],
                  truncated: bool| {
        let mut body = head.clone();
        body.extend(brief.iter().cloned());
        if let Some(last) = &input.anchor {
            body.push(format!(
                "Last anchor (phase \"{}\", {}):",
                sanitize(&last.phase),
                sanitize(&last.age)
            ));
            body.extend(anchor.iter().cloned());
        }
        if !handoff.is_empty() {
            body.push("Handoff:".to_string());
            body.extend(handoff.iter().cloned());
        }
        if !kb.is_empty() {
            body.push("KB index:".to_string());
            body.extend(kb.iter().cloned());
        }
        if !pending.is_empty() {
            body.push("Pending:".to_string());
            body.extend(pending.iter().cloned());
        }
        if truncated {
            body.push("[truncated]".to_string());
        }
        format!("<agent-hub>\n{}\n</agent-hub>", body.join("\n"))
    };

    let text = render(&brief, &anchor, &handoff, &kb, &pending, false);
    if text.chars().count() <= BLOCK_CAP {
        return Block {
            text,
            truncated: false,
        };
    }
    loop {
        let dropped = [&mut kb, &mut handoff, &mut anchor, &mut brief, &mut pending]
            .into_iter()
            .find(|group| !group.is_empty())
            .map(|group| group.pop())
            .is_some();
        let text = render(&brief, &anchor, &handoff, &kb, &pending, true);
        if !dropped || text.chars().count() <= BLOCK_CAP {
            return Block {
                text,
                truncated: true,
            };
        }
    }
}

/// The short block a later Antigravity prompt carries: the header and bind
/// lines, then whatever is waiting.
pub fn pointer_block(input: &ContextInput) -> String {
    let mut body = vec![header_line(input), bind_line(input)];
    body.extend(url_line(input));
    let pending = cut(&input.pending, usize::MAX);
    if !pending.is_empty() {
        body.push("Pending:".to_string());
        body.extend(pending);
    }
    format!("<agent-hub>\n{}\n</agent-hub>", body.join("\n"))
}

/// A block that holds one line about the connection, such as a hub that is
/// unreachable or a member waiting for approval.
pub fn notice_block(line: &str) -> String {
    format!("<agent-hub>\n{}\n</agent-hub>", sanitize(line))
}

/// A block that only lists what is waiting, for a prompt on a session the
/// model already knows.
pub fn pending_block(lines: &[String]) -> String {
    format!("<agent-hub>\nPending:\n{}\n</agent-hub>", lines.join("\n"))
}

/// What a hook hands back to its harness before the wire format is applied.
#[derive(Debug, Default, Clone, Deserialize)]
pub struct Output {
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub compaction_instructions: Option<String>,
    /// The tool input a `PreToolUse` hook replaces the model's with.
    #[serde(default)]
    pub updated_input: Option<Value>,
}

/// The harness hook output for one event, without a trailing newline.
///
/// An absent or empty context is the no-op, never an object carrying an empty
/// string.
pub fn wire(harness: Harness, event: Event, output: &Output) -> String {
    let context = output.context.as_deref().filter(|text| !text.is_empty());
    let noop = "{}".to_string();
    match (harness, event) {
        (Harness::Claude, Event::SessionStart | Event::Prompt | Event::SubagentStart) => {
            let Some(context) = context else {
                return noop;
            };
            let name = match event {
                Event::SessionStart => "SessionStart",
                Event::Prompt => "UserPromptSubmit",
                _ => "SubagentStart",
            };
            json!({"hookSpecificOutput": {"hookEventName": name, "additionalContext": context}})
                .to_string()
        }
        (Harness::Claude, Event::PreCompact) => {
            output.compaction_instructions.clone().unwrap_or_default()
        }
        // `updatedInput` alone: a `permissionDecision` would change whether the
        // call is allowed, and this hook only changes what it carries.
        (Harness::Claude, Event::PreToolUse) => match &output.updated_input {
            Some(input) => json!({"hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "updatedInput": input,
            }})
            .to_string(),
            None => noop,
        },
        (Harness::Antigravity, Event::Prompt) => match context {
            Some(context) => json!({"injectSteps": [{"ephemeralMessage": context}]}).to_string(),
            None => noop,
        },
        _ => noop,
    }
}

/// How long ago an RFC 3339 time was, as the block prints it.
pub fn ago(from: &str, now_secs: i64) -> String {
    let Ok(then) =
        time::OffsetDateTime::parse(from, &time::format_description::well_known::Rfc3339)
    else {
        return "earlier".to_string();
    };
    let secs = (now_secs - then.unix_timestamp()).max(0);
    if secs < 90 {
        return "just now".to_string();
    }
    let mins = (secs as f64 / 60.0).round() as i64;
    if mins < 90 {
        return format!("{mins}m ago");
    }
    let hours = (secs as f64 / 3600.0).round() as i64;
    if hours < 36 {
        return format!("{hours}h ago");
    }
    format!("{}d ago", (secs as f64 / 86400.0).round() as i64)
}
