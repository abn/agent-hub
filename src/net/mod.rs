//! The optional embedded tailnet endpoint.
//!
//! Compiled only with the `tailnet` feature. The process joins the tailnet in
//! userspace and serves the same router there, addressed by its tailnet IP.
//!
//! This is experimental: the library provides no tailnet name resolution and
//! no certificate issuance, and its NAT traversal is in progress, so the
//! endpoint is IP-addressed and traffic may relay through a public relay.

#[cfg(feature = "tailnet")]
mod embedded;

#[cfg(feature = "tailnet")]
pub use embedded::serve;

/// Reject a tailnet request on a build without the feature.
#[cfg(not(feature = "tailnet"))]
pub async fn serve(
    _config: &crate::config::Tailnet,
    _router: axum::Router,
) -> crate::error::Result<()> {
    Err(crate::error::Error::Config(
        "the embedded tailnet endpoint needs the tailnet feature".to_string(),
    ))
}
