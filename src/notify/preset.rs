//! What one notice looks like on the wire, per service.
//!
//! A preset is a payload shape and a URL pattern, nothing more: the request is
//! built here from the notice's facts and the target's content level, and the
//! sender in the parent module puts it on the wire. Everything here is pure,
//! so each shape is held by a unit test without a server.

use hyper::Method;
use hyper::header::{AUTHORIZATION, CONTENT_TYPE, HeaderName, HeaderValue};
use serde::Serialize;
use serde_json::{Value, json};

use super::{BODY, Content, Event};
use crate::error::{Error, Result};

/// The longest summary a notice carries, in characters.
const SUMMARY_CHARS_MAX: usize = 1000;

/// A service the hub knows the shape of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    Webhook,
    Ntfy,
    Gotify,
    Matrix,
    Slack,
    Discord,
    Telegram,
    Pushover,
}

/// What the interface needs to offer a preset.
#[derive(Debug, Serialize)]
pub struct PresetInfo {
    pub preset: Preset,
    pub label: &'static str,
    /// The URL pattern, shown as the field's placeholder.
    pub url_hint: &'static str,
    /// Whether a separate token field applies. The others carry their secret
    /// in the URL.
    pub takes_token: bool,
    /// The level a new target starts at.
    pub default_content: Content,
    /// Whether the service is a vendor's cloud rather than one the operator
    /// can run, so the interface can say what leaves the network.
    pub vendor_cloud: bool,
}

impl Preset {
    pub const ALL: [Preset; 8] = [
        Preset::Webhook,
        Preset::Ntfy,
        Preset::Gotify,
        Preset::Matrix,
        Preset::Slack,
        Preset::Discord,
        Preset::Telegram,
        Preset::Pushover,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Webhook => "webhook",
            Self::Ntfy => "ntfy",
            Self::Gotify => "gotify",
            Self::Matrix => "matrix",
            Self::Slack => "slack",
            Self::Discord => "discord",
            Self::Telegram => "telegram",
            Self::Pushover => "pushover",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|preset| preset.as_str() == value)
    }

    pub fn info(self) -> PresetInfo {
        let (label, url_hint, takes_token, default_content, vendor_cloud) = match self {
            Self::Webhook => (
                "Webhook",
                "https://hooks.lan/agent-hub",
                true,
                Content::None,
                false,
            ),
            Self::Ntfy => (
                "ntfy",
                "https://ntfy.lan/agent-hub",
                true,
                Content::None,
                false,
            ),
            // Gotify has no hosted service: a Gotify server is the operator's own.
            Self::Gotify => (
                "Gotify",
                "https://gotify.lan/message",
                true,
                Content::Title,
                false,
            ),
            Self::Matrix => (
                "Matrix",
                "https://matrix.lan/_matrix/client/v3/rooms/!room:matrix.lan/send/m.room.message",
                true,
                Content::None,
                false,
            ),
            Self::Slack => (
                "Slack",
                "https://hooks.slack.com/services/T000/B000/XXXX",
                false,
                Content::None,
                true,
            ),
            Self::Discord => (
                "Discord",
                "https://discord.com/api/webhooks/000/XXXX",
                false,
                Content::None,
                true,
            ),
            Self::Telegram => (
                "Telegram",
                "https://api.telegram.org/bot000:XXXX/sendMessage?chat_id=000",
                false,
                Content::None,
                true,
            ),
            Self::Pushover => (
                "Pushover",
                "https://api.pushover.net/1/messages.json?token=XXXX&user=XXXX",
                false,
                Content::None,
                true,
            ),
        };
        PresetInfo {
            preset: self,
            label,
            url_hint,
            takes_token,
            default_content,
            vendor_cloud,
        }
    }

    /// Check the parts of a URL this preset needs, beyond it being an http or
    /// https URL with a host.
    pub fn check(self, url: &url::Url, token: Option<&str>) -> Result<()> {
        let invalid = |message: &str| Err(Error::InvalidArgument(message.to_string()));
        if token.is_some() && !self.info().takes_token {
            return invalid(&format!(
                "a {} target takes no token: its URL carries the secret",
                self.info().label
            ));
        }
        match self {
            Self::Matrix if !url.path().contains("/send/") => {
                invalid("a Matrix URL ends in /rooms/<room>/send/m.room.message on your homeserver")
            }
            Self::Telegram if query(url, "chat_id").is_none() => {
                invalid("a Telegram URL names the chat: .../sendMessage?chat_id=<id>")
            }
            Self::Pushover if query(url, "token").is_none() || query(url, "user").is_none() => {
                invalid("a Pushover URL carries both keys: ...?token=<app>&user=<user>")
            }
            _ => Ok(()),
        }
    }
}

