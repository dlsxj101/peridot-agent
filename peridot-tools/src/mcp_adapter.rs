use async_trait::async_trait;
use peridot_common::{McpServerConfig, PeriResult, PermissionLevel, ToolGroup, ToolResult};
use peridot_mcp::{McpClient, McpTool};
use serde_json::Value;

use crate::{Tool, ToolContext, ToolRegistry};

pub fn register_mcp_tools(
    registry: &mut ToolRegistry,
    server: McpServerConfig,
    tools: impl IntoIterator<Item = McpTool>,
) -> PeriResult<()> {
    for tool in tools {
        registry.register(McpToolAdapter::new(server.clone(), tool))?;
    }
    Ok(())
}

/// Converts an MCP server tool into Peridot's local tool trait.
#[derive(Clone, Debug)]
pub struct McpToolAdapter {
    server: McpServerConfig,
    tool: McpTool,
    name: String,
}

impl McpToolAdapter {
    /// Creates an MCP tool adapter.
    pub fn new(server: McpServerConfig, tool: McpTool) -> Self {
        let name = format!(
            "mcp_{}_{}",
            sanitize_tool_name(&server.name),
            sanitize_tool_name(&tool.name)
        );
        Self { server, tool, name }
    }
}

#[async_trait]
impl Tool for McpToolAdapter {
    fn name(&self) -> &str {
        &self.name
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::Mcp
    }

    fn description(&self) -> &str {
        self.tool
            .description
            .as_deref()
            .unwrap_or("External MCP tool")
    }

    fn parameters_schema(&self) -> Value {
        if self.tool.input_schema.is_null() {
            serde_json::json!({"type": "object", "additionalProperties": true})
        } else {
            self.tool.input_schema.clone()
        }
    }

    async fn execute(&self, params: Value, _ctx: &ToolContext) -> PeriResult<ToolResult> {
        let result = McpClient::new(self.server.clone())
            .call_tool(&self.tool.name, params)
            .await?;
        let success = !result.is_error;
        let summary = if success {
            format!("MCP tool {} completed", self.tool.name)
        } else {
            format!("MCP tool {} returned an error", self.tool.name)
        };
        let output = serde_json::json!({
            "server": self.server.name,
            "tool": self.tool.name,
            "content": result.content,
            "is_error": result.is_error
        });
        if success {
            Ok(ToolResult::success(summary, output))
        } else {
            Ok(ToolResult::failure(summary))
        }
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::System
    }

    fn can_run_concurrent(&self) -> bool {
        false
    }
}

fn sanitize_tool_name(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    sanitized.trim_matches('_').to_string()
}
