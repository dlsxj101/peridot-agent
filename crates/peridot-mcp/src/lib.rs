//! MCP client boundary and server configuration types.

mod client;
mod http;
mod protocol;
mod stdio;
#[cfg(test)]
mod tests;
mod types;

pub use client::McpClient;
pub use peridot_common::{McpServerConfig, McpTransport};
pub use protocol::MCP_PROTOCOL_VERSION;
pub use types::{McpCallResult, McpTool};
