use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

use async_trait::async_trait;
use peridot_common::{PeriError, PeriResult, PermissionLevel, ToolGroup, ToolResult};
use serde_json::Value;

use crate::hooks::{HookRunner, HookVariables};
use crate::path::{ensure_within_project, required_str, workspace_path};
use crate::{Tool, ToolContext};

const MAX_SYMBOL_FILE_BYTES: u64 = 1_000_000;

/// Cap on the number of files held in the per-file outline cache. When
/// exceeded the cache is cleared wholesale — crude but bounded, and the entries
/// rebuild lazily on the next query.
const OUTLINE_CACHE_MAX_FILES: usize = 8_192;

/// A cached per-file symbol outline, valid while the file's mod/size are
/// unchanged. Incremental semantic code map (feature F1): repeated
/// `workspace_symbols` / `symbol_search` / `symbol_definition` queries reuse
/// parsed results and only re-parse files that actually changed.
struct OutlineCacheEntry {
    mtime: Option<SystemTime>,
    size: u64,
    /// The full outline (no result limit), keyed below by absolute path.
    symbols: Vec<SymbolEntry>,
}

static OUTLINE_CACHE: LazyLock<Mutex<HashMap<PathBuf, OutlineCacheEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Returns the cached full outline for `path` when the cache entry's mtime and
/// size still match `metadata`.
fn outline_cache_get(
    path: &Path,
    mtime: Option<SystemTime>,
    size: u64,
) -> Option<Vec<SymbolEntry>> {
    let cache = OUTLINE_CACHE.lock().ok()?;
    let entry = cache.get(path)?;
    if entry.size == size && entry.mtime == mtime {
        Some(entry.symbols.clone())
    } else {
        None
    }
}

/// Stores the full outline for `path` under its current mtime/size.
fn outline_cache_put(path: &Path, mtime: Option<SystemTime>, size: u64, symbols: Vec<SymbolEntry>) {
    let Ok(mut cache) = OUTLINE_CACHE.lock() else {
        return;
    };
    if cache.len() >= OUTLINE_CACHE_MAX_FILES && !cache.contains_key(path) {
        cache.clear();
    }
    cache.insert(
        path.to_path_buf(),
        OutlineCacheEntry {
            mtime,
            size,
            symbols,
        },
    );
}

/// Built-in evidence ledger reader.
#[derive(Clone, Debug)]
pub struct EvidenceReadTool;

#[async_trait]
impl Tool for EvidenceReadTool {
    fn name(&self) -> &str {
        "evidence_read"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::File
    }

    fn description(&self) -> &str {
        "Read a raw evidence ledger record by id, optionally returning a character slice"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Evidence id from a recoverable evidence ref"
                },
                "offset": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "Character offset to start reading from"
                },
                "max_chars": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 20000,
                    "description": "Maximum characters to return"
                }
            },
            "required": ["id"],
            "additionalProperties": false,
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let id = required_str(&params, "id")?;
        if !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
        {
            return Err(PeriError::Tool(format!("invalid evidence id: {id}")));
        }
        let offset = params
            .get("offset")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .min(usize::MAX as u64) as usize;
        let max_chars = params
            .get("max_chars")
            .and_then(Value::as_u64)
            .unwrap_or(12_000)
            .clamp(1, 20_000) as usize;
        let path = ctx
            .project_root
            .join(".peridot")
            .join("evidence")
            .join(format!("{id}.json"));
        let path = ensure_within_project(&ctx.project_root, &path)?;
        let content = fs::read_to_string(&path)
            .map_err(|err| PeriError::Tool(format!("failed to read {}: {err}", path.display())))?;
        let total_chars = content.chars().count();
        let slice = content
            .chars()
            .skip(offset)
            .take(max_chars)
            .collect::<String>();
        let end = (offset + slice.chars().count()).min(total_chars);
        let relative = path
            .strip_prefix(&ctx.project_root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        Ok(ToolResult::success(
            format!("read evidence {id} chars {offset}..{end} of {total_chars}"),
            serde_json::json!({
                "id": id,
                "path": relative,
                "offset": offset,
                "end": end,
                "total_chars": total_chars,
                "truncated": end < total_chars,
                "content": slice,
            }),
        ))
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
    }
}

