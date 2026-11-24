//! Which hub, member and token a managed entry point uses.
//!
//! [`resolve`] is pure over its input and is the one place a token is chosen:
//! hooks, `agent-hub mcp`, `agent-hub token --headers` and the proxy all act
//! on its `state`. The OpenCode plugin carries a JavaScript twin; both run the
//! shared `credential-*` contract fixtures, whose JSON shape these types
//! deserialize and serialize as is.
//!
//! The store side edits `config.toml` as text under the lock-dir protocol
//! ([`crate::client::lockdir`]): comments, other tables and a legacy
//! `[client] token` survive every write.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::client::lockdir::{self, LockPolicy};
use crate::error::{Error, Result};

/// The environment the resolver reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Env {
    #[serde(rename = "HUB_URL", default, skip_serializing_if = "Option::is_none")]
    pub hub_url: Option<String>,
    #[serde(rename = "HUB_TOKEN", default, skip_serializing_if = "Option::is_none")]
    pub hub_token: Option<String>,
    #[serde(
        rename = "HUB_MEMBER",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub hub_member: Option<String>,
    #[serde(
        rename = "HUB_CONFIG",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub hub_config: Option<String>,
}

/// `.agent-hub.toml` at the git top level. It never holds a token.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProjectFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
}

/// `[client]` defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ClientDefaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
}

/// One `[[credential]]`. A legacy `[client] token` folds in as one entry with
/// no member.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Credential {
    pub url: String,
    #[serde(default)]
    pub member: Option<String>,
    pub token: String,
    /// `default = true`: the credential Claude Code's header helper sends for
    /// this hub when it holds several for it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub default: bool,
}

/// One `[trust] pairings` entry. A pairing with no member matches only a
/// selection with no member.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Pairing {
    pub path: String,
    pub url: String,
    #[serde(default)]
    pub member: Option<String>,
}

/// `[trust]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Trust {
    #[serde(default)]
    pub pairings: Vec<Pairing>,
}

/// The parsed parts of `config.toml` the resolver reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ConfigView {
    #[serde(default)]
    pub client: ClientDefaults,
    #[serde(default)]
    pub credentials: Vec<Credential>,
    #[serde(default)]
    pub trust: Trust,
}

/// Everything [`resolve`] decides from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ResolveInput {
    #[serde(default)]
    pub env: Env,
    /// The URL the plugin or `--url` pins.
    #[serde(default)]
    pub pinned_url: Option<String>,
    #[serde(default)]
    pub project_file: Option<ProjectFile>,
    /// The git top level the project file was found in.
    #[serde(default)]
    pub project_root: Option<String>,
    #[serde(default)]
    pub config: ConfigView,
}

/// Where the URL came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UrlSource {
    Pinned,
    Env,
    Project,
    Client,
    /// The only distinct credential URL.
    Only,
    None,
}

/// Where the member came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MemberSource {
    Env,
    Project,
    Client,
    /// The only named credential member for the URL.
    Only,
    None,
}

/// Where the token came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TokenSource {
    Env,
    Credential,
    /// The unnamed legacy credential, used only when no member was selected.
    Legacy,
    None,
}

/// The `source` record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct Sources {
    pub url: UrlSource,
    pub member: MemberSource,
    pub token: TokenSource,
}

/// What the entry point does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// A URL and a token.
    Ok,
    /// A URL and no token: connect without Authorization (the enrol surface).
    Tokenless,
    NoHub,
    /// Several members hold credentials for the URL and nothing picks one.
    SeveralMembers,
    /// The project file asks for a pairing `[trust]` does not hold.
    NeedsTrust,
}

/// The resolver's answer.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Resolved {
    pub url: Option<String>,
    pub member: Option<String>,
    pub token: Option<String>,
    pub source: Sources,
    pub state: State,
    /// Context lines for the human, such as a project file naming another URL
    /// than the pinned one. Empty when none.
    #[serde(default)]
    pub notices: Vec<String>,
}

/// Trimmed text, or none when it is blank. Members, URLs and tokens compare by
/// this one rule everywhere.
fn present(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|text| !text.is_empty())
}

