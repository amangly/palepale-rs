//! The only code that touches the network, files, terminal or clock; clippy.toml bans the raw
//! calls everywhere else, so a transport change, retry or cache is one edit here.

pub mod clipboard;
pub mod clock;
pub mod exec;
pub mod fetch;
pub mod fs;
pub mod http;
pub mod mcp;
pub mod mcp_auth;
mod mcp_sse;
pub mod proc;
pub mod search;
pub mod secret;
pub mod shell;
pub mod skill_cache;
pub mod snapshot;
pub mod term;
pub mod user_path;