fn query(url: &url::Url, key: &str) -> Option<String> {
    url.query_pairs()
        .find(|(name, value)| name == key && !value.is_empty())
        .map(|(_, value)| value.into_owned())
}

/// What a notice is about, resolved by the dispatcher. A field the content
/// level withholds is still here; [`render`] decides what goes out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    pub event: Event,
    /// `question`, `approval`, `idea`, `enrolment`, `session`, or `test`.
    pub kind: String,
    /// The event, session or member the notice is about.
    pub id: Option<String>,
    pub project: Option<String>,
    pub project_name: Option<String>,
    pub session: Option<String>,
    pub summary: Option<String>,
    pub due_in_mins: Option<i64>,
    /// A project that needs a grant: its notices go out with no content.
    pub confidential: bool,
    /// How many notices this one stands for, after coalescing.
    pub count: usize,
    pub at: String,
}

impl Facts {
    /// The key a target dedupes on: one notice per event and subject.
    pub fn key(&self) -> String {
        format!(
            "{}:{}",
            self.event.as_str(),
            self.id.as_deref().unwrap_or("")
        )
    }
}

/// One request, ready for the sender.
#[derive(Debug)]
pub struct Outbound {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(HeaderName, HeaderValue)>,
    pub body: Vec<u8>,
}

/// The words of a notice at one content level.
#[derive(Debug, PartialEq, Eq)]
struct Words {
    headline: String,
    detail: Option<String>,
    link: Option<String>,
}

impl Words {
    fn text(&self) -> String {
        self.text_with(str::to_string)
    }