/// A URL in the form two URLs compare in: scheme and host lowercased, the
/// default port and a trailing slash dropped.
pub fn normalise_url(url: &str) -> String {
    let text = url.trim();
    if text.is_empty() {
        return String::new();
    }
    match url::Url::parse(text) {
        Ok(parsed) if parsed.has_host() => {
            let host = parsed.host_str().unwrap_or_default();
            let port = parsed
                .port()
                .map(|port| format!(":{port}"))
                .unwrap_or_default();
            let path = parsed.path().trim_end_matches('/');
            let query = parsed.query().map(|q| format!("?{q}")).unwrap_or_default();
            format!("{}://{host}{port}{path}{query}", parsed.scheme())
        }
        _ => text.trim_end_matches('/').to_string(),
    }
}

fn same(a: &str, b: &str) -> bool {
    normalise_url(a) == normalise_url(b)
}

/// The distinct URLs of the credentials, as first written.
fn distinct_urls(credentials: &[Credential]) -> Vec<&str> {
    let mut seen: Vec<String> = Vec::new();
    let mut urls = Vec::new();
    for credential in credentials {
        let key = normalise_url(&credential.url);
        if !key.is_empty() && !seen.contains(&key) {
            seen.push(key);
            urls.push(credential.url.as_str());
        }
    }
    urls
}

/// Pick the URL, member and token.
pub fn resolve(input: &ResolveInput) -> Resolved {
    let env = &input.env;
    let project = input.project_file.as_ref();
    let client = &input.config.client;
    let credentials = &input.config.credentials;
    let pairings = &input.config.trust.pairings;
    let mut notices = Vec::new();

    let pinned = present(input.pinned_url.as_deref());
    let env_url = present(env.hub_url.as_deref());
    let project_url = present(project.and_then(|p| p.url.as_deref()));
    let client_url = present(client.url.as_deref());
    let urls = distinct_urls(credentials);

    let (url, url_source) = if let Some(url) = pinned {
        (Some(url), UrlSource::Pinned)
    } else if let Some(url) = env_url {
        (Some(url), UrlSource::Env)
    } else if let Some(url) = project_url {
        (Some(url), UrlSource::Project)
    } else if let Some(url) = client_url {
        (Some(url), UrlSource::Client)
    } else if urls.len() == 1 {
        (Some(urls[0]), UrlSource::Only)
    } else {
        (None, UrlSource::None)
    };

    if let (Some(pinned), Some(project_url)) = (pinned, project_url)
        && !same(project_url, pinned)
    {
        notices.push(format!(
            ".agent-hub.toml names {project_url}; the plugin is set to {pinned}"
        ));
    }

    let out = |url: Option<&str>, notices: Vec<String>, fields: Fields| Resolved {
        url: url.map(normalise_url),
        member: fields.member,
        token: fields.token,
        source: fields.source,
        state: fields.state,
        notices,
    };
    let bare = |source: Sources, state: State| Fields {
        member: None,
        token: None,
        source,
        state,
    };

    let Some(url) = url else {
        return out(
            None,
            notices,
            bare(
                Sources {
                    url: url_source,
                    member: MemberSource::None,
                    token: TokenSource::None,
                },
                State::NoHub,
            ),
        );
    };

    let for_url: Vec<&Credential> = credentials.iter().filter(|c| same(&c.url, url)).collect();
    let mut named_members: Vec<&str> = Vec::new();
    for credential in &for_url {
        if let Some(member) = present(credential.member.as_deref())
            && !named_members.contains(&member)
        {
            named_members.push(member);
        }
    }

    let env_member = present(env.hub_member.as_deref());
    let project_member = present(project.and_then(|p| p.member.as_deref()));
    let client_member = present(client.member.as_deref());

    let mut member: Option<&str> = None;
    let mut member_source = MemberSource::None;
    let mut several = false;
    if let Some(value) = env_member {
        (member, member_source) = (Some(value), MemberSource::Env);
    } else if let Some(value) = project_member
        && project_url.is_none_or(|u| same(u, url))
    {
        (member, member_source) = (Some(value), MemberSource::Project);
    } else if let Some(value) = client_member
        && client_url.is_none_or(|u| same(u, url))
    {
        (member, member_source) = (Some(value), MemberSource::Client);
    } else if named_members.len() == 1 {
        (member, member_source) = (Some(named_members[0]), MemberSource::Only);
    } else if named_members.len() > 1 {
        // Several members on this hub: the one marked `default = true`,
        // when exactly one is.
        let mut defaults: Vec<&str> = Vec::new();
        for credential in &for_url {
            if credential.default
                && let Some(member) = present(credential.member.as_deref())
                && !defaults.contains(&member)
            {
                defaults.push(member);
            }
        }
        if let [only] = defaults[..] {
            (member, member_source) = (Some(only), MemberSource::Only);
        } else {
            several = true;
        }
    }

    let mut token: Option<&str> = None;
    let mut token_source = TokenSource::None;
    let env_token = present(env.hub_token.as_deref());
    if let Some(value) = env_token
        && env_url.is_some_and(|u| same(u, url))
        && env_member.is_none_or(|m| Some(m) == member)
    {
        (token, token_source) = (Some(value), TokenSource::Env);
    } else if let Some(member) = member {
        if let Some(hit) = for_url.iter().find(|c| {
            present(c.member.as_deref()) == Some(member) && present(Some(&c.token)).is_some()
        }) {
            (token, token_source) = (Some(hit.token.as_str()), TokenSource::Credential);
        }
    } else if !several
        && let Some(hit) = for_url
            .iter()
            .find(|c| present(c.member.as_deref()).is_none() && present(Some(&c.token)).is_some())
    {
        (token, token_source) = (Some(hit.token.as_str()), TokenSource::Legacy);
    }

    let source = Sources {
        url: url_source,
        member: member_source,
        token: token_source,
    };
    let member_out = member.map(str::to_string);

    let supplied = url_source == UrlSource::Project || member_source == MemberSource::Project;
    if supplied {
        let trusted = pairings.iter().any(|p| {
            Some(p.path.as_str()) == input.project_root.as_deref()
                && same(&p.url, url)
                && present(p.member.as_deref()) == member
        });
        if !trusted {
            return out(
                Some(url),
                notices,
                Fields {
                    member: member_out,
                    token: None,
                    source: Sources {
                        token: TokenSource::None,
                        ..source
                    },
                    state: State::NeedsTrust,
                },
            );
        }
    }

    if several && member.is_none() {
        return out(Some(url), notices, bare(source, State::SeveralMembers));
    }
    let state = if token.is_some() {
        State::Ok
    } else {
        State::Tokenless
    };
    out(
        Some(url),
        notices,
        Fields {
            member: member_out,
            token: token.map(str::to_string),
            source,
            state,
        },
    )
}

