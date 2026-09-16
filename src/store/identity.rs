//! Agent identities, tokens, and grants.
//!
//! The principal seam needs two things from here: hash a bearer token, and
//! resolve a hash to an agent. Agent management, the token lifecycle, grants,
//! and personal spaces build on this store.

use turso::Database;

use crate::error::Result;
use crate::principal::Trust;

const HEX: &[u8; 16] = b"0123456789abcdef";

/// Hash a bearer token for storage and lookup.
///
/// Tokens are high-entropy random strings, so a plain SHA-256 is enough: there
/// is no low-entropy secret to slow a guess down for.
pub fn hash_token(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(token.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Resolve a token hash to its agent id and trust level.
///
/// Also touches the token and agent last-seen timestamps, at most once per
/// window. Until the store is filled in, every hash answers "no such token", so
/// no agent token authenticates yet.
pub async fn resolve_token(_db: &Database, _token_hash: &str) -> Result<Option<(String, Trust)>> {
    Ok(None)
}
