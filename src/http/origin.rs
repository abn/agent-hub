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