struct Fields {
    member: Option<String>,
    token: Option<String>,
    source: Sources,
    state: State,
}

// config.toml: the client table, the [[credential]] entries and [trust]. A
// small scanner over the text, not a TOML library: it knows strings (basic,
// literal, multiline), comments, arrays and inline tables, and it records the
// span of every value so the store can replace one in place, which leaves
// comments, other tables and the layout alone.

/// One scanned value. A bare scalar (number, boolean, date) is kept as text
/// and is never a string.
#[derive(Debug, Clone, PartialEq)]
enum Val {
    Str(String),
    Raw(String),
    Array(Vec<Val>),
    Table(Vec<(String, Val)>),
}

impl Val {
    fn string(&self) -> Option<&str> {
        match self {
            Self::Str(text) => Some(text),
            _ => None,
        }
    }

    fn field(&self, key: &str) -> Option<&Val> {
        match self {
            Self::Table(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

/// A table header or a key and its value, with the offsets edits use.
#[derive(Debug, Clone)]
enum Item {
    Table {
        array: bool,
        name: String,
        line_end: usize,
    },
    Kv {
        key: String,
        value: Val,
        value_start: usize,
        value_end: usize,
        line_end: usize,
    },
}

fn slice(text: &str, from: usize, to: usize) -> &str {
    text.get(from..to.min(text.len())).unwrap_or("")
}

fn unescape(body: &str, multiline: bool) -> String {
    let chars: Vec<char> = body.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c != '\\' || i + 1 >= chars.len() {
            out.push(c);
            i += 1;
            continue;
        }
        i += 1;
        let next = chars[i];
        if next == 'u' || next == 'U' {
            let len = if next == 'u' { 4 } else { 8 };
            let hex: String = chars.iter().skip(i + 1).take(len).collect();
            if hex.chars().count() == len
                && hex.chars().all(|h| h.is_ascii_hexdigit())
                && let Some(decoded) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32)
            {
                out.push(decoded);
                i += 1 + len;
                continue;
            }
            out.push(next);
            i += 1;
            continue;
        }
        if multiline && matches!(next, ' ' | '\t' | '\r' | '\n') {
            let mut j = i;
            while j < chars.len() && matches!(chars[j], ' ' | '\t') {
                j += 1;
            }
            if j < chars.len() && matches!(chars[j], '\r' | '\n') {
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                i = j;
                continue;
            }
        }
        out.push(match next {
            'b' => '\u{8}',
            't' => '\t',
            'n' => '\n',
            'f' => '\u{c}',
            'r' => '\r',
            other => other,
        });
        i += 1;
    }
    out
}

fn line_end_at(bytes: &[u8], from: usize) -> usize {
    match bytes
        .get(from..)
        .and_then(|rest| rest.iter().position(|b| *b == b'\n'))
    {
        None => bytes.len(),
        Some(offset) => {
            let nl = from + offset;
            if nl > from && bytes[nl - 1] == b'\r' {
                nl - 1
            } else {
                nl
            }
        }
    }
}

/// Skip blanks, and with `lines` also line breaks and comments.
fn skip_filler(bytes: &[u8], from: usize, lines: bool) -> usize {
    let mut i = from;
    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' => i += 1,
            b'\n' | b'\r' if lines => i += 1,
            b'#' if lines => i = line_end_at(bytes, i),
            _ => break,
        }
    }
    i
}

