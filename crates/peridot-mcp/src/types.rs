use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Tool exposed by an MCP server.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpTool {
    /// Tool name.
    pub name: String,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// JSON schema for tool arguments.
    #[serde(default, rename = "inputSchema")]
    pub input_schema: Value,
}

/// Result returned by `tools/call`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpCallResult {
    /// Tool content payload.
    #[serde(default)]
    pub content: Vec<Value>,
    /// Whether the tool reported an application-level error.
    #[serde(default, rename = "isError")]
    pub is_error: bool,
}
