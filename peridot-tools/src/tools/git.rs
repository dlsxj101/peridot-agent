use async_trait::async_trait;
use peridot_common::{PeriResult, PermissionLevel, ToolGroup, ToolResult};
use serde_json::Value;

use crate::tools::command::run_read_only_command;
use crate::{Tool, ToolContext};

/// Built-in git status tool.
#[derive(Clone, Debug)]
pub struct GitStatusTool;

#[async_trait]
impl Tool for GitStatusTool {
    fn name(&self) -> &str {
        "git_status"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::Git
    }

    fn description(&self) -> &str {
        "Return git status --short --branch"
    }

    async fn execute(&self, _params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        run_read_only_command("git status --short --branch", ctx, "git status")
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
    }
}

/// Built-in git diff tool.
#[derive(Clone, Debug)]
pub struct GitDiffTool;

#[async_trait]
impl Tool for GitDiffTool {
    fn name(&self) -> &str {
        "git_diff"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::Git
    }

    fn description(&self) -> &str {
        "Return git diff"
    }

    async fn execute(&self, _params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        run_read_only_command("git diff", ctx, "git diff")
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
    }
}

/// Built-in git log tool.
#[derive(Clone, Debug)]
pub struct GitLogTool;

#[async_trait]
impl Tool for GitLogTool {
    fn name(&self) -> &str {
        "git_log"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::Git
    }

    fn description(&self) -> &str {
        "Return compact git log output"
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(10);
        run_read_only_command(&format!("git log --oneline -{limit}"), ctx, "git log")
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
    }
}