fn strip_first_newline(text: &str) -> &str {
    text.strip_prefix("\r\n")
        .or_else(|| text.strip_prefix('\n'))
        .unwrap_or(text)
}

/// Read one value at `from`: strings come back unescaped, arrays as arrays,
/// inline tables as tables and anything else as its trimmed text. The end is
/// just past the value, never past a trailing comment.
fn parse_value(text: &str, from: usize, nested: bool) -> (Val, usize) {
    let bytes = text.as_bytes();
    let n = bytes.len();
    let at = |s: &str| {
        bytes
            .get(from..)
            .is_some_and(|rest| rest.starts_with(s.as_bytes()))
    };
    if at("\"\"\"") {
        let mut pos = from + 3;
        while pos < n && !bytes[pos..].starts_with(b"\"\"\"") {
            pos += if bytes[pos] == b'\\' { 2 } else { 1 };
        }
        if pos >= n {
            let body = strip_first_newline(slice(text, from + 3, n));
            return (Val::Str(unescape(body, true)), n);
        }
        let mut extra = 0;
        while extra < 2 && bytes.get(pos + 3 + extra) == Some(&b'"') {
            extra += 1;
        }
        let body = strip_first_newline(slice(text, from + 3, pos + extra));
        return (Val::Str(unescape(body, true)), pos + 3 + extra);
    }
    if at("'''") {
        let Some(offset) = text.get(from + 3..).and_then(|rest| rest.find("'''")) else {
            let body = strip_first_newline(slice(text, from + 3, n));
            return (Val::Str(body.to_string()), n);
        };
        let close = from + 3 + offset;
        let mut extra = 0;
        while extra < 2 && bytes.get(close + 3 + extra) == Some(&b'\'') {
            extra += 1;
        }
        let body = strip_first_newline(slice(text, from + 3, close + extra));
        return (Val::Str(body.to_string()), close + 3 + extra);
    }
    if at("\"") {
        let mut pos = from + 1;
        while pos < n && bytes[pos] != b'"' && bytes[pos] != b'\n' {
            pos += if bytes[pos] == b'\\' { 2 } else { 1 };
        }
        let stop = pos.min(n);
        let closed = bytes.get(stop) == Some(&b'"');
        let body = slice(text, from + 1, stop);
        let body = body.strip_suffix('\r').unwrap_or(body);
        let end = if closed {
            stop + 1
        } else {
            line_end_at(bytes, from)
        };
        return (Val::Str(unescape(body, false)), end);
    }
    if at("'") {
        let eol = line_end_at(bytes, from);
        let close = text
            .get(from + 1..)
            .and_then(|rest| rest.find('\''))
            .map(|o| from + 1 + o);
        return match close {
            Some(close) if close <= eol => (
                Val::Str(slice(text, from + 1, close).to_string()),
                close + 1,
            ),
            _ => (Val::Str(slice(text, from + 1, eol).to_string()), eol),
        };
    }
    if at("[") {
        let mut items = Vec::new();
        let mut i = from + 1;
        while i < n {
            i = skip_filler(bytes, i, true);
            if i >= n {
                break;
            }
            if bytes[i] == b']' {
                i += 1;
                break;
            }
            if bytes[i] == b',' {
                i += 1;
                continue;
            }
            let (value, end) = parse_value(text, i, true);
            items.push(value);
            i = end.max(i + 1);
        }
        return (Val::Array(items), i);
    }
    if at("{") {
        let mut table: Vec<(String, Val)> = Vec::new();
        let mut i = from + 1;
        while i < n {
            i = skip_filler(bytes, i, true);
            if i >= n {
                break;
            }
            if bytes[i] == b'}' {
                i += 1;
                break;
            }
            if bytes[i] == b',' {
                i += 1;
                continue;
            }
            let key;
            if bytes[i] == b'"' || bytes[i] == b'\'' {
                let (value, end) = parse_value(text, i, true);
                key = value.string().unwrap_or_default().to_string();
                i = end;
            } else {
                let start = i;
                while i < n && !b",=}\n".contains(&bytes[i]) {
                    i += 1;
                }
                key = slice(text, start, i).trim().to_string();
            }
            i = skip_filler(bytes, i, false);
            if bytes.get(i) != Some(&b'=') {
                i += 1;
                continue;
            }
            i = skip_filler(bytes, i + 1, true);
            let (value, end) = parse_value(text, i, true);
            table.retain(|(k, _)| *k != key);
            table.push((key, value));
            i = end.max(i + 1);
        }
        return (Val::Table(table), i);
    }
    let stops: &[u8] = if nested { b",]}#\n\r \t" } else { b"#\n\r" };
    let mut end = from;
    while end < n && !stops.contains(&bytes[end]) {
        end += 1;
    }
    let raw = slice(text, from, end);
    let trimmed = raw.trim_end();
    (Val::Raw(trimmed.trim().to_string()), from + trimmed.len())
}

