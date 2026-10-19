//! `llminate-core` — client library for OpenAI-compatible LLM servers.
//!
//! The library holds everything that does not depend on the terminal: config
//! resolution, HTTP calls, SSE parsing and benchmark statistics. The `llminate`
//! binary is a thin CLI on top of it.

pub mod client;
pub mod config;
pub mod error;
pub mod sse;
pub mod stats;

pub use client::Client;
pub use config::{config_path, load, parse, resolve, ResolvedTarget, Target};
pub use error::{Error, Result};
pub use stats::{BenchRow, BenchStats};