#[derive(Clone, Debug)]
struct DecodedText {
    content: String,
    lossy: bool,
}

fn read_text_file(path: &Path) -> PeriResult<DecodedText> {
    let bytes = fs::read(path)
        .map_err(|err| PeriError::Tool(format!("failed to read {}: {err}", path.display())))?;
    match String::from_utf8(bytes) {
        Ok(content) => Ok(DecodedText {
            content,
            lossy: false,
        }),
        Err(err) => Ok(DecodedText {
            content: String::from_utf8_lossy(err.as_bytes()).into_owned(),
            lossy: true,
        }),
    }
}

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
        let decoded = read_text_file(&path)?;
        let summary = if decoded.lossy {
            format!("read {} (invalid UTF-8 bytes replaced)", path.display())
        } else {
            format!("read {}", path.display())
        };
        Ok(ToolResult::success(summary, Value::String(decoded.content)))
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

/// Built-in ripgrep-backed workspace search tool.
#[derive(Clone, Debug)]
pub struct RipgrepSearchTool;

#[async_trait]
impl Tool for RipgrepSearchTool {
    fn name(&self) -> &str {
        "ripgrep_search"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::File
    }

    fn description(&self) -> &str {
        "Fast read-only workspace text search using ripgrep when available. Prefer this over shell_exec for code and text searches."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Text or regex pattern to search for"},
                "path": {
                    "type": "string",
                    "description": "Optional project-relative file or directory to search"
                },
                "glob": {
                    "oneOf": [
                        {"type": "string"},
                        {"type": "array", "items": {"type": "string"}, "maxItems": 16}
                    ],
                    "description": "Optional ripgrep glob(s), e.g. '*.rs' or '!target/**'"
                },
                "regex": {
                    "type": "boolean",
                    "description": "Treat query as a regex. Defaults to false (literal search)."
                },
                "case_sensitive": {
                    "type": "boolean",
                    "description": "Case-sensitive search. Defaults to true."
                },
                "context_lines": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 5,
                    "description": "Context lines before and after each match"
                },
                "max_matches": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 200,
                    "description": "Maximum match records to return"
                }
            },
            "required": ["query"],
            "additionalProperties": false,
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let query = required_str(&params, "query")?;
        let search_root = params.get("path").and_then(Value::as_str).map_or_else(
            || ctx.project_root.clone(),
            |path| ctx.project_root.join(path),
        );
        let search_root = ensure_within_project(&ctx.project_root, &search_root)?;
        let max_matches = params
            .get("max_matches")
            .and_then(Value::as_u64)
            .unwrap_or(50)
            .clamp(1, 200) as usize;
        let context_lines = params
            .get("context_lines")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .clamp(0, 5) as usize;
        let regex = params
            .get("regex")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let case_sensitive = params
            .get("case_sensitive")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let globs = search_globs(params.get("glob"))?;
        match run_ripgrep_search(
            &ctx.project_root,
            &search_root,
            query,
            &globs,
            regex,
            case_sensitive,
            context_lines,
            max_matches,
        ) {
            Ok(result) => Ok(result),
            Err(PeriError::Tool(message)) if message.contains("`rg` not installed") => {
                fallback_substring_search(&ctx.project_root, &search_root, query, max_matches)
            }
            Err(err) => Err(err),
        }
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

/// Built-in symbol definition lookup tool. Returns the definition site(s) of
/// a symbol by exact name across the workspace — the semantic counterpart to
/// grepping for `fn name`.
#[derive(Clone, Debug)]
pub struct SymbolDefinitionTool;

#[async_trait]
impl Tool for SymbolDefinitionTool {
    fn name(&self) -> &str {
        "symbol_definition"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::File
    }

