//! MCP for PULSE: the registry of servers agents may call (`pulse mcp`), a
//! minimal Model Context Protocol client over stdio, Streamable HTTP and the
//! older HTTP+SSE transport, OAuth 2.1 sign-in for http and sse servers
//! (`pulse mcp login`), and the shared skills (`pulse skill`).

pub mod client;
pub mod commands;
pub mod oauth;

pub use client::*;
pub use commands::{
    Checked, RESOLVER_TIMEOUT, connect_and_record, connect_error, register, resolver,
};
pub use oauth::LoginNeeded;
