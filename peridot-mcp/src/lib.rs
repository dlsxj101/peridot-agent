//! MCP client boundary and server configuration types.

pub use peridot_common::{McpServerConfig, McpTransport};

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
