//! Interactive `/codemap` and `/todos` slash-command handlers.
//!
//! Glue between the TUI and the workspace code-map index in [`crate::commands`]:
//! each `handle_*` runs an index query (load / refresh / search / locate /
//! outline / refs / status), updates the Status side panel's code-map summary,
//! and pushes a rendered report into the transcript. Split out of `main.rs` so
//! the code-map UI lives in one place. The slash dispatcher in
//! `apply_session_command` calls these.

use std::path::Path;

use peridot_tui::{CodeMapSummary, TuiState};

use crate::commands;

pub(crate) fn handle_code_map(state: &mut TuiState, project_root: &Path, refresh: bool) {
    let index = if refresh {
        commands::refresh_code_map_index(
            project_root,
            commands::DEFAULT_MAX_SYMBOLS,
            commands::DEFAULT_MAX_TODOS,
        )
    } else {
        commands::load_or_refresh_code_map_index(
            project_root,
            commands::DEFAULT_MAX_SYMBOLS,
            commands::DEFAULT_MAX_TODOS,
        )
    };
    let Ok(index) = index else {
        state.push_error("codemap: failed to load workspace code map index");
        return;
    };
    state.side_panel.code_map = Some(code_map_summary_from_index(&index, refresh));
    let report = &index.report;
    if report.symbols.is_empty() && report.todos.is_empty() {
        state.push_transcript(format!(
            "codemap: no symbols or TODO markers found (scanned {} file(s))",
            report.walked_files
        ));
        return;
    }
    state.push_transcript(render_code_map_text(&index));
}

pub(crate) fn handle_code_map_status(state: &mut TuiState, project_root: &Path) {
    match commands::code_map_status(project_root) {
        Ok(status) => {
            state.side_panel.code_map = Some(code_map_summary_from_status(&status));
            state.push_transcript(render_code_map_status_text(&status));
        }
        Err(_) => state.push_error("codemap: failed to check workspace code map status"),
    }
}

pub(crate) fn handle_code_map_find(state: &mut TuiState, project_root: &Path, query: &str) {
    let index = commands::load_or_refresh_code_map_index(
        project_root,
        commands::DEFAULT_MAX_SYMBOLS,
        commands::DEFAULT_MAX_TODOS,
    );
    let Ok(index) = index else {
        state.push_error("codemap: failed to load workspace code map index");
        return;
    };
    state.side_panel.code_map = Some(code_map_summary_from_index(&index, false));
    let report = commands::search_code_map_index(&index, query);
    if report.symbols.is_empty() && report.todos.is_empty() {
        state.push_transcript(format!(
            "codemap: no matches for '{query}' (indexed at {})",
            index.generated_at_unix
        ));
        return;
    }
    state.push_transcript(render_code_map_report(
        &report,
        index.generated_at_unix,
        Some(query),
    ));
}

pub(crate) fn handle_code_map_locate(state: &mut TuiState, project_root: &Path, query: &str) {
    let index = commands::load_or_refresh_code_map_index(
        project_root,
        commands::DEFAULT_MAX_SYMBOLS,
        commands::DEFAULT_MAX_TODOS,
    );
    let Ok(index) = index else {
        state.push_error("codemap: failed to load workspace code map index");
        return;
    };
    state.side_panel.code_map = Some(code_map_summary_from_index(&index, false));
    let report = commands::locate_code_map_symbols(&index, query);
    if report.symbols.is_empty() {
        state.push_transcript(format!(
            "codemap: no symbol matches for '{query}' (indexed at {})",
            index.generated_at_unix
        ));
        return;
    }
    state.push_transcript(render_code_map_report(
        &report,
        index.generated_at_unix,
        Some(query),
    ));
}

pub(crate) fn handle_code_map_outline(state: &mut TuiState, project_root: &Path, path: &str) {
    let index = commands::load_or_refresh_code_map_index(
        project_root,
        commands::DEFAULT_MAX_SYMBOLS,
        commands::DEFAULT_MAX_TODOS,
    );
    let Ok(index) = index else {
        state.push_error("codemap: failed to load workspace code map index");
        return;
    };
    state.side_panel.code_map = Some(code_map_summary_from_index(&index, false));
    let report = commands::outline_code_map_file(&index, path);
    if report.symbols.is_empty() {
        state.push_transcript(format!(
            "codemap: no indexed symbols for '{path}' (indexed at {})",
            index.generated_at_unix
        ));
        return;
    }
    state.push_transcript(render_code_map_report(
        &report,
        index.generated_at_unix,
        Some(path),
    ));
}

