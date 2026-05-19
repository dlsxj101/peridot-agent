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
        let command_owned: String = params
            .get("command")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or(detected)
            .unwrap_or_else(|| "cargo build --workspace".to_string());
        run_verification_command(&command_owned, ctx, "verify build", "build")
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
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
        let command_owned: String = params
            .get("command")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or(detected)
            .unwrap_or_else(|| "cargo test --workspace".to_string());
        run_verification_command(&command_owned, ctx, "verify test", "test")
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
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
        let command_owned: String = params
            .get("command")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or(detected)
            .unwrap_or_else(|| "cargo clippy --workspace -- -D warnings".to_string());
        run_verification_command(&command_owned, ctx, "verify lint", "lint")
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
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

fn run_verification_command(
    command: &str,
    ctx: &ToolContext,
    label: &str,
    stage: &str,
) -> PeriResult<ToolResult> {
    let result = run_read_only_command(command, ctx, label)?;
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
