use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Default JSON schema used when a tool omits `inputSchema`. Downstream
/// consumers expect an object schema, not `null`.
fn default_input_schema() -> Value {
    json!({ "type": "object" })
}

/// Tool exposed by an MCP server.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpTool {
    /// Tool name.
    pub name: String,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// JSON schema for tool arguments.
    #[serde(default = "default_input_schema", rename = "inputSchema")]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_without_input_schema_defaults_to_object() {
        let tool: McpTool = serde_json::from_value(json!({ "name": "demo" })).unwrap();

        assert_eq!(tool.input_schema, json!({ "type": "object" }));
    }

    #[test]
    fn tool_preserves_explicit_input_schema() {
        let tool: McpTool = serde_json::from_value(json!({
            "name": "demo",
            "inputSchema": { "type": "object", "properties": { "x": { "type": "string" } } }
        }))
        .unwrap();

        assert_eq!(tool.input_schema["properties"]["x"]["type"], "string");
    }
}