/// Every table header and key/value of the text, in order, with the offsets
/// edits go by. A commented-out line is never an item.
fn scan_config(text: &str) -> Vec<Item> {
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut items = Vec::new();
    let mut i = 0;
    let next_line = |from: usize| match bytes
        .get(from..)
        .and_then(|r| r.iter().position(|b| *b == b'\n'))
    {
        Some(offset) => from + offset + 1,
        None => n,
    };
    while i < n {
        let mut j = i;
        while j < n && (bytes[j] == b' ' || bytes[j] == b'\t') {
            j += 1;
        }
        if j >= n {
            break;
        }
        let c = bytes[j];
        if c == b'\n' || c == b'\r' || c == b'#' {
            i = next_line(j);
            continue;
        }
        let eol = line_end_at(bytes, j);
        if c == b'[' {
            let line = slice(text, j, eol);
            let header = if line.starts_with("[[")
                && line
                    .find(']')
                    .is_some_and(|close| line[close..].starts_with("]]"))
            {
                let close = line.find(']').unwrap_or(line.len());
                Some((true, line[2..close].trim().to_string()))
            } else {
                line.find(']')
                    .map(|close| (false, line[1..close].trim().to_string()))
            };
            if let Some((array, name)) = header {
                items.push(Item::Table {
                    array,
                    name,
                    line_end: eol,
                });
            }
            i = next_line(j);
            continue;
        }
        let key;
        let mut at = j;
        if c == b'"' || c == b'\'' {
            let (value, end) = parse_value(text, j, true);
            key = value.string().unwrap_or_default().to_string();
            at = end;
        } else {
            while at < eol && bytes[at] != b'=' {
                at += 1;
            }
            key = slice(text, j, at).trim().to_string();
        }
        at = skip_filler(bytes, at, false);
        if bytes.get(at) != Some(&b'=') || at >= eol {
            i = next_line(j);
            continue;
        }
        let value_start = skip_filler(bytes, at + 1, false);
        if value_start >= eol || bytes.get(value_start) == Some(&b'#') {
            i = next_line(j);
            continue;
        }
        let (value, value_end) = parse_value(text, value_start, false);
        items.push(Item::Kv {
            key,
            value,
            value_start,
            value_end,
            line_end: line_end_at(bytes, value_end),
        });
        i = next_line(value_end.min(n));
    }
    items
}

fn string_of(value: &Val) -> String {
    value.string().unwrap_or_default().to_string()
}

