use peridot_common::{PeriError, PeriResult, ToolResult};

use crate::ToolContext;
use crate::tools::shell::shell_command;

pub(crate) fn run_read_only_command(
    command: &str,
    ctx: &ToolContext,
    label: &str,
) -> PeriResult<ToolResult> {
    // Route through `shell_command` so the configured SandboxMode
    // (Docker / Firejail) wraps the invocation. The git_* and verify_*
    // tools build on this helper and execute project code (build.rs, test
    // runners, lint scripts); running them on the bare host while the
    // operator selected a sandbox was a sandbox-escape. SandboxMode::None
    // resolves to the same `sh -c <command>` in project_root as before, so
    // the default path is unchanged.
    let mut prepared = shell_command(command, ctx)?;
    // Honour shell_dry_run the same way ShellExecTool does: surface the
    // resolved (sandbox-wrapped) invocation without spawning, so safety
    // drills can confirm verify_*/git_* are sandboxed too.
    if ctx.security.shell_dry_run {
        return Ok(ToolResult::success(
            format!("dry-run: {label}"),
            serde_json::json!({
                "dry_run": true,
                "would_execute": crate::tools::shell::describe_shell_command(&prepared),
                "command": command,
            }),
        ));
    }
    let output = prepared
        .output()
        .map_err(|err| PeriError::Tool(format!("failed to run {label}: {err}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let success = output.status.success();
    Ok(ToolResult {
        success,
        summary: format!("{label} exited {}", output.status.code().unwrap_or(-1)),
        output: serde_json::json!({
            "status": output.status.code(),
            "success": success,
            "stdout": stdout,
            "stderr": stderr
        }),
    })
}