    /// The text with `escape` applied to the words, which carry agent text,
    /// and not to the link, which the hub built.
    fn text_with(&self, escape: impl Fn(&str) -> String) -> String {
        [
            Some(escape(&self.headline)),
            self.detail.as_deref().map(&escape),
            self.link.clone(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n")
    }

    /// For a service with a title and a message: the headline is the title
    /// only when there is a detail to carry as the message.
    fn title_and_message(&self) -> (Option<&str>, &str) {
        match &self.detail {
            Some(detail) => (Some(self.headline.as_str()), detail.as_str()),
            None => (None, self.headline.as_str()),
        }
    }
}

/// The level a notice actually goes out at: a confidential project's never
/// says more than that something happened.
pub fn effective(content: Content, facts: &Facts) -> Content {
    if facts.confidential {
        Content::None
    } else {
        content
    }
}

fn kind_word(kind: &str) -> &'static str {
    match kind {
        "question" => "Question",
        "approval" => "Approval",
        "idea" => "Idea",
        "enrolment" => "Enrolment request",
        "session" => "Session",
        _ => "Item",
    }
}

fn words(content: Content, facts: &Facts, public_url: Option<&str>) -> Words {
    let level = effective(content, facts);
    if level == Content::None {
        let headline = match facts.event {
            Event::Ask => BODY,
            Event::Deadline => "Something waiting for you in Agent Hub is near its deadline.",
            Event::Ended => "A session ended in Agent Hub.",
            Event::Test => "Test notification from Agent Hub.",
        };
        return Words {
            headline: headline.to_string(),
            detail: None,
            link: None,
        };
    }
    let project = facts
        .project_name
        .as_deref()
        .or(facts.project.as_deref())
        .unwrap_or("Agent Hub");
    let in_session = facts
        .session
        .as_deref()
        .map(|session| format!(" · {session}"))
        .unwrap_or_default();
    let kind = kind_word(&facts.kind);
    let headline = if facts.count > 1 {
        format!("{} updates in Agent Hub", facts.count)
    } else {
        match facts.event {
            Event::Ask if facts.kind == "enrolment" => "Enrolment request waiting".to_string(),
            Event::Ask => format!("{kind} waiting in {project}{in_session}"),
            Event::Deadline => format!(
                "{kind} in {project} is due in {} min",
                facts.due_in_mins.unwrap_or(1).max(1)
            ),
            Event::Ended => format!(
                "Session {} ended in {project}",
                facts.session.as_deref().unwrap_or("")
            ),
            Event::Test => "Test notification from Agent Hub".to_string(),
        }
    };
    // An enrolment request is written by a caller that holds no token, so its
    // text never leaves, at any level.
    let detail = match level {
        Content::Full if facts.count <= 1 && facts.kind != "enrolment" => facts
            .summary
            .as_deref()
            .map(|summary| summary.chars().take(SUMMARY_CHARS_MAX).collect()),
        _ => None,
    };
    let link = public_url.map(|origin| {
        let origin = origin.trim_end_matches('/');
        match (&facts.id, facts.kind.as_str()) {
            _ if facts.count > 1 => format!("{origin}/"),
            (_, "enrolment") => format!("{origin}/#/access"),
            (Some(id), _) => crate::http::origin::resource_url(origin, id),
            (None, _) => format!("{origin}/"),
        }
    });
    Words {
        headline,
        detail,
        link,
    }
}

/// The webhook preset's stable body. A key the level withholds is absent.
fn webhook_body(content: Content, facts: &Facts, words: &Words) -> Value {
    let mut body = json!({
        "version": 1,
        "event": facts.event.as_str(),
        "text": words.headline,
        "at": facts.at,
    });
    let level = effective(content, facts);
    if level == Content::None {
        return body;
    }
    let object = body.as_object_mut().expect("an object");
    let mut put = |key: &str, value: Option<Value>| {
        if let Some(value) = value {
            object.insert(key.to_string(), value);
        }
    };
    put("kind", Some(Value::from(facts.kind.clone())));
    put("project", facts.project.clone().map(Value::from));
    put("project_name", facts.project_name.clone().map(Value::from));
    put("session", facts.session.clone().map(Value::from));
    // An enrolment's id is the member name its caller asked for.
    put(
        "id",
        facts
            .id
            .clone()
            .filter(|_| facts.kind != "enrolment")
            .map(Value::from),
    );
    put("due_in_minutes", facts.due_in_mins.map(Value::from));
    put(
        "count",
        (facts.count > 1).then(|| Value::from(facts.count as u64)),
    );
    put("url", words.link.clone().map(Value::from));
    put("summary", words.detail.clone().map(Value::from));
    body
}

fn json_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("a JSON value serialises")
}

fn header(value: &str) -> std::result::Result<HeaderValue, String> {
    let mut value =
        HeaderValue::from_str(value).map_err(|_| "the token is not a header value".to_string())?;
    value.set_sensitive(true);
    Ok(value)
}

/// Text for a Slack-shaped webhook, which Mattermost and Rocket.Chat also
/// read: Slack's three control characters are escaped, so `<!channel>` or a
/// `<url|label>` link in agent text is shown and not acted on, and an `@` is
/// followed by a zero-width space, so a bare `@channel` or `@all` pings nobody
/// on the services that read those.
fn slack_plain(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('@', "@\u{200B}")
}

/// Text for Discord: every Markdown control character is escaped with a
/// backslash, so a masked link such as `[x](https://example.com)`, a heading or
/// a spoiler in agent text is shown as written and not rendered.
fn discord_plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(
            ch,
            '\\' | '*' | '_' | '~' | '`' | '|' | '>' | '[' | ']' | '(' | ')' | '#' | '-'
        ) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Build the request a target sends for a notice. `txn` names this send, so a
