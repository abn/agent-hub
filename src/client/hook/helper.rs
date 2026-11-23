//! The credential Claude Code's `headersHelper` sends for the plugin's hub.
//!
//! The helper runs from the plugin's directory with no project and no plugin
//! options, so it cannot see the project file the hooks read. It is given the
//! hub URL and nothing else, and chooses from `config.toml` alone: the only
//! credential for that URL, else the one marked `default = true`. A hook that
//! resolves to another member says so, since its calls and the connection's
//! would otherwise be made by two members.

use crate::client::credential::{Credential, normalise_url};

/// The credential the helper sends for `url`, or `None` when it sends no
/// `Authorization`: no credential for the URL, several with none or more than
/// one marked default.
pub fn choice<'a>(credentials: &'a [Credential], url: &str) -> Option<&'a Credential> {
    let url = normalise_url(url);
    let held: Vec<&Credential> = credentials
        .iter()
        .filter(|c| normalise_url(&c.url) == url && !c.token.trim().is_empty())
        .collect();
    if let [only] = held.as_slice() {
        return Some(only);
    }
    let mut marked = held.into_iter().filter(|c| c.default);
    match (marked.next(), marked.next()) {
        (Some(one), None) => Some(one),
        _ => None,
    }
}

/// The line for a project whose member is not the one the connection
/// authenticates as, or `None` when they agree.
pub fn mismatch(credentials: &[Credential], url: &str, member: Option<&str>) -> Option<String> {
    let member = member?;
    let sent = choice(credentials, url).and_then(|c| c.member.as_deref());
    if sent == Some(member) {
        return None;
    }
    let connection = match sent {
        Some(other) => format!("authenticates as {other}"),
        None => "sends no credential".to_string(),
    };
    Some(format!(
        "This project uses member {member}, but Claude Code's hub connection {connection}. \
         Mark {member} as the default for {url} or use a separate hub entry."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(url: &str, member: &str, default: bool) -> Credential {
        Credential {
            url: url.to_string(),
            member: Some(member.to_string()),
            token: format!("t-{member}"),
            default,
        }
    }

    const HUB: &str = "https://hub.example.net";

    #[test]
    fn the_only_credential_for_the_url_is_sent() {
        let held = [entry(HUB, "a", false), entry("https://other", "b", false)];
        assert_eq!(
            choice(&held, "https://hub.example.net/").map(|c| c.member.as_deref()),
            Some(Some("a"))
        );
    }

    #[test]
    fn with_several_the_one_marked_default_is_sent() {
        let held = [entry(HUB, "a", false), entry(HUB, "b", true)];
        assert_eq!(
            choice(&held, HUB).and_then(|c| c.member.as_deref()),
            Some("b")
        );
    }

    #[test]
    fn with_several_and_no_single_default_nothing_is_sent() {
        assert!(choice(&[entry(HUB, "a", false), entry(HUB, "b", false)], HUB).is_none());
        assert!(choice(&[entry(HUB, "a", true), entry(HUB, "b", true)], HUB).is_none());
        assert!(choice(&[], HUB).is_none());
    }

    #[test]
    fn a_project_on_another_member_is_told_which_one_the_connection_is() {
        let held = [entry(HUB, "a", false), entry(HUB, "b", true)];
        assert_eq!(mismatch(&held, HUB, Some("b")), None);
        assert_eq!(
            mismatch(&held, HUB, Some("a")).as_deref(),
            Some(
                "This project uses member a, but Claude Code's hub connection authenticates as b. \
                 Mark a as the default for https://hub.example.net or use a separate hub entry."
            )
        );
        let none = [entry(HUB, "a", false), entry(HUB, "b", false)];
        assert!(
            mismatch(&none, HUB, Some("a"))
                .is_some_and(|line| line.contains("sends no credential"))
        );
        assert_eq!(mismatch(&none, HUB, None), None);
    }
}
