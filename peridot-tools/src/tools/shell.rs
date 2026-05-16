use std::path::Path;
use std::process::Command;

use async_trait::async_trait;
use peridot_common::{PeriError, PeriResult, PermissionLevel, SandboxMode, ToolGroup, ToolResult};
use serde_json::Value;

use crate::path::required_str;
use crate::{Tool, ToolContext};

/// Built-in shell execution tool.
#[derive(Clone, Debug)]
pub struct ShellExecTool;

#[async_trait]
impl Tool for ShellExecTool {
    fn name(&self) -> &str {
        "shell_exec"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::Shell
    }

    fn description(&self) -> &str {
        "Execute a shell command from the project root after deterministic safety checks"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Shell command line executed from the project root"
                }
            },
            "required": ["command"],
            "additionalProperties": false,
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let command = required_str(&params, "command")?;
        reject_hard_blocked_command(command)?;
        enforce_shell_approval_policy(command, ctx)?;
        let output = shell_command(command, ctx)?
            .output()
            .map_err(|err| PeriError::Tool(format!("failed to run command: {err}")))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let summary = if output.status.success() {
            format!("command exited 0: {command}")
        } else {
            format!(
                "command exited {}: {command}",
                output.status.code().unwrap_or(-1)
            )
        };
        Ok(ToolResult::success(
            summary,
            serde_json::json!({
                "status": output.status.code(),
                "stdout": stdout,
                "stderr": stderr
            }),
        ))
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }

    fn can_run_concurrent(&self) -> bool {
        false
    }
}

pub(crate) fn reject_hard_blocked_command(command: &str) -> PeriResult<()> {
    let normalized = command.split_whitespace().collect::<Vec<_>>().join(" ");
    let hard_blocked = [
        "rm -rf /",
        "mkfs.",
        "dd if=/dev/zero",
        ":(){ :|:& };:",
        "chmod -R 777 /",
        "curl",
        "wget",
    ];

    if normalized.contains("curl") && normalized.contains("| sh") {
        return Err(PeriError::PermissionDenied(
            "piping remote curl output into a shell is blocked".to_string(),
        ));
    }
    if normalized.contains("wget") && normalized.contains("| bash") {
        return Err(PeriError::PermissionDenied(
            "piping remote wget output into a shell is blocked".to_string(),
        ));
    }
    if hard_blocked
        .iter()
        .take(5)
        .any(|pattern| normalized.contains(pattern))
    {
        return Err(PeriError::PermissionDenied(format!(
            "hard-blocked shell command pattern: {command}"
        )));
    }
    Ok(())
}

pub(crate) fn enforce_shell_approval_policy(command: &str, ctx: &ToolContext) -> PeriResult<()> {
    let normalized = normalize_shell_command(command);
    if ctx.security.ask_before_install && is_install_command(&normalized) {
        return Err(PeriError::PermissionDenied(
            "dependency installation requires explicit user approval".to_string(),
        ));
    }
    if ctx.security.ask_before_delete && is_destructive_shell_command(&normalized) {
        return Err(PeriError::PermissionDenied(
            "destructive shell command requires explicit user approval".to_string(),
        ));
    }
    Ok(())
}

fn normalize_shell_command(command: &str) -> String {
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_install_command(command: &str) -> bool {
    let padded = format!(" {command} ");
    [
        " cargo add ",
        " cargo install ",
        " npm install ",
        " npm i ",
        " npm ci ",
        " pnpm add ",
        " pnpm install ",
        " yarn add ",
        " yarn install ",
        " pip install ",
        " pip3 install ",
        " python -m pip install ",
        " python3 -m pip install ",
        " uv add ",
        " uv pip install ",
        " poetry add ",
        " apt install ",
        " apt-get install ",
        " dnf install ",
        " yum install ",
        " brew install ",
    ]
    .iter()
    .any(|pattern| padded.contains(pattern))
}

fn is_destructive_shell_command(command: &str) -> bool {
    let padded = format!(" {command} ");
    command.starts_with("rm ")
        || padded.contains(" && rm ")
        || padded.contains(" ; rm ")
        || padded.contains(" | xargs rm ")
        || padded.contains(" find ") && padded.contains(" -delete ")
        || padded.contains(" git clean ")
        || padded.contains(" git reset --hard ")
        || padded.contains(" git push --force ")
        || padded.contains(" git push -f ")
}

fn shell_command(command: &str, ctx: &ToolContext) -> PeriResult<Command> {
    match ctx.security.sandbox {
        SandboxMode::None => {
            let mut process = Command::new("sh");
            process
                .arg("-c")
                .arg(command)
                .current_dir(&ctx.project_root);
            Ok(process)
        }
        SandboxMode::Docker => {
            let mut process = Command::new("docker");
            process.args(docker_shell_args(
                &ctx.project_root,
                command,
                &ctx.security.docker_image,
                ctx.security.docker_network,
            ));
            Ok(process)
        }
        SandboxMode::Firejail => {
            let mut process = Command::new("firejail");
            process
                .args(firejail_shell_args(
                    &ctx.project_root,
                    command,
                    ctx.security.docker_network,
                ))
                .current_dir(&ctx.project_root);
            Ok(process)
        }
    }
}

pub(crate) fn docker_shell_args(
    project_root: &Path,
    command: &str,
    image: &str,
    network: bool,
) -> Vec<String> {
    let mut args = vec![
        "run".to_string(),
        "--rm".to_string(),
        "-v".to_string(),
        format!("{}:/workspace", project_root.display()),
        "-w".to_string(),
        "/workspace".to_string(),
    ];
    if !network {
        args.extend(["--network".to_string(), "none".to_string()]);
    }
    args.extend([
        image.to_string(),
        "sh".to_string(),
        "-lc".to_string(),
        command.to_string(),
    ]);
    args
}

pub(crate) fn firejail_shell_args(
    project_root: &Path,
    command: &str,
    network: bool,
) -> Vec<String> {
    let mut args = vec![
        "--quiet".to_string(),
        "--noprofile".to_string(),
        "--private-dev".to_string(),
        "--private-tmp".to_string(),
        format!("--whitelist={}", project_root.display()),
        format!("--read-write={}", project_root.display()),
    ];
    if !network {
        args.push("--net=none".to_string());
    }
    args.extend(["sh".to_string(), "-lc".to_string(), command.to_string()]);
    args
}
