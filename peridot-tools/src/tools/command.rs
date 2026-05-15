use std::process::Command;

use peridot_common::{PeriError, PeriResult, ToolResult};

use crate::ToolContext;

pub(crate) fn run_read_only_command(
    command: &str,
    ctx: &ToolContext,
    label: &str,
) -> PeriResult<ToolResult> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(&ctx.project_root)
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
