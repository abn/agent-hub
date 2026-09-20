//! The harness the integration tests share, one module per concern.
//!
//! Every file under `tests/` is a crate of its own and uses a part of this, so
//! what one of them leaves unused is allowed here rather than copied there.
#![allow(dead_code)]

pub mod http;
pub mod process;
pub mod seed;
pub mod state;
pub mod stdio;
pub mod store;
pub mod temp;
pub mod wire;
