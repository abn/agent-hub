//! The hub's own absolute origin, as one source every served document shares.
//!
//! The frame policy, the link preview tags, and the served bootstrap skill all
//! have to name the address a caller actually reaches the hub at. They read it
//! from here so a tightening in one place cannot leave another behind.

use axum::http::HeaderMap;

use crate::config::Config;

/// The hub origin to name in a served document.
///
/// `HUB_PUBLIC_URL` wins when it is set, because only the operator knows the
/// address in front of a proxy that rewrites the host. Otherwise the forwarded
/// scheme and host win, then the request host, then the configured bind. Each
/// header source is validated on its own, so an unusable forwarded host falls
/// through to a good request host rather than straight to the bind.
pub fn request_origin(config: &Config, headers: &HeaderMap) -> String {
    if let Some(public_url) = &config.public_url {
        return public_url.clone();
    }
    let scheme = match first_header_value(headers, "x-forwarded-proto").as_deref() {
        Some(value) if value.eq_ignore_ascii_case("https") => "https",
        _ => "http",
    };
    let host = first_header_value(headers, "x-forwarded-host")
        .filter(|value| is_safe_host(value))
        .or_else(|| first_header_value(headers, "host").filter(|value| is_safe_host(value)))
        .unwrap_or_else(|| fallback_authority(config.bind));
    format!("{scheme}://{host}")
}

/// Whether the `Host` a request was made to is one this hub owns, in a way a
/// DNS rebinding cannot forge: an IP literal, a loopback name (`localhost` or a
/// name under it), or the host of `HUB_PUBLIC_URL`. The configured bind is an
/// address, so it is covered by the IP rule. Any other name could have been
/// pointed at the hub by a stranger's DNS, with the browser sending that
/// stranger's name as both `Host` and `Origin`.
pub fn host_is_owned(config: &Config, host: &str) -> bool {
    let Some(name) = host_name(host) else {
        return false;
    };
    match name {
        url::Host::Ipv4(_) | url::Host::Ipv6(_) => true,
        url::Host::Domain(domain) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            domain == "localhost"
                || domain.ends_with(".localhost")
                || config
                    .public_url
                    .as_deref()
                    .and_then(|public| url::Url::parse(public).ok())
                    .and_then(|public| public.host_str().map(str::to_ascii_lowercase))
                    .is_some_and(|public| public.trim_end_matches('.') == domain)
        }
    }
}

/// The host part of a `Host` header value, which may carry a port.
fn host_name(host: &str) -> Option<url::Host<String>> {
    if !is_safe_host(host) {
        return None;
    }
    url::Url::parse(&format!("http://{host}/"))
        .ok()?
        .host()
        .map(|host| host.to_owned())
}

/// The short URL of a hub resource: `{origin}/id/{id}`.
///
/// The one place a resource URL is built. Ids are ULIDs, so there is nothing
/// to escape; the id is still encoded so a stray character cannot break out of
/// the path.
pub fn resource_url(origin: &str, id: &str) -> String {
    format!("{origin}/id/{}", encode(id))
}

/// The URL of a knowledge base page: `{origin}/#/projects/{project}/wiki?page={path}`.
pub fn kb_page_url(origin: &str, project_id: &str, path: &str) -> String {
    format!(
        "{origin}/#/projects/{}/wiki?page={}",
        encode(project_id),
        encode(path)
    )
}

/// What `encodeURIComponent` leaves alone, so a URL built here matches the one
/// the interface builds.
const COMPONENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// Percent-encode one path or query component.
pub fn encode(component: &str) -> String {
    percent_encoding::utf8_percent_encode(component, COMPONENT).to_string()
}

/// The authority to use when no request host is available. An unspecified
/// bind such as `0.0.0.0:8080` is not a usable URL, so it becomes loopback on
/// the same port.
fn fallback_authority(bind: std::net::SocketAddr) -> String {
    if bind.ip().is_unspecified() {
        format!("localhost:{}", bind.port())
    } else {
        bind.to_string()
    }
}

/// The first value of a possibly comma-separated header.
fn first_header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Whether a host is safe to echo into a document. The value lands in a URL
/// in served markup and in a content policy, so it is restricted to the
/// characters a host and optional port can contain.
pub(crate) fn is_safe_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 255
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
}
