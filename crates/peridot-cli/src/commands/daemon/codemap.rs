//! TODO-index and workspace code-map slash command handlers (`/todos`,
//! `/codemap [status|refresh|find|locate|outline|refs]`) plus their
//! result builders, split out of the daemon module. Reached via
//! `use super::*`; `code_map_status_summary` is also used by
//! `handle_status` so it is re-exported to the parent with `pub(super)`.

use serde_json::Value;

use super::*;

pub(super) fn handle_command_todos(
    state: &DaemonState,
    raw_command: &str,
) -> Result<Value, String> {
    let load = crate::commands::load_or_refresh_code_map_index_with_status(
        state.project_root.as_ref(),
        crate::commands::DEFAULT_MAX_SYMBOLS,
        crate::commands::TODO_SCAN_MAX_TODOS,
    )
    .map_err(|err| format!("todos: failed to load code map index: {err}"))?;
    let report = crate::commands::todo_code_map_report(&load.index);
    let items = report
        .todos
        .iter()
        .map(|todo| {
            serde_json::json!({
                "source": "todo",
                "label": todo.marker,
                "path": todo.path,
                "line": todo.line,
                "detail": todo.text,
            })
        })
        .collect::<Vec<_>>();
    let message = if items.is_empty() {
        format!(
            "todos: no markers found (indexed {} file(s) at {})",
            report.walked_files, load.index.generated_at_unix
        )
    } else {
        format!(
            "todos: {} hit(s) across {} indexed file(s) (indexed at {})",
            items.len(),
            report.walked_files,
            load.index.generated_at_unix
        )
    };
    Ok(serde_json::json!({
        "kind": "todos",
        "title": "TODOs",
        "message": message,
        "severity": "info",
        "command": raw_command,
        "items": items,
        "walked_files": report.walked_files,
        "generated_at_unix": load.index.generated_at_unix,
        "refreshed": load.refreshed,
        "truncated": report.todos_truncated,
    }))
}

pub(super) fn handle_command_codemap(
    state: &DaemonState,
    raw_command: &str,
    refresh: bool,
) -> Result<Value, String> {
    let index = if refresh {
        crate::commands::CodeMapIndexLoad {
            index: crate::commands::refresh_code_map_index(
                state.project_root.as_ref(),
                crate::commands::DEFAULT_MAX_SYMBOLS,
                crate::commands::DEFAULT_MAX_TODOS,
            )
            .map_err(|err| format!("codemap: failed to load index: {err}"))?,
            refreshed: true,
        }
    } else {
        crate::commands::load_or_refresh_code_map_index_with_status(
            state.project_root.as_ref(),
            crate::commands::DEFAULT_MAX_SYMBOLS,
            crate::commands::DEFAULT_MAX_TODOS,
        )
        .map_err(|err| format!("codemap: failed to load index: {err}"))?
    };
    Ok(code_map_result(
        raw_command,
        "Workspace Code Map",
        None,
        &index.index.report,
        index.index.generated_at_unix,
        index.refreshed,
    ))
}

pub(super) fn handle_command_codemap_status(
    state: &DaemonState,
    raw_command: &str,
) -> Result<Value, String> {
    let status = crate::commands::code_map_status(state.project_root.as_ref())
        .map_err(|err| format!("codemap: failed to check status: {err}"))?;
    Ok(code_map_status_result(raw_command, &status))
}

pub(super) fn handle_command_codemap_find(
    state: &DaemonState,
    raw_command: &str,
    query: &str,
) -> Result<Value, String> {
    let load = crate::commands::load_or_refresh_code_map_index_with_status(
        state.project_root.as_ref(),
        crate::commands::DEFAULT_MAX_SYMBOLS,
        crate::commands::DEFAULT_MAX_TODOS,
    )
    .map_err(|err| format!("codemap: failed to load index: {err}"))?;
    let report = crate::commands::search_code_map_index(&load.index, query);
    Ok(code_map_result(
        raw_command,
        "Workspace Code Map Search",
        Some(query),
        &report,
        load.index.generated_at_unix,
        load.refreshed,
    ))
}

pub(super) fn handle_command_codemap_locate(
    state: &DaemonState,
    raw_command: &str,
    query: &str,
) -> Result<Value, String> {
    let load = crate::commands::load_or_refresh_code_map_index_with_status(
        state.project_root.as_ref(),
        crate::commands::DEFAULT_MAX_SYMBOLS,
        crate::commands::DEFAULT_MAX_TODOS,
    )
    .map_err(|err| format!("codemap: failed to load index: {err}"))?;
    let report = crate::commands::locate_code_map_symbols(&load.index, query);
    Ok(code_map_result(
        raw_command,
        "Workspace Symbol Locations",
        Some(query),
        &report,
        load.index.generated_at_unix,
        load.refreshed,
    ))
}

pub(super) fn handle_command_codemap_outline(
    state: &DaemonState,
    raw_command: &str,
    path: &str,
) -> Result<Value, String> {
    let load = crate::commands::load_or_refresh_code_map_index_with_status(
        state.project_root.as_ref(),
        crate::commands::DEFAULT_MAX_SYMBOLS,
        crate::commands::DEFAULT_MAX_TODOS,
    )
    .map_err(|err| format!("codemap: failed to load index: {err}"))?;
    let report = crate::commands::outline_code_map_file(&load.index, path);
    Ok(code_map_result(
        raw_command,
        "Workspace File Outline",
        Some(path),
        &report,
        load.index.generated_at_unix,
        load.refreshed,
    ))
}

