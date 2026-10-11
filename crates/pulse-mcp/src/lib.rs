//! MCP for PULSE: the registry of servers agents may call (`pulse mcp`), a
//! minimal Model Context Protocol client over stdio, Streamable HTTP and the
//! older HTTP+SSE transport, and the shared skills (`pulse skill`).

pub mod client;
pub mod commands;

pub use client::*;
pub use commands::{Checked, RESOLVER_TIMEOUT, connect_and_record, register, resolver};
