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
//! file and hands out a per-file write lock, so concurrent writers to one
//! file serialise while writers to distinct files never block. A project
//! knowledge base is one more file behind the same wrapper and the same lock,
//! opened under [`KNOWLEDGE_FILE`] in place of a session id.

pub mod knowledge;
mod session;

pub use agentfs_sdk::ToolCall;
pub use session::{
    Brain, BrainStore, Entry, EntryKind, KNOWLEDGE_FILE, LastWrites, Stamp, VERSION_ABSENT,
    WriteFilter, WriteLogPage, WriteRecord, canonical_path, file_bytes, is_under, knowledge_dir,
    reconcile_sessions, version,
};
