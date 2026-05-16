use std::fs;
use std::path::Path;

use async_trait::async_trait;
use peridot_common::{PeriError, PeriResult, PermissionLevel, ToolGroup, ToolResult};
use serde_json::Value;

use crate::hooks::{HookRunner, HookVariables};
use crate::path::{ensure_within_project, required_str, workspace_path};
use crate::{Tool, ToolContext};

/// Built-in file read tool.
#[derive(Clone, Debug)]
pub struct FileReadTool;

#[async_trait]
impl Tool for FileReadTool {
    fn name(&self) -> &str {
        "file_read"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::File
    }

    fn description(&self) -> &str {
        "Read a workspace file"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Project-relative path of the file to read"
                }
            },
            "required": ["path"],
            "additionalProperties": false,
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let path = workspace_path(ctx, &params)?;
        let content = fs::read_to_string(&path)
            .map_err(|err| PeriError::Tool(format!("failed to read {}: {err}", path.display())))?;
        Ok(ToolResult::success(
            format!("read {}", path.display()),
            Value::String(content),
        ))
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
    }
}

/// Built-in file write tool.
#[derive(Clone, Debug)]
pub struct FileWriteTool;

#[async_trait]
impl Tool for FileWriteTool {
    fn name(&self) -> &str {
        "file_write"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::File
    }

    fn description(&self) -> &str {
        "Write a workspace file"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Project-relative path"},
                "content": {"type": "string", "description": "File contents to write"}
            },
            "required": ["path", "content"],
            "additionalProperties": false,
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let path = workspace_path(ctx, &params)?;
        let content = required_str(&params, "content")?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                PeriError::Tool(format!("failed to create {}: {err}", parent.display()))
            })?;
        }
        fs::write(&path, content)
            .map_err(|err| PeriError::Tool(format!("failed to write {}: {err}", path.display())))?;
        run_file_changed_hook(ctx, &path)?;
        Ok(ToolResult::success(
            format!("wrote {}", path.display()),
            serde_json::json!({ "path": path }),
        ))
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }

    fn can_run_concurrent(&self) -> bool {
        false
    }
}

/// Built-in precision file patch tool.
#[derive(Clone, Debug)]
pub struct FilePatchTool;

#[async_trait]
impl Tool for FilePatchTool {
    fn name(&self) -> &str {
        "file_patch"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::File
    }

    fn description(&self) -> &str {
        "Replace one exact text segment in a workspace file"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Project-relative path"},
                "old_text": {"type": "string", "description": "Exact substring to replace"},
                "new_text": {"type": "string", "description": "Replacement text"}
            },
            "required": ["path", "old_text", "new_text"],
            "additionalProperties": false,
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let path = workspace_path(ctx, &params)?;
        let old_text = required_str(&params, "old_text")?;
        let new_text = required_str(&params, "new_text")?;
        let content = fs::read_to_string(&path)
            .map_err(|err| PeriError::Tool(format!("failed to read {}: {err}", path.display())))?;
        if !content.contains(old_text) {
            return Err(PeriError::Tool(format!(
                "old_text not found in {}",
                path.display()
            )));
        }
        let patched = content.replacen(old_text, new_text, 1);
        fs::write(&path, patched)
            .map_err(|err| PeriError::Tool(format!("failed to write {}: {err}", path.display())))?;
        run_file_changed_hook(ctx, &path)?;
        Ok(ToolResult::success(
            format!("patched {}", path.display()),
            serde_json::json!({ "path": path }),
        ))
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }

    fn can_run_concurrent(&self) -> bool {
        false
    }
}

fn run_file_changed_hook(ctx: &ToolContext, path: &Path) -> PeriResult<()> {
    let mut variables = HookVariables::new();
    variables.insert(
        "project_root".to_string(),
        ctx.project_root.display().to_string(),
    );
    variables.insert(
        "workspace".to_string(),
        ctx.project_root.display().to_string(),
    );
    variables.insert("path".to_string(), hook_relative_path(ctx, path));
    variables.insert("absolute_path".to_string(), path.display().to_string());
    HookRunner::new(&ctx.project_root, ctx.hooks.clone())
        .run_event_hooks("file_changed", &variables)?;
    Ok(())
}

fn hook_relative_path(ctx: &ToolContext, path: &Path) -> String {
    path.strip_prefix(&ctx.project_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Built-in substring file search tool.
#[derive(Clone, Debug)]
pub struct FileSearchTool;

#[async_trait]
impl Tool for FileSearchTool {
    fn name(&self) -> &str {
        "file_search"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::File
    }

    fn description(&self) -> &str {
        "Search workspace files for a substring pattern"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "Substring to search for"},
                "path": {
                    "type": "string",
                    "description": "Optional project-relative directory to scope the search"
                }
            },
            "required": ["pattern"],
            "additionalProperties": false,
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let pattern = required_str(&params, "pattern")?;
        let path = params.get("path").and_then(Value::as_str).map_or_else(
            || ctx.project_root.clone(),
            |path| ctx.project_root.join(path),
        );
        let path = ensure_within_project(&ctx.project_root, &path)?;
        let mut matches = Vec::new();
        search_path(&path, pattern, &mut matches)?;
        Ok(ToolResult::success(
            format!("found {} matches for {pattern}", matches.len()),
            serde_json::json!(matches),
        ))
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
    }
}

fn search_path(path: &Path, pattern: &str, matches: &mut Vec<Value>) -> PeriResult<()> {
    if path.is_dir() {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if matches!(name, ".git" | "target" | "node_modules") {
            return Ok(());
        }
        for entry in fs::read_dir(path)
            .map_err(|err| PeriError::Tool(format!("failed to search {}: {err}", path.display())))?
        {
            let entry = entry.map_err(|err| PeriError::Tool(err.to_string()))?;
            search_path(&entry.path(), pattern, matches)?;
        }
        return Ok(());
    }

    if !path.is_file() {
        return Ok(());
    }

    let Ok(content) = fs::read_to_string(path) else {
        return Ok(());
    };
    for (line_idx, line) in content.lines().enumerate() {
        if line.contains(pattern) {
            matches.push(serde_json::json!({
                "path": path,
                "line": line_idx + 1,
                "text": line
            }));
        }
    }
    Ok(())
}

/// Built-in file list tool.
#[derive(Clone, Debug)]
pub struct FileListTool;

#[async_trait]
impl Tool for FileListTool {
    fn name(&self) -> &str {
        "file_list"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::File
    }

    fn description(&self) -> &str {
        "List a workspace directory"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Project-relative directory path (use \".\" for project root)"
                }
            },
            "required": ["path"],
            "additionalProperties": false,
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let path = workspace_path(ctx, &params)?;
        let mut entries = Vec::new();
        for entry in fs::read_dir(&path)
            .map_err(|err| PeriError::Tool(format!("failed to list {}: {err}", path.display())))?
        {
            let entry = entry.map_err(|err| PeriError::Tool(err.to_string()))?;
            entries.push(entry.file_name().to_string_lossy().to_string());
        }
        entries.sort();
        Ok(ToolResult::success(
            format!("listed {}", path.display()),
            serde_json::json!(entries),
        ))
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
    }
}