/// Read the resolver's view of a `config.toml` text. Comments are skipped;
/// `[trust] pairings` is read only as an inline array of inline tables; a
/// legacy `[client] token` folds in as one credential with no member.
pub fn parse_config(text: &str) -> Result<ConfigView> {
    let mut view = ConfigView::default();
    let mut legacy_token: Option<String> = None;
    let mut table = String::new();
    let mut current: Option<usize> = None;
    for item in scan_config(text) {
        match item {
            Item::Table { array, name, .. } => {
                current = None;
                if array && name == "credential" {
                    view.credentials.push(Credential {
                        url: String::new(),
                        member: None,
                        token: String::new(),
                        default: false,
                    });
                    current = Some(view.credentials.len() - 1);
                }
                table = name;
            }
            Item::Kv { key, value, .. } => {
                if table == "trust" && key == "pairings" {
                    if let Val::Array(entries) = &value {
                        view.trust.pairings = entries
                            .iter()
                            .filter_map(|entry| {
                                let path = entry.field("path")?.string()?;
                                let url = entry.field("url")?.string()?;
                                if path.is_empty() || url.is_empty() {
                                    return None;
                                }
                                Some(Pairing {
                                    path: path.to_string(),
                                    url: url.to_string(),
                                    member: entry
                                        .field("member")
                                        .and_then(Val::string)
                                        .map(str::to_string),
                                })
                            })
                            .collect();
                    }
                } else if table == "client" {
                    match key.as_str() {
                        "url" => view.client.url = Some(string_of(&value)),
                        "member" => view.client.member = Some(string_of(&value)),
                        "token" => legacy_token = Some(string_of(&value)),
                        _ => {}
                    }
                } else if let Some(index) = current {
                    let credential = &mut view.credentials[index];
                    match key.as_str() {
                        "url" => credential.url = string_of(&value),
                        "member" => credential.member = Some(string_of(&value)),
                        "token" => credential.token = string_of(&value),
                        "default" => {
                            credential.default = matches!(&value, Val::Raw(text) if text == "true");
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    if let (Some(token), Some(url)) = (legacy_token.as_deref(), view.client.url.as_deref())
        && present(Some(token)).is_some()
        && present(Some(url)).is_some()
    {
        view.credentials.insert(
            0,
            Credential {
                url: url.to_string(),
                member: None,
                token: token.to_string(),
                default: false,
            },
        );
    }
    Ok(view)
}

/// Read and parse a `config.toml`; a missing or unreadable file reads as an
/// empty one.
pub fn read_config_file(path: &Path) -> ConfigView {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| parse_config(&text).ok())
        .unwrap_or_default()
}

/// How long a writer waits for the config lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteOptions {
    pub wait: Duration,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            wait: Duration::from_secs(5),
        }
    }
}

/// The text change one edit makes.
struct Edit {
    start: usize,
    end: usize,
    text: String,
}

fn apply_edits(text: &str, mut edits: Vec<Edit>) -> String {
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.start));
    let mut out = text.to_string();
    for edit in edits {
        out.replace_range(edit.start..edit.end, &edit.text);
    }
    out
}

fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One `[[credential]]` block of the scanned text.
struct Block {
    fields: Vec<(String, usize, usize, Val)>,
    /// Where a new field goes: the end of the block's last item.
    last_line_end: usize,
}

impl Block {
    fn string(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, ..)| k == key)
            .and_then(|(.., value)| value.string())
    }

    /// The change that sets a key: the value's whole span when the key exists
    /// (its inline comment stays), else a new line after the last item.
    fn set(&self, key: &str, value: &str, eol: &str) -> Edit {
        match self.fields.iter().find(|(k, ..)| k == key) {
            Some((_, start, end, _)) => Edit {
                start: *start,
                end: *end,
                text: quote(value),
            },
            None => Edit {
                start: self.last_line_end,
                end: self.last_line_end,
                text: format!("{eol}{key} = {}", quote(value)),
            },
        }
    }
}

fn credential_blocks(items: &[Item]) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut open = false;
    for item in items {
        match item {
            Item::Table {
                array,
                name,
                line_end,
            } => {
                open = *array && name == "credential";
                if open {
                    blocks.push(Block {
                        fields: Vec::new(),
                        last_line_end: *line_end,
                    });
                }
            }
            Item::Kv {
                key,
                value,
                value_start,
                value_end,
                line_end,
            } if open => {
                if let Some(block) = blocks.last_mut() {
                    if !block.fields.iter().any(|(k, ..)| k == key) {
                        block
                            .fields
                            .push((key.clone(), *value_start, *value_end, value.clone()));
                    }
                    block.last_line_end = *line_end;
                }
            }
            Item::Kv { .. } => {}
        }
    }
    blocks
}

/// Whether the text ends in a line with nothing on it.
fn ends_with_blank_line(text: &str) -> bool {
    let Some(body) = text.strip_suffix('\n') else {
        return false;
    };
    let body = body.strip_suffix('\r').unwrap_or(body);
    let last = body.rsplit('\n').next().unwrap_or("");
    last.chars().all(|c| c == ' ' || c == '\t')
}

