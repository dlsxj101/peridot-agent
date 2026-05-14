//! MCP client boundary and server configuration types.

use serde::{Deserialize, Serialize};

/// MCP transport type.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    /// Standard input/output transport.
    Stdio,
    /// HTTP/SSE transport.
    Http,
}

/// MCP server configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Server name.
    pub name: String,
    /// Transport kind.
    pub transport: McpTransport,
}

/// MCP client skeleton.
#[derive(Clone, Debug)]
pub struct McpClient {
    config: McpServerConfig,
}

impl McpClient {
    /// Creates an MCP client skeleton.
    pub fn new(config: McpServerConfig) -> Self {
        Self { config }
    }

    /// Returns server config.
    pub fn config(&self) -> &McpServerConfig {
        &self.config
    }
}
