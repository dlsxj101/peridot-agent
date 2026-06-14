use peridot_common::{PeriError, PeriResult, ToolResult};

use crate::ToolContext;
use crate::tools::shell::{shell_command, spawn_and_wait_interruptible};

pub(crate) async fn run_read_only_command(
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
    let prepared = shell_command(command, ctx)?;
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
    // Run on the blocking pool via `spawn_and_wait_interruptible`, matching
    // ShellExecTool. Two robustness fixes over the old inline `.output()`:
    //   1. A long verify_* (e.g. `cargo test`) no longer pins a tokio worker
    //      thread for its whole duration — it ran on the async runtime,
    //      starving event emission and cancellation polling.
    //   2. The command now honours the cancel token and the configured
    //      shell_command_timeout_seconds, so an interrupted run or a hung
    //      build is actually torn down instead of running to completion.
    // When no cancel token is attached and no timeout is set (tests, headless
    // smokes), `spawn_and_wait_interruptible` takes its fast path and behaves
    // exactly like the previous `.output()` call.
    let ctx_for_block = ctx.clone();
    let label_owned = label.to_string();
    let output = tokio::task::spawn_blocking(move || {
        spawn_and_wait_interruptible(prepared, &ctx_for_block, &label_owned)
    })
    .await
    .map_err(|err| PeriError::Tool(format!("failed to join {label} task: {err}")))??;
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
