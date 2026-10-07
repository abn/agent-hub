//! Agent Hub: a local-first operations layer for AI agents and their humans.

pub mod app;
pub mod blob;
pub mod brain;
#[cfg(feature = "client")]
pub mod client;
pub mod config;
pub mod error;
pub mod http;
pub mod limits;
pub mod markdown;
pub mod mcp;
pub mod metrics;
pub mod net;
pub mod notify;
pub mod okf;
pub mod ops;
pub mod policy;
pub mod principal;
pub mod store;

pub use error::{Error, Result};
