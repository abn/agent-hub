//! The hook contract's golden fixtures, run against the Rust implementation.
//!
//! Each file in `plugins/contract/fixtures` holds one case, and the OpenCode
//! plugin runs the same files, so the two implementations are held to one
//! behaviour. The fixtures carry no I/O: git facts, files and the clock are data
//! in the input.

use std::path::PathBuf;

use agent_hub::client::Failure;
use agent_hub::client::hook::bundle::brief_lines;
use agent_hub::client::hook::child::{self, Starter};
use agent_hub::client::hook::input::{self, Event, Harness};
use agent_hub::client::hook::render::{
    ContextInput, Item, Output, WakeMode, context_block, notices, wire,
};
use agent_hub::client::hook::resolve::{ResolveInput, resolve_context};
use agent_hub::client::hook::session_input;
use agent_hub::client::hook::state::State;
use serde::Deserialize;
use serde_json::Value;

struct Case {
    name: String,
    input: Value,
    expected: Value,
}

fn family(prefix: &str) -> Vec<Case> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("plugins/contract/fixtures");
    let mut cases: Vec<Case> = std::fs::read_dir(&dir)
        .unwrap_or_else(|err| panic!("read {}: {err}", dir.display()))
        .filter_map(|entry| {
            let path = entry.expect("a directory entry").path();
            let file = path.file_name()?.to_str()?.to_string();
            (file.starts_with(&format!("{prefix}-")) && file.ends_with(".json")).then_some(path)
        })
        .map(|path| {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
            let mut fixture: Value = serde_json::from_str(&text)
                .unwrap_or_else(|err| panic!("parse {}: {err}", path.display()));
            let name = fixture["name"].as_str().expect("a name").to_string();
            assert_eq!(
                Some(name.as_str()),
                path.file_stem().and_then(|stem| stem.to_str()),
                "a fixture is named for its file"
            );
            Case {
                name,
                input: fixture["input"].take(),
                expected: fixture["expected"].take(),
            }
        })
        .collect();
    cases.sort_by(|a, b| a.name.cmp(&b.name));
    cases
}

#[test]
fn resolve_fixtures() {
    let cases = family("resolve");
    assert!(cases.len() > 20, "found {} resolve cases", cases.len());
    for case in cases {
        let input: ResolveInput = serde_json::from_value(case.input)
            .unwrap_or_else(|err| panic!("{}: input: {err}", case.name));
        let got = serde_json::to_value(resolve_context(&input)).expect("serialise");
        assert_eq!(got, case.expected, "{}", case.name);
    }
}

#[derive(Deserialize)]
struct NoticeInput {
    hub_wake: Option<String>,
    now: u64,
    last_wake_at: Option<u64>,
    items: Vec<Item>,
    #[serde(default)]
    hub_url: Option<String>,
}

#[test]
fn notice_fixtures() {
    let cases = family("notice");
    assert!(cases.len() > 10, "found {} notice cases", cases.len());
    for case in cases {
        let input: NoticeInput = serde_json::from_value(case.input)
            .unwrap_or_else(|err| panic!("{}: input: {err}", case.name));
        let got = notices(
            WakeMode::parse(input.hub_wake.as_deref()),
            input.now,
            input.last_wake_at,
            &input.items,
            input.hub_url.as_deref(),
        );
        assert_eq!(
            got.lines,
            serde_json::from_value::<Vec<String>>(case.expected["lines"].clone()).expect("lines"),
            "{}: lines",
            case.name
        );
        assert_eq!(
            Some(got.wake),
            case.expected["wake"].as_bool(),
            "{}: wake",
            case.name
        );
        assert_eq!(
            got.watch_lines,
            serde_json::from_value::<Vec<String>>(case.expected["watch_lines"].clone())
                .expect("watch_lines"),
            "{}: watch_lines",
            case.name
        );
    }
}

#[test]
fn brief_fixtures() {
    let cases = family("brief");
    assert!(!cases.is_empty(), "found no brief cases");
    for case in cases {
        let hub_url = case.input["hub_url"].as_str();
        let got = brief_lines(Some(&case.input["brief"]), hub_url);
        assert_eq!(
            serde_json::to_value(got).expect("lines"),
            case.expected["lines"],
            "{}",
            case.name
        );
    }
}

#[test]
fn context_fixtures() {
    let cases = family("context");
    assert!(cases.len() > 10, "found {} context cases", cases.len());
    for case in cases {
        let input: ContextInput = serde_json::from_value(case.input)
            .unwrap_or_else(|err| panic!("{}: input: {err}", case.name));
        let got = context_block(&input);
        assert_eq!(
            Some(got.text.as_str()),
            case.expected["text"].as_str(),
            "{}: text",
            case.name
        );
        assert_eq!(
            Some(got.truncated),
            case.expected["truncated"].as_bool(),
            "{}: truncated",
            case.name
        );
    }
}

