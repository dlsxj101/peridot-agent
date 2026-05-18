use std::fs;
use std::path::Path;

use async_trait::async_trait;
use peridot_common::{PeriError, PeriResult, PermissionLevel, ToolGroup, ToolResult};
use serde_json::Value;

use crate::hooks::{HookRunner, HookVariables};
use crate::path::{ensure_within_project, required_str, workspace_path};
use crate::{Tool, ToolContext};

const MAX_SYMBOL_FILE_BYTES: u64 = 1_000_000;

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

/// Built-in file outline tool.
#[derive(Clone, Debug)]
pub struct FileOutlineTool;

#[async_trait]
impl Tool for FileOutlineTool {
    fn name(&self) -> &str {
        "file_outline"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::File
    }

    fn description(&self) -> &str {
        "List top-level symbols in one source file"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Project-relative source file path"},
                "max_results": {"type": "integer", "minimum": 1, "maximum": 500}
            },
            "required": ["path"],
            "additionalProperties": false,
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let path = workspace_path(ctx, &params)?;
        let max_results = max_results(&params, 200);
        let symbols = outline_file(&ctx.project_root, &path, max_results)?;
        Ok(ToolResult::success(
            format!("outlined {} symbols in {}", symbols.len(), path.display()),
            serde_json::json!(symbols),
        ))
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
    }
}

/// Built-in workspace symbol listing tool.
#[derive(Clone, Debug)]
pub struct WorkspaceSymbolsTool;

#[async_trait]
impl Tool for WorkspaceSymbolsTool {
    fn name(&self) -> &str {
        "workspace_symbols"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::File
    }

    fn description(&self) -> &str {
        "List symbols across source files without reading full file contents"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Optional project-relative directory or file"},
                "max_results": {"type": "integer", "minimum": 1, "maximum": 1000}
            },
            "additionalProperties": false,
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let path = scoped_path(ctx, &params)?;
        let max_results = max_results(&params, 300);
        let mut symbols = Vec::new();
        collect_symbols(&ctx.project_root, &path, max_results, &mut symbols)?;
        Ok(ToolResult::success(
            format!("found {} workspace symbols", symbols.len()),
            serde_json::json!(symbols),
        ))
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
    }
}

/// Built-in workspace symbol search tool.
#[derive(Clone, Debug)]
pub struct SymbolSearchTool;

#[async_trait]
impl Tool for SymbolSearchTool {
    fn name(&self) -> &str {
        "symbol_search"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::File
    }

