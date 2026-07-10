use async_trait::async_trait;
use peridot_common::{PeriResult, PermissionLevel, ToolGroup, ToolResult};
use peridot_project::ProjectScanner;
use serde_json::Value;

use crate::hooks::{HookRunner, HookVariables};
use crate::tools::command::run_read_only_command;
use crate::{Tool, ToolContext};

/// Resolves the default build / test / lint / format command for the
/// current project by scanning the workspace markers (Cargo.toml,
/// package.json, pyproject.toml, go.mod, …). Returns `None` when
/// the scanner can't infer a command of the requested kind; the
/// caller decides whether to fall back to the legacy cargo default
/// or surface an error. Centralised here so every verify_* tool
/// uses the same detection logic instead of each one hard-coding
/// `cargo build --workspace` / `cargo test --workspace`.
fn detect_command(ctx: &ToolContext, kind: VerifyKind) -> Option<String> {
    let profile = ProjectScanner::new().scan(&ctx.project_root).ok()?;
    match kind {
        VerifyKind::Build => profile.commands.build,
        VerifyKind::Test => profile.commands.test,
        VerifyKind::Lint => profile.commands.lint,
        VerifyKind::Format => profile.commands.format,
    }
}

#[derive(Clone, Copy)]
enum VerifyKind {
    Build,
    Test,
    Lint,
    #[allow(dead_code)]
    Format,
}

/// Built-in verify build tool.
#[derive(Clone, Debug)]
pub struct VerifyBuildTool;

#[async_trait]
impl Tool for VerifyBuildTool {
    fn name(&self) -> &str {
        "verify_build"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::Verify
    }

    fn description(&self) -> &str {
        "Run a build verification command"
    }

    fn parameters_schema(&self) -> Value {
        verify_command_schema(
            "auto-detected from project markers (Cargo.toml / package.json / pyproject.toml / …)",
        )
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let detected = detect_command(ctx, VerifyKind::Build);
        match resolve_command(&params, detected) {
            Some(command) => run_verification_command(&command, ctx, "verify build", "build").await,
            None => Ok(no_command_skip("build")),
        }
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
    }

    fn risk_class(&self) -> peridot_common::RiskClass {
        peridot_common::RiskClass::BuildOrTest
    }
}

/// Built-in verify test tool.
#[derive(Clone, Debug)]
pub struct VerifyTestTool;

#[async_trait]
impl Tool for VerifyTestTool {
    fn name(&self) -> &str {
        "verify_test"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::Verify
    }

    fn description(&self) -> &str {
        "Run a test verification command"
    }

    fn parameters_schema(&self) -> Value {
        verify_command_schema("auto-detected from project markers")
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let detected = detect_command(ctx, VerifyKind::Test);
        match resolve_command(&params, detected) {
            Some(command) => run_verification_command(&command, ctx, "verify test", "test").await,
            None => Ok(no_command_skip("test")),
        }
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
    }

    fn risk_class(&self) -> peridot_common::RiskClass {
        peridot_common::RiskClass::BuildOrTest
    }
}

/// Built-in verify lint tool.
#[derive(Clone, Debug)]
pub struct VerifyLintTool;

#[async_trait]
impl Tool for VerifyLintTool {
    fn name(&self) -> &str {
        "verify_lint"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::Verify
    }

    fn description(&self) -> &str {
        "Run a lint verification command"
    }

    fn parameters_schema(&self) -> Value {
        verify_command_schema("auto-detected from project markers")
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let detected = detect_command(ctx, VerifyKind::Lint);
        match resolve_command(&params, detected) {
            Some(command) => run_verification_command(&command, ctx, "verify lint", "lint").await,
            None => Ok(no_command_skip("lint")),
        }
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
    }

    fn risk_class(&self) -> peridot_common::RiskClass {
        peridot_common::RiskClass::BuildOrTest
    }
}

/// Resolves the command to run: an explicit `command` parameter wins,
/// then the project-detected command. `None` means neither is available
/// and the caller should surface a skip rather than guessing.
fn resolve_command(params: &Value, detected: Option<String>) -> Option<String> {
    params
        .get("command")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .or(detected)
}

/// Result returned when no verification command could be resolved for
/// this project. It is neither a pass nor a failure — the workspace
/// simply has no `kind` command configured. `success` is `true` so the
/// auto-fix circuit breaker never counts it, and the `skipped` marker in
/// `output` lets callers (auto-verify, preflight) tell it apart from a
/// real green run.
fn no_command_skip(kind: &str) -> ToolResult {
    ToolResult {
        success: true,
        summary: format!(
            "no {kind} command detected for this project; configure AGENTS.md `## commands` or pass `command`"
        ),
        output: serde_json::json!({
            "skipped": true,
            "reason": format!("no {kind} command detected"),
        }),
    }
}

fn verify_command_schema(default: &str) -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "description": format!("Verification command line (default \"{default}\")")
            }
        },
        "additionalProperties": false,
    })
}

async fn run_verification_command(
    command: &str,
    ctx: &ToolContext,
    label: &str,
    stage: &str,
) -> PeriResult<ToolResult> {
    let result = run_read_only_command(command, ctx, label).await?;
    let stdout = result.output["stdout"].as_str().unwrap_or_default();
    let stderr = result.output["stderr"].as_str().unwrap_or_default();
    let detail = [stdout, stderr]
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let hook_output = if detail.trim().is_empty() {
        result.summary.clone()
    } else {
        detail.replace(['\r', '\n'], " ")
    };
    run_verification_event_hook(ctx, stage, result.success, &hook_output)?;
    Ok(result)
}

fn run_verification_event_hook(
    ctx: &ToolContext,
    stage: &str,
    passed: bool,
    output: &str,
) -> PeriResult<()> {
    let mut variables = HookVariables::new();
    variables.insert(
        "project_root".to_string(),
        ctx.project_root.display().to_string(),
    );
    variables.insert(
        "workspace".to_string(),
        ctx.project_root.display().to_string(),
    );
    variables.insert("stage".to_string(), stage.to_string());
    variables.insert(
        "status".to_string(),
        if passed { "passed" } else { "failed" }.to_string(),
    );
    variables.insert("output".to_string(), output.to_string());
    let event = if passed {
        "verification_passed"
    } else {
        "verification_failed"
    };
    HookRunner::new(&ctx.project_root, ctx.hooks.clone()).run_event_hooks(event, &variables)?;
    Ok(())
}