    fn description(&self) -> &str {
        "Find where a symbol is defined (exact name) across the workspace"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Exact symbol name to locate"},
                "path": {"type": "string", "description": "Optional project-relative directory or file"},
                "max_results": {"type": "integer", "minimum": 1, "maximum": 200}
            },
            "required": ["name"],
            "additionalProperties": false,
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let name = required_str(&params, "name")?.to_string();
        let path = scoped_path(ctx, &params)?;
        let max_results = max_results(&params, 50);
        let mut symbols = Vec::new();
        collect_symbols(
            &ctx.project_root,
            &path,
            max_results.saturating_mul(20),
            &mut symbols,
        )?;
        symbols.retain(|symbol| symbol.name == name);
        symbols.truncate(max_results);
        Ok(ToolResult::success(
            format!("found {} definitions of {name}", symbols.len()),
            serde_json::json!(symbols),
        ))
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
    }
}

/// Built-in symbol reference lookup tool. Returns identifier-token occurrences
/// of a name across the workspace. Rust files are scanned AST-aware via
/// tree-sitter (occurrences in comments/strings excluded); other source files
/// fall back to a word-boundary textual scan.
#[derive(Clone, Debug)]
pub struct SymbolReferencesTool;

#[async_trait]
impl Tool for SymbolReferencesTool {
    fn name(&self) -> &str {
        "symbol_references"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::File
    }

    fn description(&self) -> &str {
        "Find references of a symbol by name across the workspace; each result is tagged definition or usage"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Exact symbol name to find usages of"},
                "path": {"type": "string", "description": "Optional project-relative directory or file"},
                "max_results": {"type": "integer", "minimum": 1, "maximum": 1000}
            },
            "required": ["name"],
            "additionalProperties": false,
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let name = required_str(&params, "name")?.to_string();
        let path = scoped_path(ctx, &params)?;
        let max_results = max_results(&params, 200);
        let mut refs = Vec::new();
        collect_references(&ctx.project_root, &path, &name, max_results, &mut refs)?;
        Ok(ToolResult::success(
            format!("found {} references to {name}", refs.len()),
            serde_json::json!(refs),
        ))
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
    }
}

/// Recursively collects identifier references to `name` under `path`.
fn collect_references(
    project_root: &Path,
    path: &Path,
    name: &str,
    limit: usize,
    out: &mut Vec<Value>,
) -> PeriResult<()> {
    if out.len() >= limit {
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
            collect_references(project_root, &entry, name, limit, out)?;
            if out.len() >= limit {
                break;
            }
        }
        return Ok(());
    }
    if !is_source_file(path) {
        return Ok(());
    }
    let Ok(metadata) = fs::metadata(path) else {
        return Ok(());
    };
    if metadata.len() > MAX_SYMBOL_FILE_BYTES {
        return Ok(());
    }
    let Ok(content) = fs::read_to_string(path) else {
        return Ok(());
    };
    let relative = path
        .strip_prefix(project_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let lines: Vec<&str> = content.lines().collect();
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");

    if let Some(references) = peridot_symbols::references_for_extension(extension, &content, name) {
        for reference in references {
            if out.len() >= limit {
                break;
            }
            let text = lines
                .get(reference.line.saturating_sub(1))
                .map(|line| line.trim().to_string())
                .unwrap_or_default();
            out.push(serde_json::json!({
                "path": relative,
                "line": reference.line,
                "column": reference.column,
                "kind": if reference.is_definition { "definition" } else { "usage" },
                "text": text,
            }));
        }
    } else {
        for (line_idx, line) in lines.iter().enumerate() {
            for column in word_boundary_matches(line, name) {
                if out.len() >= limit {
                    return Ok(());
                }
                out.push(serde_json::json!({
                    "path": relative,
                    "line": line_idx + 1,
                    "column": column + 1,
                    "text": line.trim(),
                }));
            }
        }
    }
    Ok(())
}