fn wire_family(prefix: &str, minimum: usize) {
    let cases = family(prefix);
    assert!(
        cases.len() >= minimum,
        "found {} {prefix} cases",
        cases.len()
    );
    for case in cases {
        let harness = Harness::parse(case.input["harness"].as_str().expect("a harness"))
            .unwrap_or_else(|| panic!("{}: a known harness", case.name));
        let event = Event::parse(case.input["event"].as_str().expect("an event"))
            .unwrap_or_else(|| panic!("{}: a known event", case.name));
        let output: Output = serde_json::from_value(case.input["output"].clone())
            .unwrap_or_else(|err| panic!("{}: output: {err}", case.name));
        let printed = wire(harness, event, &output);
        assert_eq!(case.expected["exit_code"], 0, "{}: exit code", case.name);
        match case.expected.get("stdout_json") {
            Some(expected) => {
                let parsed: Value = serde_json::from_str(&printed).unwrap_or_else(|err| {
                    panic!("{}: stdout is JSON ({err}): {printed:?}", case.name)
                });
                assert_eq!(&parsed, expected, "{}", case.name);
            }
            None => assert_eq!(
                Some(printed.as_str()),
                case.expected["stdout_text"].as_str(),
                "{}",
                case.name
            ),
        }
    }
}

#[test]
fn claude_fixtures() {
    wire_family("claude", 8);
}

#[test]
fn antigravity_fixtures() {
    wire_family("antigravity", 6);
}

#[test]
fn pretool_fixtures() {
    let cases = family("pretool");
    assert!(cases.len() >= 10, "found {} pretool cases", cases.len());
    for case in cases {
        let state: State = serde_json::from_value(case.input["state"].clone())
            .unwrap_or_else(|err| panic!("{}: state: {err}", case.name));
        let stdin = case.input["stdin"].to_string();
        let parsed = input::parse(Harness::Claude, &stdin);
        let output = Output {
            updated_input: session_input(&state, &parsed),
            ..Output::default()
        };
        let printed = wire(Harness::Claude, Event::PreToolUse, &output);
        let got: Value = serde_json::from_str(&printed)
            .unwrap_or_else(|err| panic!("{}: stdout is JSON ({err}): {printed:?}", case.name));
        assert_eq!(case.expected["exit_code"], 0, "{}: exit code", case.name);
        assert_eq!(got, case.expected["stdout_json"], "{}", case.name);
        assert!(
            got.pointer("/hookSpecificOutput/permissionDecision")
                .is_none(),
            "{}: the hook never decides the permission",
            case.name
        );
    }
}

/// Answers `session_start` from the replies a fixture recorded, and notes each
/// call.
struct Recorded {
    replies: std::vec::IntoIter<Value>,
    calls: Vec<Value>,
}

impl Starter for Recorded {
    async fn start(&mut self, session_name: &str, exclusive: bool) -> Result<Value, Failure> {
        self.calls
            .push(serde_json::json!({"session_name": session_name, "exclusive": exclusive}));
        let reply = self
            .replies
            .next()
            .expect("a reply is recorded for the call");
        match (reply.get("ok"), reply.get("error")) {
            (Some(ok), _) => Ok(ok.clone()),
            (_, Some(error)) => Err(Failure::Tool(error.clone())),
            _ => panic!("a reply is ok or error: {reply}"),
        }
    }
}

#[test]
fn subagent_fixtures() {
    let cases = family("subagent");
    assert!(cases.len() >= 10, "found {} subagent cases", cases.len());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a runtime");
    for case in cases {
        let text = |key: &str| case.input[key].as_str().map(str::to_string);
        let replies = case.input["replies"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut starter = Recorded {
            replies: replies.into_iter(),
            calls: Vec::new(),
        };
        let parent = text("parent").expect("a parent");
        let agent_id = text("agent_id").expect("an agent id");
        let started = runtime.block_on(async {
            match text("mapped") {
                Some(name) => {
                    let known = agent_hub::client::hook::state::Child {
                        name,
                        session_id: None,
                    };
                    child::resume(&mut starter, &known).await
                }
                None => {
                    child::start_new(
                        &mut starter,
                        &parent,
                        text("agent_type").as_deref(),
                        &agent_id,
                    )
                    .await
                }
            }
        });
        assert_eq!(
            Value::Array(starter.calls),
            case.expected["calls"],
            "{}: calls",
            case.name
        );
        let got = started.ok().map(|child| Value::String(child.name));
        assert_eq!(
            got.unwrap_or(Value::Null),
            case.expected["child"],
            "{}: child",
            case.name
        );
    }
}