pub(super) fn handle_command_codemap_refs(
    state: &DaemonState,
    raw_command: &str,
    query: &str,
) -> Result<Value, String> {
    let load = crate::commands::load_or_refresh_code_map_index_with_status(
        state.project_root.as_ref(),
        crate::commands::DEFAULT_MAX_SYMBOLS,
        crate::commands::DEFAULT_MAX_TODOS,
    )
    .map_err(|err| format!("codemap: failed to load index: {err}"))?;
    let report = crate::commands::find_code_map_references(
        state.project_root.as_ref(),
        &load.index,
        query,
        80,
    );
    Ok(code_map_result(
        raw_command,
        "Workspace Symbol References",
        Some(query),
        &report,
        load.index.generated_at_unix,
        load.refreshed,
    ))
}

fn code_map_status_result(raw_command: &str, status: &crate::commands::CodeMapStatus) -> Value {
    let state = if !status.index_exists {
        "missing"
    } else if status.stale {
        "stale"
    } else {
        "fresh"
    };
    let generated = status
        .generated_at_unix
        .map(|ts| ts.to_string())
        .unwrap_or_else(|| "none".to_string());
    let newest = status
        .newest_source_mtime_unix
        .map(|ts| ts.to_string())
        .unwrap_or_else(|| "none".to_string());
    let message =
        format!("codemap: index {state} (indexed at {generated}, newest source {newest})");
    serde_json::json!({
        "kind": "codemap_status",
        "title": "Workspace Code Map Status",
        "message": message,
        "severity": if status.stale { "warning" } else { "info" },
        "command": raw_command,
        "code_map": code_map_status_summary(status),
        "index_exists": status.index_exists,
        "stale": status.stale,
        "generated_at_unix": status.generated_at_unix,
        "newest_source_mtime_unix": status.newest_source_mtime_unix,
        "source_files": status.source_files,
        "walked_files": status.walked_files,
        "symbol_count": status.symbol_count,
        "todo_count": status.todo_count,
        "items": [
            { "label": "state", "detail": state },
            { "label": "source files", "detail": status.source_files.to_string() },
            { "label": "indexed files", "detail": status.walked_files.to_string() },
            { "label": "symbols", "detail": status.symbol_count.to_string() },
            { "label": "TODOs", "detail": status.todo_count.to_string() },
        ],
    })
}

pub(super) fn code_map_status_summary(status: &crate::commands::CodeMapStatus) -> Value {
    serde_json::json!({
        "index_exists": status.index_exists,
        "stale": status.stale,
        "generated_at_unix": status.generated_at_unix,
        "newest_source_mtime_unix": status.newest_source_mtime_unix,
        "source_files": status.source_files,
        "walked_files": status.walked_files,
        "symbol_count": status.symbol_count,
        "todo_count": status.todo_count,
    })
}

fn code_map_result(
    raw_command: &str,
    title: &str,
    query: Option<&str>,
    report: &crate::commands::CodeMapReport,
    generated_at_unix: u64,
    refreshed: bool,
) -> Value {
    let mut items = Vec::new();
    for symbol in report.symbols.iter().take(80) {
        items.push(serde_json::json!({
            "source": "symbol",
            "label": format!("{} {}", symbol.kind, symbol.name),
            "path": symbol.path,
            "line": symbol.line,
            "detail": symbol.signature,
        }));
    }
    for todo in report.todos.iter().take(40) {
        items.push(serde_json::json!({
            "source": "todo",
            "label": todo.marker,
            "path": todo.path,
            "line": todo.line,
            "detail": todo.text,
        }));
    }
    for reference in report.references.iter().take(80) {
        items.push(serde_json::json!({
            "source": "reference",
            "label": reference.symbol,
            "path": reference.path,
            "line": reference.line,
            "detail": reference.text,
        }));
    }
    let truncated = report.symbols_truncated
        || report.todos_truncated
        || report.references_truncated
        || report.symbols.len() > 80
        || report.todos.len() > 40
        || report.references.len() > 80;
    let reference_result = title.contains("References");
    let message = if let Some(query) = query {
        if reference_result {
            format!(
                "codemap: {} reference match(es) for '{}' across {} file(s) (indexed at {})",
                report.references.len(),
                query,
                report.walked_files,
                generated_at_unix,
            )
        } else {
            format!(
                "codemap: {} symbol match(es), {} TODO match(es) for '{}' across {} file(s) (indexed at {})",
                report.symbols.len(),
                report.todos.len(),
                query,
                report.walked_files,
                generated_at_unix,
            )
        }
    } else {
        format!(
            "codemap: {} symbol(s), {} TODO marker(s) across {} file(s) (indexed at {})",
            report.symbols.len(),
            report.todos.len(),
            report.walked_files,
            generated_at_unix,
        )
    };
    let mut result = serde_json::json!({
        "kind": "codemap",
        "title": title,
        "message": message,
        "severity": "info",
        "command": raw_command,
        "items": items,
        "symbol_count": report.symbols.len(),
        "todo_count": report.todos.len(),
        "reference_count": report.references.len(),
        "walked_files": report.walked_files,
        "generated_at_unix": generated_at_unix,
        "refreshed": refreshed,
        "truncated": truncated,
    });
    if let Some(query) = query {
        result["query"] = serde_json::json!(query);
    }
    result
}