pub(crate) fn handle_code_map_refs(state: &mut TuiState, project_root: &Path, query: &str) {
    let index = commands::load_or_refresh_code_map_index(
        project_root,
        commands::DEFAULT_MAX_SYMBOLS,
        commands::DEFAULT_MAX_TODOS,
    );
    let Ok(index) = index else {
        state.push_error("codemap: failed to load workspace code map index");
        return;
    };
    state.side_panel.code_map = Some(code_map_summary_from_index(&index, false));
    let report = commands::find_code_map_references(project_root, &index, query, 80);
    if report.references.is_empty() {
        state.push_transcript(format!(
            "codemap: no references for '{query}' (indexed at {})",
            index.generated_at_unix
        ));
        return;
    }
    state.push_transcript(render_code_map_report(
        &report,
        index.generated_at_unix,
        Some(query),
    ));
}

pub(crate) fn code_map_summary_from_load(load: &commands::CodeMapIndexLoad) -> CodeMapSummary {
    code_map_summary_from_index(&load.index, load.refreshed)
}

fn code_map_summary_from_index(index: &commands::CodeMapIndex, refreshed: bool) -> CodeMapSummary {
    CodeMapSummary {
        index_exists: true,
        stale: false,
        source_files: index.report.walked_files,
        walked_files: index.report.walked_files,
        symbol_count: index.report.symbols.len(),
        todo_count: index.report.todos.len(),
        generated_at_unix: Some(index.generated_at_unix),
        newest_source_mtime_unix: None,
        refreshed,
    }
}

fn code_map_summary_from_status(status: &commands::CodeMapStatus) -> CodeMapSummary {
    CodeMapSummary {
        index_exists: status.index_exists,
        stale: status.stale,
        source_files: status.source_files,
        walked_files: status.walked_files,
        symbol_count: status.symbol_count,
        todo_count: status.todo_count,
        generated_at_unix: status.generated_at_unix,
        newest_source_mtime_unix: status.newest_source_mtime_unix,
        refreshed: false,
    }
}

fn render_code_map_text(index: &commands::CodeMapIndex) -> String {
    render_code_map_report(&index.report, index.generated_at_unix, None)
}

fn render_code_map_status_text(status: &commands::CodeMapStatus) -> String {
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
    format!(
        "codemap: index {state} (indexed at {generated}, newest source {newest})\nsource files: {} · indexed files: {} · symbols: {} · TODOs: {}{}",
        status.source_files,
        status.walked_files,
        status.symbol_count,
        status.todo_count,
        if status.stale {
            "\nrun /codemap refresh to rebuild the workspace code map index"
        } else {
            ""
        }
    )
}

fn render_code_map_report(
    report: &commands::CodeMapReport,
    generated_at_unix: u64,
    query: Option<&str>,
) -> String {
    let mut body = if let Some(query) = query {
        if report.references.is_empty() {
            format!(
                "codemap: {} symbol match(es), {} TODO match(es) for '{}' across {} file(s) (indexed at {})",
                report.symbols.len(),
                report.todos.len(),
                query,
                report.walked_files,
                generated_at_unix,
            )
        } else {
            format!(
                "codemap: {} reference match(es) for '{}' across {} file(s) (indexed at {})",
                report.references.len(),
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
    if !report.symbols.is_empty() {
        body.push_str("\n\nSymbols:");
        for symbol in report.symbols.iter().take(40) {
            body.push_str(&format!(
                "\n{}:{}  {}  {}",
                symbol.path, symbol.line, symbol.kind, symbol.name
            ));
        }
        if report.symbols.len() > 40 || report.symbols_truncated {
            body.push_str("\n(symbols truncated)");
        }
    }
    if !report.todos.is_empty() {
        body.push_str("\n\nTODOs:");
        for todo in report.todos.iter().take(20) {
            body.push_str(&format!("\n{}:{}  {}", todo.path, todo.line, todo.text));
        }
        if report.todos.len() > 20 || report.todos_truncated {
            body.push_str("\n(TODO markers truncated)");
        }
    }
    if !report.references.is_empty() {
        body.push_str("\n\nReferences:");
        for reference in report.references.iter().take(40) {
            body.push_str(&format!(
                "\n{}:{}  {}  {}",
                reference.path, reference.line, reference.symbol, reference.text
            ));
        }
        if report.references.len() > 40 || report.references_truncated {
            body.push_str("\n(references truncated)");
        }
    }
    body
}
