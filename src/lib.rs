//! Agent Hub: a local-first operations layer for AI agents and their humans.

pub mod app;
pub mod brain;
pub mod config;
pub mod error;
pub mod http;
pub mod mcp;
pub mod store;

pub use error::{Error, Result};
