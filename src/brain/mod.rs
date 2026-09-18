//! The per-session AgentFS brain wrapper.
//!
//! One AgentFS file per session, wrapped so the hub is the single writer. The
//! hub never lets an agent reach the file directly; every read and write goes
//! through this API, which is what keeps the file consistent.
//!
//! Paths are namespaced so key-value entries and filesystem paths cannot
//! collide:
//!
//! - `/kv/<key>` addresses the AgentFS key-value store.
//! - `/fs/<path>` addresses the AgentFS POSIX-like filesystem.
//!
//! [`BrainStore`] is the process-wide factory: it maps a session id to its
//! file and hands out a per-session write lock, so concurrent writers to one
//! session serialise while writers to distinct sessions never block.

mod session;

pub use session::{Brain, BrainStore, Entry, EntryKind, VERSION_ABSENT, canonical_path, version};
