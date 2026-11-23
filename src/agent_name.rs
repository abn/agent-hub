//! Canonical agent names: the client slug a write is attributed to.
//!
//! The agent comes from the MCP client's `clientInfo.name`, or from an
//! `X-Agent-Id` header, and is folded to one slug per client so the same
//! client reads the same everywhere. Unknown names are kept as sent.

/// The agent a call is attributed to when it names none: the CLI, the REST
/// surface, or a client that sent no handshake name and no header.
pub const CLI: &str = "cli";

/// The agent recorded when a name was sent but cannot be kept.
pub const UNKNOWN: &str = "unknown";

/// The longest raw name kept as sent.
const RAW_MAX: usize = 64;

/// Known clients: each slug and the raw names that fold to it, lowercase.
const TABLE: &[(&str, &[&str])] = &[
    (
        "claude-code",
        &["claude-code", "claude", "claude code", "claude-ai"],
    ),
    ("opencode", &["opencode", "agent-hub-opencode"]),
    ("antigravity", &["antigravity", "antigravity-client", "agy"]),
    (CLI, &["agent-hub", "agent-hub-cli", "cli"]),
];

/// The canonical slug for a raw client name.
///
/// Matching is case-insensitive after trimming. A name no client in the table
/// claims is kept as sent (trimmed) when it is 1 to 64 characters with no
/// control characters, and is [`UNKNOWN`] otherwise. An empty name is [`CLI`],
/// the same as no name at all.
pub fn canonical(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return CLI.to_string();
    }
    let folded = trimmed.to_lowercase();
    for (slug, names) in TABLE {
        if names.contains(&folded.as_str()) {
            return (*slug).to_string();
        }
    }
    if trimmed.chars().count() <= RAW_MAX && !trimmed.chars().any(char::is_control) {
        trimmed.to_string()
    } else {
        UNKNOWN.to_string()
    }
}

/// The agent for a call: the header's name, else the handshake's, else
/// [`CLI`]. Both are canonicalised.
pub fn resolve(header: Option<&str>, client_info_name: Option<&str>) -> String {
    header
        .filter(|name| !name.trim().is_empty())
        .or(client_info_name.filter(|name| !name.trim().is_empty()))
        .map_or_else(|| CLI.to_string(), canonical)
}

/// The session name to suggest when nothing names one: `<agent>-<YYYY-MM-DD>`,
/// with any character a session name cannot hold in the agent turned to `-`.
pub fn suggested_session(agent: &str, date: time::Date) -> String {
    let slug: String = agent
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!(
        "{slug}-{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_clients_fold_to_one_slug() {
        assert_eq!(canonical("claude-code"), "claude-code");
        assert_eq!(canonical("  Claude Code "), "claude-code");
        assert_eq!(canonical("opencode"), "opencode");
        assert_eq!(canonical("agent-hub-opencode"), "opencode");
        assert_eq!(canonical("antigravity-client"), "antigravity");
        assert_eq!(canonical("agy"), "antigravity");
        assert_eq!(canonical("agent-hub"), "cli");
    }

    #[test]
    fn unknown_names_are_kept_unless_unusable() {
        assert_eq!(canonical(" Zed "), "Zed");
        assert_eq!(canonical(&"x".repeat(64)), "x".repeat(64));
        assert_eq!(canonical(&"x".repeat(65)), UNKNOWN);
        assert_eq!(canonical("bad\u{7}name"), UNKNOWN);
        assert_eq!(canonical("   "), CLI);
    }

    #[test]
    fn the_suggested_session_is_the_agent_and_the_day() {
        let day = time::Date::from_calendar_date(2026, time::Month::November, 3).expect("date");
        assert_eq!(
            suggested_session("claude-code", day),
            "claude-code-2026-11-03"
        );
        assert_eq!(
            suggested_session("Zed Editor", day),
            "Zed-Editor-2026-11-03"
        );
    }

    #[test]
    fn the_header_beats_the_handshake_and_nothing_is_the_cli() {
        assert_eq!(resolve(Some("opencode"), Some("claude-code")), "opencode");
        assert_eq!(resolve(None, Some("claude-code")), "claude-code");
        assert_eq!(resolve(Some(" "), None), CLI);
        assert_eq!(resolve(None, None), CLI);
    }
}