    fn description(&self) -> &str {
        "Search source symbols by name across the workspace"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Case-insensitive symbol name substring"},
                "path": {"type": "string", "description": "Optional project-relative directory or file"},
                "max_results": {"type": "integer", "minimum": 1, "maximum": 500}
            },
            "required": ["query"],
            "additionalProperties": false,
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let query = required_str(&params, "query")?.to_ascii_lowercase();
        let path = scoped_path(ctx, &params)?;
        let max_results = max_results(&params, 100);
        let mut symbols = Vec::new();
        collect_symbols(
            &ctx.project_root,
            &path,
            max_results.saturating_mul(4),
            &mut symbols,
        )?;
        symbols.retain(|symbol| symbol.name.to_ascii_lowercase().contains(&query));
        symbols.truncate(max_results);
        Ok(ToolResult::success(
            format!("found {} symbols matching {query}", symbols.len()),
            serde_json::json!(symbols),
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

#[derive(Clone, Debug, serde::Serialize)]
struct SymbolEntry {
    path: String,
    line: usize,
    kind: String,
    name: String,
    signature: String,
}

fn scoped_path(ctx: &ToolContext, params: &Value) -> PeriResult<std::path::PathBuf> {
    let path = params.get("path").and_then(Value::as_str).map_or_else(
        || ctx.project_root.clone(),
        |path| ctx.project_root.join(path),
    );
    ensure_within_project(&ctx.project_root, &path)
}

fn max_results(params: &Value, default: usize) -> usize {
    params
        .get("max_results")
        .and_then(Value::as_u64)
        .map(|value| value.clamp(1, 1000) as usize)
        .unwrap_or(default)
}

fn collect_symbols(
    project_root: &Path,
    path: &Path,
    limit: usize,
    symbols: &mut Vec<SymbolEntry>,
) -> PeriResult<()> {
    if symbols.len() >= limit {
        return Ok(());
    }
    if path.is_dir() {
        if should_skip_symbol_dir(path) {
            return Ok(());
        }
        let mut entries = fs::read_dir(path)
            .map_err(|err| PeriError::Tool(format!("failed to scan {}: {err}", path.display())))?
            .flatten()
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        entries.sort();
        for entry in entries {
            collect_symbols(project_root, &entry, limit, symbols)?;
            if symbols.len() >= limit {
                break;
            }
        }
        return Ok(());
    }
    if is_source_file(path) {
        symbols.extend(outline_file(
            project_root,
            path,
            limit.saturating_sub(symbols.len()),
        )?);
    }
    Ok(())
}

fn outline_file(project_root: &Path, path: &Path, limit: usize) -> PeriResult<Vec<SymbolEntry>> {
    let metadata = fs::metadata(path)
        .map_err(|err| PeriError::Tool(format!("failed to stat {}: {err}", path.display())))?;
    if metadata.len() > MAX_SYMBOL_FILE_BYTES {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(path)
        .map_err(|err| PeriError::Tool(format!("failed to read {}: {err}", path.display())))?;
    let relative = path
        .strip_prefix(project_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let mut symbols = Vec::new();
    for (line_idx, line) in content.lines().enumerate() {
        if let Some((kind, name)) = detect_symbol(line, extension) {
            symbols.push(SymbolEntry {
                path: relative.clone(),
                line: line_idx + 1,
                kind,
                name,
                signature: line.trim().to_string(),
            });
            if symbols.len() >= limit {
                break;
            }
        }
    }
    Ok(symbols)
}

fn should_skip_symbol_dir(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    matches!(
        name,
        ".git" | ".peridot" | "target" | "node_modules" | "dist" | "build" | ".next"
    )
}

fn is_source_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some(
            "rs" | "ts"
                | "tsx"
                | "js"
                | "jsx"
                | "py"
                | "go"
                | "java"
                | "kt"
                | "swift"
                | "c"
                | "cc"
                | "cpp"
                | "h"
                | "hpp"
        )
    )
}

fn detect_symbol(line: &str, extension: &str) -> Option<(String, String)> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with('*') {
        return None;
    }
    match extension {
        "rs" => detect_rust_symbol(trimmed),
        "ts" | "tsx" | "js" | "jsx" => detect_js_symbol(trimmed),
        "py" => detect_python_symbol(trimmed),
        "go" => detect_go_symbol(trimmed),
        _ => detect_generic_symbol(trimmed),
    }
}

fn detect_rust_symbol(line: &str) -> Option<(String, String)> {
    let line = line.strip_prefix("pub(crate) ").unwrap_or(line);
    let line = line.strip_prefix("pub ").unwrap_or(line);
    let line = line.strip_prefix("async ").unwrap_or(line);
    for (prefix, kind) in [
        ("fn ", "function"),
        ("struct ", "struct"),
        ("enum ", "enum"),
        ("trait ", "trait"),
        ("mod ", "module"),
        ("impl ", "impl"),
    ] {
        if let Some(rest) = line.strip_prefix(prefix) {
            return Some((kind.to_string(), symbol_name(rest)));
        }
    }
    None
}

fn detect_js_symbol(line: &str) -> Option<(String, String)> {
    let line = line.strip_prefix("export default ").unwrap_or(line);
    let line = line.strip_prefix("export ").unwrap_or(line);
    for (prefix, kind) in [
        ("async function ", "function"),
        ("function ", "function"),
        ("class ", "class"),
        ("interface ", "interface"),
        ("type ", "type"),
        ("const ", "constant"),
        ("let ", "variable"),
        ("var ", "variable"),
    ] {
        if let Some(rest) = line.strip_prefix(prefix) {
            return Some((kind.to_string(), symbol_name(rest)));
        }
    }
    None
}

fn detect_python_symbol(line: &str) -> Option<(String, String)> {
    for (prefix, kind) in [
        ("async def ", "function"),
        ("def ", "function"),
        ("class ", "class"),
    ] {
        if let Some(rest) = line.strip_prefix(prefix) {
            return Some((kind.to_string(), symbol_name(rest)));
        }
    }
    None
}

fn detect_go_symbol(line: &str) -> Option<(String, String)> {
    if let Some(rest) = line.strip_prefix("func ") {
        let name = if let Some(after_receiver) = rest.strip_prefix('(') {
            after_receiver
                .split_once(')')
                .map(|(_, rest)| symbol_name(rest.trim_start()))
                .unwrap_or_else(|| symbol_name(rest))
        } else {
            symbol_name(rest)
        };
        return Some(("function".to_string(), name));
    }
    if let Some(rest) = line.strip_prefix("type ") {
        return Some(("type".to_string(), symbol_name(rest)));
    }
    None
}

fn detect_generic_symbol(line: &str) -> Option<(String, String)> {
    for (prefix, kind) in [
        ("class ", "class"),
        ("interface ", "interface"),
        ("func ", "function"),
        ("function ", "function"),
    ] {
        if let Some(rest) = line.strip_prefix(prefix) {
            return Some((kind.to_string(), symbol_name(rest)));
        }
    }
    None
}

fn symbol_name(rest: &str) -> String {
    rest.trim_start()
        .trim_start_matches('*')
        .chars()
        .take_while(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '-' | '$'))
        .collect::<String>()
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
