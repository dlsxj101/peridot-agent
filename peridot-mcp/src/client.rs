use std::time::Duration;

use peridot_common::{McpServerConfig, McpTransport, PeriError, PeriResult};
use serde_json::{Value, json};

use crate::http::http_request;
use crate::stdio::stdio_request;
use crate::types::{McpCallResult, McpTool};

/// MCP client.
#[derive(Clone, Debug)]
pub struct McpClient {
    config: McpServerConfig,
    timeout: Duration,
}

impl McpClient {
    /// Creates an MCP client.
    pub fn new(config: McpServerConfig) -> Self {
        Self {
            config,
            timeout: Duration::from_secs(30),
        }
    }

    /// Creates an MCP client with an explicit timeout.
    pub fn with_timeout(config: McpServerConfig, timeout: Duration) -> Self {
        Self { config, timeout }
    }

    /// Returns server config.
    pub fn config(&self) -> &McpServerConfig {
        &self.config
    }

    /// Initializes the server and lists exposed tools.
    pub async fn list_tools(&self) -> PeriResult<Vec<McpTool>> {
        let result = self.request("tools/list", json!({}), 2).await?;
        let parsed = result.get("tools").cloned().unwrap_or_else(|| json!([]));
        serde_json::from_value(parsed)
            .map_err(|err| PeriError::Parse(format!("invalid MCP tools/list response: {err}")))
    }

    /// Calls one MCP tool.
    pub async fn call_tool(&self, name: &str, arguments: Value) -> PeriResult<McpCallResult> {
        let result = self
            .request(
                "tools/call",
                json!({
                    "name": name,
                    "arguments": arguments
                }),
                2,
            )
            .await?;
        serde_json::from_value(result)
            .map_err(|err| PeriError::Parse(format!("invalid MCP tools/call response: {err}")))
    }

    async fn request(&self, method: &str, params: Value, id: u64) -> PeriResult<Value> {
        match self.config.transport {
            McpTransport::Stdio => {
                stdio_request(&self.config, self.timeout, method, params, id).await
            }
            McpTransport::Http => {
                http_request(&self.config, self.timeout, method, params, id).await
            }
        }
    }
}