fn append_block(text: &str, eol: &str, lines: &[String]) -> String {
    let final_newline = text.is_empty() || text.ends_with('\n');
    let mut out = text.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push_str(eol);
    }
    if !out.is_empty() && !ends_with_blank_line(&out) {
        out.push_str(eol);
    }
    out.push_str(&lines.join(eol));
    out.push_str(eol);
    if final_newline {
        out
    } else {
        out[..out.len() - eol.len()].to_string()
    }
}

fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".tmp.{}", std::process::id()));
    let temp = PathBuf::from(name);
    let result = lockdir::create_private(&temp, text.as_bytes()).and_then(|()| {
        std::fs::rename(&temp, path)?;
        #[cfg(unix)]
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty())
            && let Ok(dir) = std::fs::File::open(parent)
        {
            let _ = dir.sync_all();
        }
        Ok(())
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// Lock the file, read it, let `mutate` return the new text, and rename it
/// into place. A change that alters nothing writes nothing.
fn edit(
    path: &Path,
    options: &WriteOptions,
    mutate: impl FnOnce(&str, &[Item], &str) -> String,
) -> Result<()> {
    let policy = LockPolicy {
        wait: options.wait,
        ..LockPolicy::CONFIG
    };
    let _lock = lockdir::acquire(path, policy)?;
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(Error::Io(err)),
    };
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let items = scan_config(&text);
    let next = mutate(&text, &items, eol);
    if next != text {
        write_atomic(path, &next).map_err(Error::Io)?;
    }
    Ok(())
}

/// Store a token for (url, member): replace the token of the matching entry
/// (normalised URL, member compared exactly after trim) or append a
/// `[[credential]]`. Locked read, modify, write to a 0600 temp, fsync, rename.
/// A legacy entry for the URL with no member is left alone.
pub fn write(
    path: &Path,
    url: &str,
    member: &str,
    token: &str,
    options: &WriteOptions,
) -> Result<()> {
    edit(path, options, |text, items, eol| {
        let blocks = credential_blocks(items);
        let hit = blocks.iter().find(|block| {
            block.string("url").is_some_and(|u| same(u, url))
                && present(block.string("member")) == present(Some(member))
        });
        match hit {
            Some(block) => apply_edits(text, vec![block.set("token", token, eol)]),
            None => append_block(
                text,
                eol,
                &[
                    "[[credential]]".to_string(),
                    format!("url = {}", quote(url)),
                    format!("member = {}", quote(member)),
                    format!("token = {}", quote(token)),
                ],
            ),
        }
    })
}

/// Rename a member in place for one URL, after an approval under another
/// name. The token stays.
pub fn rekey(path: &Path, url: &str, from: &str, to: &str, options: &WriteOptions) -> Result<()> {
    edit(path, options, |text, items, eol| {
        let edits: Vec<Edit> = credential_blocks(items)
            .iter()
            .filter(|block| {
                block.string("url").is_some_and(|u| same(u, url))
                    && present(block.string("member")) == present(Some(from))
            })
            .map(|block| block.set("member", to, eol))
            .collect();
        apply_edits(text, edits)
    })
}

fn inline_pairing(pairing: &Pairing) -> String {
    let mut text = format!(
        "{{ path = {}, url = {}",
        quote(&pairing.path),
        quote(&pairing.url)
    );
    if let Some(member) = pairing.member.as_deref() {
        text.push_str(&format!(", member = {}", quote(member)));
    }
    text.push_str(" }");
    text
}

/// Where an array value closes and where its last element ends.
fn array_layout(text: &str, from: usize) -> Option<(usize, Option<usize>)> {
    let bytes = text.as_bytes();
    if bytes.get(from) != Some(&b'[') {
        return None;
    }
    let mut last_end = None;
    let mut i = from + 1;
    while i < bytes.len() {
        i = skip_filler(bytes, i, true);
        match bytes.get(i) {
            None => return None,
            Some(b']') => return Some((i, last_end)),
            Some(b',') => i += 1,
            Some(_) => {
                let (_, end) = parse_value(text, i, true);
                last_end = Some(end);
                i = end.max(i + 1);
            }
        }
    }
    None
}

