use async_trait::async_trait;
use peridot_common::{McpServerConfig, PeriResult, PermissionLevel, ToolGroup, ToolResult};
use peridot_mcp::{McpClient, McpTool};
use serde_json::Value;

use crate::audit::{AuditEvent, append_audit_event};
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
    permission_level: PermissionLevel,
}

impl McpToolAdapter {
    /// Creates an MCP tool adapter, resolving the permission level
    /// from the per-tool override map first, then the server-wide
    /// `default_permission`, then defaulting to `System` for legacy
    /// configs that predate the override fields.
    pub fn new(server: McpServerConfig, tool: McpTool) -> Self {
        let name = format!(
            "mcp_{}_{}",
            sanitize_tool_name(&server.name),
            sanitize_tool_name(&tool.name)
        );
        let permission_level = resolve_mcp_permission_level(&server, &tool.name);
        Self {
            server,
            tool,
            name,
            permission_level,
        }
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

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let invoked_params = params.clone();
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
        // Best-effort audit. MCP tool calls used to bypass the audit
        // log entirely; logging here gives parity with built-in tools
        // (every shell/file mutation already lands in audit.jsonl).
        let _ = append_audit_event(
            &ctx.project_root,
            &AuditEvent::tool_call(
                &self.name,
                success,
                &summary,
                serde_json::json!({
                    "server": self.server.name,
                    "raw_tool": self.tool.name,
                    "params": invoked_params,
                    "permission_level": format!("{:?}", self.permission_level).to_lowercase(),
                }),
            ),
        );
        if success {
            Ok(ToolResult::success(summary, output))
        } else {
            Ok(ToolResult::failure(summary))
        }
    }

    fn permission_level(&self) -> PermissionLevel {
        self.permission_level
    }

    fn can_run_concurrent(&self) -> bool {
        false
    }
}

fn resolve_mcp_permission_level(server: &McpServerConfig, tool_name: &str) -> PermissionLevel {
    if let Some(override_label) = server.tool_permission_overrides.get(tool_name) {
        return parse_permission_label(override_label);
    }
    parse_permission_label(&server.default_permission)
}

fn parse_permission_label(label: &str) -> PermissionLevel {
    match label.trim().to_ascii_lowercase().as_str() {
        "read" | "read_only" | "readonly" => PermissionLevel::Read,
        "write" => PermissionLevel::Write,
        "destructive" => PermissionLevel::Destructive,
        // Unknown / "system" / empty all fall through to the safest
        // default: full approval gating.
        _ => PermissionLevel::System,
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
