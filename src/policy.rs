//! Authorization policy: what a principal may read and write.
//!
//! This is the single seam the tools and routes call before touching a
//! project. It fails closed: until the trust, grant, and personal-space rules
//! are filled in, every request is refused.

use turso::Database;

use crate::error::{Error, Result};
use crate::principal::Principal;

/// The access a caller asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Read a project's feed, artifacts, inbox, or search results.
    Read,
    /// Write to a project.
    Write,
}

/// Reject a caller that may not reach a project. Refuses every caller until
/// the rules are filled in.
pub async fn authorize(
    _db: &Database,
    _principal: &Principal,
    _project_id: &str,
    _access: Access,
) -> Result<()> {
    Err(Error::Forbidden(
        "authorization is not wired yet".to_string(),
    ))
}
