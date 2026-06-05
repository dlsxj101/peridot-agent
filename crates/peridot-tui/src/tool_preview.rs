//! Tool parameter / output preview formatting for the transcript.
//!
//! Turns a tool call's parameters and result `output` JSON into the short,
//! indented preview lines the transcript renders under a tool entry (command,
//! path, a few stdout/stderr or match lines, …). Pure functions over
//! `serde_json::Value`; split out of `state.rs` so the per-tool preview shapes
//! live in one place. [`TuiState`] calls [`tool_parameter_preview`] on
//! `ToolStart` and [`tool_output_preview`] on `ToolEnd`.
//!
//! [`TuiState`]: super::TuiState

/// Preview lines for a finished tool's `output`, dispatched by tool name.
/// Returns an empty vec for tools without a dedicated preview.
pub(crate) fn tool_output_preview(tool_name: &str, output: &serde_json::Value) -> Vec<String> {
    match tool_name {
        "shell_exec" | "shell_readonly" => shell_output_preview(output),
        "ripgrep_search" => ripgrep_output_preview(output),
        "file_write" | "file_patch" | "file_read" => file_output_preview(tool_name, output),
        _ => Vec::new(),
    }
}

/// Preview lines for a tool call's `parameters`, dispatched by tool name.
/// Falls back to a bare `path:` line for tools without a dedicated preview.
pub(crate) fn tool_parameter_preview(
    tool_name: &str,
    parameters: &serde_json::Value,
) -> Vec<String> {
    match tool_name {
        "shell_exec" | "shell_readonly" => parameters
            .get("command")
            .and_then(serde_json::Value::as_str)
            .map(|command| vec![format!("  command: {command}")])
            .unwrap_or_default(),
        "ripgrep_search" => ripgrep_parameter_preview(parameters),
        "file_patch" => file_patch_parameter_preview(parameters),
        "file_write" => file_write_parameter_preview(parameters),
        _ => parameters
            .get("path")
            .and_then(serde_json::Value::as_str)
            .map(|path| vec![format!("  path: {path}")])
            .unwrap_or_default(),
    }
}

fn file_patch_parameter_preview(parameters: &serde_json::Value) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(path) = parameters.get("path").and_then(serde_json::Value::as_str) {
        lines.push(format!("  path: {path}"));
    }
    // The diff bodies themselves arrive as a dedicated `FileDiff` event
    // after the tool finishes (see `record_file_diff`), so the ToolStart
    // preview only carries the path. Anything else here would
    // double-render in the chat alongside the post-execution diff.
    lines
}

fn file_write_parameter_preview(parameters: &serde_json::Value) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(path) = parameters.get("path").and_then(serde_json::Value::as_str) {
        lines.push(format!("  path: {path}"));
    }
    if let Some(content) = parameters
        .get("content")
        .and_then(serde_json::Value::as_str)
    {
        lines.push("  content:".to_string());
        lines.extend(
            preview_lines(content, 4)
                .into_iter()
                .map(|line| format!("    {line}")),
        );
    }
    lines
}

fn ripgrep_parameter_preview(parameters: &serde_json::Value) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(query) = parameters.get("query").and_then(serde_json::Value::as_str) {
        lines.push(format!("  query: {query}"));
    }
    if let Some(path) = parameters.get("path").and_then(serde_json::Value::as_str) {
        lines.push(format!("  path: {path}"));
    }
    lines
}

fn shell_output_preview(output: &serde_json::Value) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(status) = output.get("status") {
        lines.push(format!("  status: {status}"));
    }
    if let Some(mutated) = output
        .get("workspace_mutated")
        .and_then(serde_json::Value::as_bool)
    {
        lines.push(format!("  mutated: {mutated}"));
    }
    for key in ["stdout", "stderr"] {
        let Some(text) = output.get(key).and_then(serde_json::Value::as_str) else {
            continue;
        };
        let preview = preview_lines(text, 3);
        if !preview.is_empty() {
            lines.push(format!("  {key}:"));
            lines.extend(preview.into_iter().map(|line| format!("    {line}")));
        }
    }
    lines
}

fn ripgrep_output_preview(output: &serde_json::Value) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(backend) = output.get("backend").and_then(serde_json::Value::as_str) {
        lines.push(format!("  backend: {backend}"));
    }
    if let Some(matches) = output.get("matches").and_then(serde_json::Value::as_array) {
        lines.push(format!("  matches: {}", matches.len()));
        for item in matches.iter().take(3) {
            let path = item
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("<unknown>");
            let line = item
                .get("line")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default();
            let text = item
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .trim();
            lines.push(format!("    {path}:{line}: {text}"));
        }
    }
    lines
}

fn file_output_preview(tool_name: &str, output: &serde_json::Value) -> Vec<String> {
    if tool_name == "file_read" {
        let Some(content) = output.as_str() else {
            return Vec::new();
        };
        let preview = preview_lines(content, 4);
        if preview.is_empty() {
            return Vec::new();
        }
        let mut lines = vec!["  preview:".to_string()];
        lines.extend(preview.into_iter().map(|line| format!("    {line}")));
        return lines;
    }
    output
        .get("path")
        .map(|path| vec![format!("  path: {path}")])
        .unwrap_or_default()
}

fn preview_lines(text: &str, limit: usize) -> Vec<String> {
    let mut lines = text
        .lines()
        .take(limit)
        .map(|line| {
            if line.chars().count() > 120 {
                format!("{}...", line.chars().take(117).collect::<String>())
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>();
    if text.lines().count() > limit {
        lines.push("...".to_string());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_output_preview_caps_lines_and_marks_truncation() {
        let output = serde_json::json!({
            "status": 0,
            "stdout": "a\nb\nc\nd\ne",
            "stderr": "",
        });
        let lines = tool_output_preview("shell_exec", &output);
        assert!(lines.iter().any(|l| l == "  status: 0"));
        assert!(lines.iter().any(|l| l == "  stdout:"));
        // 3-line cap on stdout preview, plus a truncation marker.
        assert!(lines.iter().any(|l| l.trim() == "..."));
        // stderr is empty → no stderr section.
        assert!(!lines.iter().any(|l| l == "  stderr:"));
    }

    #[test]
    fn file_write_parameter_preview_shows_path_and_indented_content() {
        let params = serde_json::json!({"path": "src/lib.rs", "content": "one\ntwo"});
        let lines = tool_parameter_preview("file_write", &params);
        assert_eq!(lines[0], "  path: src/lib.rs");
        assert_eq!(lines[1], "  content:");
        assert_eq!(lines[2], "    one");
        assert_eq!(lines[3], "    two");
    }

    #[test]
    fn unknown_tool_falls_back_to_path_line() {
        let params = serde_json::json!({"path": "notes.md"});
        assert_eq!(
            tool_parameter_preview("file_read", &params),
            vec!["  path: notes.md".to_string()]
        );
        assert!(tool_output_preview("unknown_tool", &serde_json::json!({})).is_empty());
    }
}
