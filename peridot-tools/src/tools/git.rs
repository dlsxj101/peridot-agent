use std::process::Command;

use async_trait::async_trait;
use peridot_common::{PeriError, PeriResult, PermissionLevel, ToolGroup, ToolResult};
use serde_json::Value;

use crate::tools::command::run_read_only_command;
use crate::{Tool, ToolContext};

/// Runs `git` with explicit argv (no shell) so commit messages and branch
/// names containing spaces / quotes don't need escaping. Returns the
/// standard `ToolResult` shape with status / stdout / stderr.
fn run_git(args: &[&str], ctx: &ToolContext, label: &str) -> PeriResult<ToolResult> {
    let output = Command::new("git")
        .args(args)
        .current_dir(&ctx.project_root)
        .output()
        .map_err(|err| PeriError::Tool(format!("failed to run {label}: {err}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let success = output.status.success();
    let summary = if success {
        format!("{label} ok")
    } else {
        format!(
            "{label} exited {}",
            output.status.code().unwrap_or(-1)
        )
    };
    Ok(ToolResult {
        success,
        summary,
        output: serde_json::json!({
            "status": output.status.code(),
            "success": success,
            "stdout": stdout,
            "stderr": stderr,
        }),
    })
}

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

    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}, "additionalProperties": false})
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

    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}, "additionalProperties": false})
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

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 200,
                    "description": "Number of commits to show (default 10)"
                }
            },
            "additionalProperties": false,
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(10);
        run_read_only_command(&format!("git log --oneline -{limit}"), ctx, "git log")
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
    }
}

/// Built-in `git commit -m "..."` tool. Surfaces as `git_commit` so the
/// project-local `pre:git_commit` hook (typically `cargo fmt + clippy +
/// test`) fires automatically before the commit lands. The model passes
/// the message verbatim through `params.message`; the optional `add_all`
/// flag triggers `git add -A` first so the commit captures unstaged
/// changes — mirroring the manual workflow most operators expect.
#[derive(Clone, Debug)]
pub struct GitCommitTool;

#[async_trait]
impl Tool for GitCommitTool {
    fn name(&self) -> &str {
        "git_commit"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::Git
    }

    fn description(&self) -> &str {
        "Create a git commit with the given message. Use add_all=true to stage every unstaged tracked file before committing."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "Commit message. Use a Conventional Commits prefix when the project uses that style."
                },
                "add_all": {
                    "type": "boolean",
                    "description": "When true, run `git add -A` before committing to stage all unstaged changes (default: false)."
                }
            },
            "required": ["message"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let message = params
            .get("message")
            .and_then(Value::as_str)
            .ok_or_else(|| PeriError::Tool("git_commit requires `message`".to_string()))?;
        let add_all = params
            .get("add_all")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if add_all {
            let stage = run_git(&["add", "-A"], ctx, "git add -A")?;
            if !stage.success {
                return Ok(stage);
            }
        }
        run_git(&["commit", "-m", message], ctx, "git commit")
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }

    fn can_run_concurrent(&self) -> bool {
        false
    }
}

/// Built-in `git branch` / `git checkout -b` tool. `checkout=true` (the
/// default) creates AND switches to the new branch; `checkout=false`
/// creates the branch without leaving the current HEAD. Refuses to act
/// when `name` is empty so a misclicked tool call doesn't quietly
/// no-op.
#[derive(Clone, Debug)]
pub struct GitBranchTool;

#[async_trait]
impl Tool for GitBranchTool {
    fn name(&self) -> &str {
        "git_branch"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::Git
    }

    fn description(&self) -> &str {
        "Create a new git branch. Defaults to checking it out as well; set checkout=false to create without switching."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Branch name (no spaces, no leading dash). Conventional projects often use a `feat/...` or `fix/...` prefix."
                },
                "checkout": {
                    "type": "boolean",
                    "description": "When true (default), switch to the new branch after creating it."
                }
            },
            "required": ["name"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| PeriError::Tool("git_branch requires `name`".to_string()))?;
        if name.is_empty() {
            return Err(PeriError::Tool("git_branch: name must not be empty".into()));
        }
        let checkout = params
            .get("checkout")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        if checkout {
            run_git(&["checkout", "-b", name], ctx, "git checkout -b")
        } else {
            run_git(&["branch", name], ctx, "git branch")
        }
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }

    fn can_run_concurrent(&self) -> bool {
        false
    }
}

/// Built-in `git push` tool. Defaults to pushing the current branch to
/// `origin`; `remote` / `branch` let the caller target a specific
/// remote/ref, `set_upstream=true` adds `-u` so the local branch tracks
/// the remote one (needed on first push of a new branch), and `force` is
/// guarded by the `Destructive` permission level so it surfaces an
/// approval prompt under safe/auto modes.
#[derive(Clone, Debug)]
pub struct GitPushTool;

#[async_trait]
impl Tool for GitPushTool {
    fn name(&self) -> &str {
        "git_push"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::Git
    }

    fn description(&self) -> &str {
        "Push the current branch (or an explicit remote/branch) to the configured remote. Set set_upstream=true on first push of a new branch."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "remote": {"type": "string", "description": "Remote name (default: origin)."},
                "branch": {"type": "string", "description": "Branch name (default: current branch)."},
                "set_upstream": {
                    "type": "boolean",
                    "description": "Pass -u so the local branch tracks the remote one. Required on the first push of a new branch."
                },
                "force": {
                    "type": "boolean",
                    "description": "When true, push with --force-with-lease (safer than --force). Destructive — requires explicit user approval."
                }
            },
            "additionalProperties": false
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let mut args: Vec<String> = vec!["push".to_string()];
        if params
            .get("set_upstream")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            args.push("-u".to_string());
        }
        if params.get("force").and_then(Value::as_bool).unwrap_or(false) {
            // Use --force-with-lease over --force so we never overwrite a
            // remote update we haven't seen — peridot is a coding agent,
            // not a release manager.
            args.push("--force-with-lease".to_string());
        }
        if let Some(remote) = params.get("remote").and_then(Value::as_str) {
            args.push(remote.to_string());
            if let Some(branch) = params.get("branch").and_then(Value::as_str) {
                args.push(branch.to_string());
            }
        }
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        run_git(&argv, ctx, "git push")
    }

    fn permission_level(&self) -> PermissionLevel {
        // Push is destructive — overwriting a remote ref is hard to undo.
        // Force-with-lease is safer than --force, but every push affects
        // shared state, so we still gate it behind the destructive bucket
        // so safe/auto modes prompt the user.
        PermissionLevel::Destructive
    }

    fn can_run_concurrent(&self) -> bool {
        false
    }
}