/// retry of it is the same transaction on a service that dedupes by one.
pub fn render(
    preset: Preset,
    url: &url::Url,
    token: Option<&str>,
    content: Content,
    facts: &Facts,
    public_url: Option<&str>,
    txn: &str,
) -> std::result::Result<Outbound, String> {
    let words = words(content, facts, public_url);
    let json_type = (CONTENT_TYPE, HeaderValue::from_static("application/json"));
    let mut headers = Vec::new();
    let mut target = url.clone();
    let mut method = Method::POST;
    let body = match preset {
        Preset::Webhook => {
            headers.push(json_type);
            json_bytes(&webhook_body(content, facts, &words))
        }
        Preset::Ntfy => {
            headers.push((
                CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            ));
            // ntfy reads its message options from the query. One the operator
            // set on the URL is theirs and stays, except markdown: a notice
            // that carries agent text is always shown as plain text.
            if effective(content, facts) > Content::None
                && let Some(query) = target.query().map(str::to_string)
            {
                // The operator's own pairs are kept as written, not re-encoded.
                let kept: Vec<&str> = query
                    .split('&')
                    .filter(|pair| !matches!(pair.split('=').next(), Some("markdown" | "md")))
                    .collect();
                target.set_query((!kept.is_empty()).then(|| kept.join("&")).as_deref());
            }
            let (title, message) = words.title_and_message();
            let held: Vec<String> = target
                .query_pairs()
                .map(|(key, _)| key.into_owned())
                .collect();
            let mut add = Vec::new();
            if let Some(title) = title {
                add.push(("title", title.to_string()));
            }
            if let Some(link) = &words.link {
                add.push(("click", link.clone()));
            }
            let add: Vec<_> = add
                .into_iter()
                .filter(|(key, _)| !held.iter().any(|held| held == key))
                .collect();
            if !add.is_empty() {
                let mut pairs = target.query_pairs_mut();
                for (key, value) in add {
                    pairs.append_pair(key, &value);
                }
            }
            message.as_bytes().to_vec()
        }
        Preset::Gotify => {
            headers.push(json_type);
            if let Some(token) = token {
                headers.push((HeaderName::from_static("x-gotify-key"), header(token)?));
            }
            let (title, message) = words.title_and_message();
            // Plain text, so agent text is never rendered as markdown.
            let mut body = json!({
                "message": message,
                "priority": 5,
                "extras": { "client::display": { "contentType": "text/plain" } },
            });
            if let Some(title) = title {
                body["title"] = Value::from(title);
            }
            if let Some(link) = &words.link {
                body["extras"]["client::notification"] = json!({ "click": { "url": link } });
            }
            json_bytes(&body)
        }
        Preset::Matrix => {
            headers.push(json_type);
            method = Method::PUT;
            let path = format!(
                "{}/{}",
                target.path().trim_end_matches('/'),
                crate::http::origin::encode(txn)
            );
            target.set_path(&path);
            // A plain body with no formatted body, and an empty mention set,
            // which turns off the rules that ping a room on "@room" in a body.
            json_bytes(&json!({ "msgtype": "m.text", "body": words.text(), "m.mentions": {} }))
        }
        Preset::Slack => {
            headers.push(json_type);
            json_bytes(&json!({ "text": slack_plain(&words.text()) }))
        }
        Preset::Discord => {
            headers.push(json_type);
            json_bytes(&json!({
                "content": words.text_with(discord_plain),
                "allowed_mentions": { "parse": [] },
            }))
        }
        Preset::Telegram => {
            headers.push(json_type);
            let chat = query(&target, "chat_id").ok_or("the URL names no chat_id")?;
            target.set_query(None);
            json_bytes(&json!({
                "chat_id": chat,
                "text": words.text(),
                "disable_web_page_preview": true,
            }))
        }
        Preset::Pushover => {
            headers.push(json_type);
            let app = query(&target, "token").ok_or("the URL carries no token")?;
            let user = query(&target, "user").ok_or("the URL carries no user")?;
            target.set_query(None);
            let (title, message) = words.title_and_message();
            let mut body = json!({ "token": app, "user": user, "message": message });
            if let Some(title) = title {
                body["title"] = Value::from(title);
            }
            if let Some(link) = &words.link {
                body["url"] = Value::from(link.as_str());
                body["url_title"] = Value::from("Open in Agent Hub");
            }
            json_bytes(&body)
        }
    };
    if let Some(token) = token
        && matches!(preset, Preset::Webhook | Preset::Ntfy | Preset::Matrix)
    {
        headers.push((AUTHORIZATION, header(&format!("Bearer {token}"))?));
    }
    Ok(Outbound {
        method,
        url: target.to_string(),
        headers,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(event: Event) -> Facts {
        Facts {
            event,
            kind: "question".to_string(),
            id: Some("01JQUESTION".to_string()),
            project: Some("zebra-launch".to_string()),
            project_name: Some("Zebra launch".to_string()),
            session: Some("nightly".to_string()),
            summary: Some("Ship the nightly build?".to_string()),
            due_in_mins: None,
            confidential: false,
            count: 1,
            at: "2026-11-26T10:00:00Z".to_string(),
        }
    }

    fn body(out: &Outbound) -> Value {
        serde_json::from_slice(&out.body).expect("a JSON body")
    }

    fn send(preset: Preset, url: &str, content: Content, facts: &Facts) -> Outbound {
        render(
            preset,
            &url.parse().unwrap(),
            None,
            content,
            facts,
            Some("https://hub.lan"),
            "ask-01JQUESTION",
        )
        .unwrap()
    }

    #[test]
    fn none_says_only_the_fixed_sentence() {
        let out = send(
            Preset::Webhook,
            "http://hook.lan/x",
            Content::None,
            &facts(Event::Ask),
        );
        assert_eq!(
            body(&out),
            json!({"version": 1, "event": "ask", "text": BODY, "at": "2026-11-26T10:00:00Z"})
        );
        let text = String::from_utf8(
            send(
                Preset::Ntfy,
                "http://ntfy.lan/t",
                Content::None,
                &facts(Event::Ask),
            )
            .body,
        )
        .unwrap();
        assert_eq!(text, BODY);
    }

    #[test]
    fn title_names_the_kind_project_and_session_with_a_link() {
        let out = send(
            Preset::Slack,
            "https://hooks.slack.com/s/x",
            Content::Title,
            &facts(Event::Ask),
        );
        assert_eq!(
            body(&out)["text"],
            "Question waiting in Zebra launch · nightly\nhttps://hub.lan/id/01JQUESTION"
        );
    }

    #[test]
    fn full_adds_the_summary_and_a_confidential_project_falls_to_none() {
        let mut facts = facts(Event::Ask);
        let out = send(
            Preset::Discord,
            "https://discord.com/api/webhooks/1/x",
            Content::Full,
            &facts,
        );
        assert!(
            body(&out)["content"]
                .as_str()
                .unwrap()
                .contains("Ship the nightly build?")
        );
        facts.confidential = true;
        let out = send(
            Preset::Discord,
            "https://discord.com/api/webhooks/1/x",
            Content::Full,
            &facts,
        );
        assert_eq!(body(&out)["content"], BODY);
    }

    #[test]
    fn telegram_and_pushover_move_their_keys_into_the_body() {
        let facts = facts(Event::Ask);
        let out = send(
            Preset::Telegram,
            "https://api.telegram.org/bot1:abc/sendMessage?chat_id=42",
            Content::None,
            &facts,
        );
        assert_eq!(out.url, "https://api.telegram.org/bot1:abc/sendMessage");
        assert_eq!(body(&out)["chat_id"], "42");
        let out = send(
            Preset::Pushover,
            "https://api.pushover.net/1/messages.json?token=app&user=me",
            Content::None,
            &facts,
        );
        assert_eq!(out.url, "https://api.pushover.net/1/messages.json");
        assert_eq!(
            body(&out),
            json!({"token": "app", "user": "me", "message": BODY})
        );
    }

    #[test]
    fn matrix_puts_to_a_transaction_path() {
        let out = send(
            Preset::Matrix,
            "https://m.lan/_matrix/client/v3/rooms/!r:m.lan/send/m.room.message",
            Content::None,
            &facts(Event::Ask),
        );
        assert_eq!(out.method, Method::PUT);
        assert!(
            out.url.ends_with("/send/m.room.message/ask-01JQUESTION"),
            "{}",
            out.url
        );
    }

    #[test]
    fn ntfy_keeps_the_operators_own_query() {
        let out = send(
            Preset::Ntfy,
            "http://ntfy.lan/t?click=https://mine",
            Content::Title,
            &facts(Event::Ask),
        );
        assert!(out.url.contains("click=https://mine"), "{}", out.url);
        assert_eq!(out.url.matches("click=").count(), 1, "{}", out.url);
    }

    #[test]
    fn agent_text_cannot_ping_a_room_or_turn_on_markup() {
        let mut facts = facts(Event::Ask);
        facts.summary =
            Some("<!channel> @all @room <https://x.example|click> & **bold**".to_string());
        let send = |preset, url: &str| {
            render(
                preset,
                &url.parse().unwrap(),
                None,
                Content::Full,
                &facts,
                None,
                "ask-1",
            )
            .unwrap()
        };

        let slack = body(&send(Preset::Slack, "https://hooks.slack.com/s/x"))["text"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(!slack.contains('<') && !slack.contains('>'), "{slack}");
        assert!(
            slack.contains("&lt;!channel&gt;") && slack.contains("&amp;"),
            "{slack}"
        );
        assert!(!slack.contains("@all"), "a bare mention survived: {slack}");

        let discord = body(&send(
            Preset::Discord,
            "https://discord.com/api/webhooks/1/x",
        ));
        assert_eq!(discord["allowed_mentions"], json!({"parse": []}));

        let telegram = body(&send(
            Preset::Telegram,
            "https://api.telegram.org/bot1:a/sendMessage?chat_id=1&parse_mode=HTML",
        ));
        assert!(telegram.get("parse_mode").is_none(), "{telegram}");

        let matrix = body(&send(
            Preset::Matrix,
            "https://m.lan/_matrix/client/v3/rooms/!r:m.lan/send/m.room.message",
        ));
        assert!(matrix.get("formatted_body").is_none() && matrix.get("format").is_none());
        assert_eq!(matrix["m.mentions"], json!({}));

        let gotify = body(&send(Preset::Gotify, "https://gotify.lan/message"));
        assert_eq!(
            gotify["extras"]["client::display"]["contentType"],
            "text/plain"
        );

        let pushover = body(&send(
            Preset::Pushover,
            "https://api.pushover.net/1/messages.json?token=a&user=u&html=1",
        ));
        assert!(pushover.get("html").is_none(), "{pushover}");

        let ntfy = send(
            Preset::Ntfy,
            "https://ntfy.lan/t?markdown=yes&md=1&priority=4",
        );
        assert!(
            !ntfy.url.contains("markdown") && !ntfy.url.contains("md="),
            "{}",
            ntfy.url
        );
        assert!(ntfy.url.contains("priority=4"), "{}", ntfy.url);
    }

    #[test]
    fn discord_shows_markdown_in_agent_text_as_written() {
        let mut facts = facts(Event::Ask);
        facts.summary = Some(r"[x](https://evil.example) # head -- ||s|| `c` \".to_string());
        let out = send(
            Preset::Discord,
            "https://discord.com/api/webhooks/1/x",
            Content::Full,
            &facts,
        );
        let content = body(&out)["content"].as_str().unwrap().to_string();
        assert!(
            content.contains(r"\[x\]\(https://evil.example\)"),
            "a masked link survived: {content}"
        );
        assert!(
            content.contains(r"\# head \-\- \|\|s\|\| \`c\` \\"),
            "{content}"
        );
        assert!(
            content.ends_with("\nhttps://hub.lan/id/01JQUESTION"),
            "the hub's own link is left as it is: {content}"
        );
    }

    #[test]
    fn an_enrolment_never_carries_the_requesters_text() {
        let mut facts = facts(Event::Ask);
        facts.kind = "enrolment".to_string();
        facts.id = Some("evil-host".to_string());
        facts.project = None;
        facts.project_name = None;
        facts.session = None;
        facts.summary = Some("Evil host: <!channel> please approve".to_string());
        for preset in [Preset::Webhook, Preset::Slack] {
            let out = send(preset, "https://hooks.example/x", Content::Full, &facts);
            let text = String::from_utf8(out.body).unwrap();
            assert!(
                !text.contains("Evil") && !text.contains("evil-host"),
                "{text}"
            );
            assert!(text.contains("Enrolment request waiting"), "{text}");
            assert!(text.contains("https://hub.lan/#/access"), "{text}");
        }
    }

    #[test]
    fn a_preset_checks_the_url_parts_it_needs() {
        let url: url::Url = "https://api.telegram.org/bot1:abc/sendMessage"
            .parse()
            .unwrap();
        assert!(Preset::Telegram.check(&url, None).is_err());
        let url: url::Url = "https://hooks.slack.com/services/x".parse().unwrap();
        assert!(Preset::Slack.check(&url, Some("tk")).is_err());
        assert!(Preset::Slack.check(&url, None).is_ok());
    }
}