/// Byte-column offsets where `needle` appears in `line` as a whole word
/// (neither neighbour is an identifier character). Used as the non-Rust
/// reference fallback until more grammars are wired in.
fn word_boundary_matches(line: &str, needle: &str) -> Vec<usize> {
    if needle.is_empty() {
        return Vec::new();
    }
    let bytes = line.as_bytes();
    let mut hits = Vec::new();
    let mut start = 0;
    while let Some(rel) = line[start..].find(needle) {
        let at = start + rel;
        let before_ok = at == 0 || !is_ident_byte(bytes[at - 1]);
        let after = at + needle.len();
        let after_ok = after >= bytes.len() || !is_ident_byte(bytes[after]);
        if before_ok && after_ok {
            hits.push(at);
        }
        start = at + needle.len();
    }
    hits
}

fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
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

fn search_globs(value: Option<&Value>) -> PeriResult<Vec<String>> {
    match value {
        None => Ok(Vec::new()),
        Some(Value::String(glob)) => Ok(vec![glob.clone()]),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str().map(ToString::to_string).ok_or_else(|| {
                    PeriError::Tool("glob array entries must be strings".to_string())
                })
            })
            .collect(),
        Some(_) => Err(PeriError::Tool(
            "glob must be a string or array of strings".to_string(),
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_ripgrep_search(
    project_root: &Path,
    search_root: &Path,
    query: &str,
    globs: &[String],
    regex: bool,
    case_sensitive: bool,
    context_lines: usize,
    max_matches: usize,
) -> PeriResult<ToolResult> {
    let mut command = Command::new("rg");
    command
        .arg("--json")
        .arg("--line-number")
        .arg("--column")
        .arg("--color")
        .arg("never")
        .arg("--no-messages");
    if !regex {
        command.arg("--fixed-strings");
    }
    if !case_sensitive {
        command.arg("--ignore-case");
    }
    if context_lines > 0 {
        command.arg("--context").arg(context_lines.to_string());
    }
    for default_ignore in [
        ".git/**",
        "target/**",
        "node_modules/**",
        ".peridot/evidence/**",
    ] {
        command.arg("--glob").arg(format!("!{default_ignore}"));
    }
    for glob in globs {
        command.arg("--glob").arg(glob);
    }
    command
        .arg(query)
        .arg(search_root)
        .current_dir(project_root);
    let output = command.output().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            PeriError::Tool("`rg` not installed; falling back unavailable".to_string())
        } else {
            PeriError::Tool(format!("failed to run ripgrep_search: {err}"))
        }
    })?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(PeriError::Tool(format!(
            "ripgrep_search exited {}: {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut matches = Vec::new();
    let mut context = Vec::new();
    for line in stdout.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match value.get("type").and_then(Value::as_str) {
            Some("match") => {
                if matches.len() >= max_matches {
                    continue;
                }
                if let Some(record) = ripgrep_json_record(project_root, &value, "match") {
                    matches.push(record);
                }
            }
            Some("context") => {
                if context.len()
                    < max_matches.saturating_mul(context_lines.saturating_mul(2).max(1))
                    && let Some(record) = ripgrep_json_record(project_root, &value, "context")
                {
                    context.push(record);
                }
            }
            _ => {}
        }
    }
    Ok(ToolResult::success(
        format!("ripgrep_search: {} matches for {query:?}", matches.len()),
        serde_json::json!({
            "backend": "rg",
            "query": query,
            "matches": matches,
            "context": context,
            "truncated": matches.len() >= max_matches,
        }),
    ))
}

fn ripgrep_json_record(project_root: &Path, value: &Value, kind: &str) -> Option<Value> {
    let data = value.get("data")?;
    let path = data.get("path")?.get("text")?.as_str().unwrap_or_default();
    let relative = Path::new(path)
        .strip_prefix(project_root)
        .unwrap_or_else(|_| Path::new(path))
        .to_string_lossy()
        .replace('\\', "/");
    let lines = data
        .get("lines")
        .and_then(|lines| lines.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim_end_matches('\n');
    let line_number = data.get("line_number").and_then(Value::as_u64).unwrap_or(0);
    let submatches = data
        .get("submatches")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(serde_json::json!({
                        "start": item.get("start")?.as_u64()?,
                        "end": item.get("end")?.as_u64()?,
                        "text": item.get("match")?.get("text")?.as_str()?,
                    }))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some(serde_json::json!({
        "kind": kind,
        "path": relative,
        "line": line_number,
        "text": lines,
        "submatches": submatches,
    }))
}

fn fallback_substring_search(
    project_root: &Path,
    search_root: &Path,
    query: &str,
    max_matches: usize,
) -> PeriResult<ToolResult> {
    let mut matches = Vec::new();
    search_path_limited(project_root, search_root, query, &mut matches, max_matches)?;
    Ok(ToolResult::success(
        format!(
            "ripgrep_search fallback: {} matches for {query:?}",
            matches.len()
        ),
        serde_json::json!({
            "backend": "builtin_substring",
            "query": query,
            "matches": matches,
            "truncated": matches.len() >= max_matches,
        }),
    ))
}

fn search_path_limited(
    project_root: &Path,
    path: &Path,
    pattern: &str,
    matches: &mut Vec<Value>,
    max_matches: usize,
) -> PeriResult<()> {
    if matches.len() >= max_matches {
        return Ok(());
    }
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
            search_path_limited(project_root, &entry.path(), pattern, matches, max_matches)?;
            if matches.len() >= max_matches {
                break;
            }
        }
        return Ok(());
    }
    if !path.is_file() {
        return Ok(());
    }
    let Ok(content) = fs::read_to_string(path) else {
        return Ok(());
    };
    let relative = path
        .strip_prefix(project_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    for (line_idx, line) in content.lines().enumerate() {
        if line.contains(pattern) {
            matches.push(serde_json::json!({
                "kind": "match",
                "path": relative,
                "line": line_idx + 1,
                "text": line
            }));
            if matches.len() >= max_matches {
                break;
            }
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
    /// Owning type/class for associated items (e.g. `Scanner` for
    /// `Scanner::scan`). Omitted for top-level symbols and for the
    /// line-based heuristic, which has no container information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    container: Option<String>,
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
    let mtime = metadata.modified().ok();
    let size = metadata.len();
    // Reuse the cached outline when the file is unchanged; otherwise parse and
    // cache the full outline. The result `limit` is applied on return so the
    // cache is independent of any single query's cap.
    let full = match outline_cache_get(path, mtime, size) {
        Some(cached) => cached,
        None => {
            let built = build_outline_full(project_root, path)?;
            outline_cache_put(path, mtime, size, built.clone());
            built
        }
    };
    Ok(full.into_iter().take(limit).collect())
}

/// Parses a file's complete symbol outline (no result limit). Tree-sitter
/// languages get a real parse; others fall back to the line heuristic.
fn build_outline_full(project_root: &Path, path: &Path) -> PeriResult<Vec<SymbolEntry>> {
    let content = read_text_file(path)?.content;
    let relative = path
        .strip_prefix(project_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    // Languages with a tree-sitter grammar get a real parse (feature F1):
    // accurate kinds, class/impl association, and multi-line-aware start
    // positions. Other languages keep the line-based heuristic.
    if let Some(parsed) = peridot_symbols::outline_for_extension(extension, &content) {
        let lines: Vec<&str> = content.lines().collect();
        let mut symbols = Vec::new();
        for symbol in parsed {
            let signature = lines
                .get(symbol.start_line.saturating_sub(1))
                .map(|line| line.trim().to_string())
                .unwrap_or_default();
            symbols.push(SymbolEntry {
                path: relative.clone(),
                line: symbol.start_line,
                kind: symbol.kind.label().to_string(),
                name: symbol.name,
                container: symbol.container,
                signature,
            });
        }
        return Ok(symbols);
    }
    let mut symbols = Vec::new();
    for (line_idx, line) in content.lines().enumerate() {
        if let Some((kind, name)) = detect_symbol(line, extension) {
            symbols.push(SymbolEntry {
                path: relative.clone(),
                line: line_idx + 1,
                kind,
                name,
                container: None,
                signature: line.trim().to_string(),
            });
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