/// Record a pairing under `[trust] pairings`, in the inline-array form. A
/// pairing already there changes nothing.
pub fn trust(path: &Path, pairing: &Pairing, options: &WriteOptions) -> Result<()> {
    edit(path, options, |text, items, eol| {
        let entry = inline_pairing(pairing);
        let mut in_trust = false;
        let mut trust_header_end: Option<usize> = None;
        let mut trust_last_end: Option<usize> = None;
        let mut pairings: Option<(usize, usize, Val)> = None;
        for item in items {
            match item {
                Item::Table {
                    array,
                    name,
                    line_end,
                } => {
                    in_trust = !*array && name == "trust";
                    if in_trust && trust_header_end.is_none() {
                        trust_header_end = Some(*line_end);
                        trust_last_end = Some(*line_end);
                    }
                }
                Item::Kv {
                    key,
                    value,
                    value_start,
                    value_end,
                    line_end,
                } if in_trust => {
                    trust_last_end = Some(*line_end);
                    if key == "pairings" && pairings.is_none() {
                        pairings = Some((*value_start, *value_end, value.clone()));
                    }
                }
                Item::Kv { .. } => {}
            }
        }

        if let Some((start, end, value)) = pairings {
            let known = match &value {
                Val::Array(entries) => entries.iter().any(|entry| {
                    entry.field("path").and_then(Val::string) == Some(pairing.path.as_str())
                        && entry
                            .field("url")
                            .and_then(Val::string)
                            .is_some_and(|u| same(u, &pairing.url))
                        && present(entry.field("member").and_then(Val::string))
                            == present(pairing.member.as_deref())
                }),
                _ => false,
            };
            if known {
                return text.to_string();
            }
            let edit = match (&value, array_layout(text, start)) {
                (Val::Array(_), Some((close, Some(last_end)))) => {
                    let multiline = slice(text, start, end).contains('\n');
                    let insert = if multiline {
                        let line_start = text[..last_end].rfind('\n').map_or(0, |p| p + 1);
                        let indent: String = text[line_start..]
                            .chars()
                            .take_while(|c| *c == ' ' || *c == '\t')
                            .collect();
                        format!(",{eol}{indent}{entry}")
                    } else {
                        format!(", {entry}")
                    };
                    let _ = close;
                    Edit {
                        start: last_end,
                        end: last_end,
                        text: insert,
                    }
                }
                (Val::Array(_), Some((close, None))) => Edit {
                    start,
                    end: close + 1,
                    text: format!("[ {entry} ]"),
                },
                _ => Edit {
                    start,
                    end,
                    text: format!("[ {entry} ]"),
                },
            };
            return apply_edits(text, vec![edit]);
        }

        let line = format!("pairings = [ {entry} ]");
        match trust_last_end {
            Some(at) => apply_edits(
                text,
                vec![Edit {
                    start: at,
                    end: at,
                    text: format!("{eol}{line}"),
                }],
            ),
            None => append_block(text, eol, &["[trust]".to_string(), line]),
        }
    })
}

/// The most of a project file that is read.
const PROJECT_FILE_MAX: u64 = 16 * 1024;

fn read_small(path: &Path) -> std::io::Result<Option<String>> {
    use std::io::Read;
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    if !meta.is_file() {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(PROJECT_FILE_MAX)
        .read_to_end(&mut bytes)?;
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

/// The project file at a git top level, with any context lines (a legacy
/// `.agent-hub` file found beside it). A `token` key is an error.
pub fn read_project_file(root: &Path) -> Result<(Option<ProjectFile>, Vec<String>)> {
    let mut lines = Vec::new();
    if let Ok(Some(text)) = read_small(&root.join(".agent-hub")) {
        let first = text.split('\n').next().unwrap_or_default().trim();
        if !first.is_empty() {
            lines.push(format!(
                "found .agent-hub; this release reads .agent-hub.toml: project = \"{first}\""
            ));
        }
    }
    let path = root.join(".agent-hub.toml");
    let text = read_small(&path)
        .map_err(|err| Error::Config(format!("{} could not be read: {err}", path.display())))?;
    let Some(text) = text else {
        return Ok((None, lines));
    };
    let mut file = ProjectFile::default();
    let mut in_top = true;
    for item in scan_config(&text) {
        match item {
            Item::Table { .. } => in_top = false,
            Item::Kv { key, value, .. } if in_top => match key.as_str() {
                "token" => {
                    return Err(Error::Config(
                        ".agent-hub.toml must not hold a token; it is shared".to_string(),
                    ));
                }
                "project" => file.project = value.string().map(str::to_string),
                "url" => file.url = value.string().map(str::to_string),
                "member" => file.member = value.string().map(str::to_string),
                _ => {}
            },
            Item::Kv { .. } => {}
        }
    }
    Ok((Some(file), lines))
}

/// The git top level above `start`, if any: the nearest ancestor holding a
/// `.git` directory or file.
pub fn project_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
}
