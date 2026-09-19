//! Line-oriented Open Knowledge Format frontmatter reader, patcher, and lint.
//!
//! Frontmatter is read and patched without a YAML parser to preserve exact byte
//! formatting, comments, unknown keys, and key order.

pub mod frontmatter;

pub use frontmatter::{
    Change, Frontmatter, FrontmatterError, PatchValue, PromoteParams, Record, Scalar,
    SourceCitation, Verification, parse_frontmatter, patch_frontmatter, promote_frontmatter,
    review_frontmatter,
};
