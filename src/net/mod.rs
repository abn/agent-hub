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

/// Acknowledge that the embedded tailnet uses early-days software, so the
/// library's experimental guard passes. A build without the feature has no
/// guard and does nothing.
///
/// # Safety
///
/// Mutates the process environment, so the caller must be single-threaded and
/// call it before any other thread starts. Call it once from the sync start of
/// `main`, before the async runtime is built.
pub unsafe fn acknowledge_unstable() {
    #[cfg(feature = "tailnet")]
    {
        unsafe {
            std::env::set_var("TS_RS_EXPERIMENT", "this_is_unstable_software");
        }
    }
}

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
