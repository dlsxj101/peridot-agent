//! Side-panel block renderers (request context, committee, MCP, code
//! map, attachments, notes, goal) split out of the render module.
//! Shared helpers (`format_token_count`, `truncate_display_width`,
//! `short_session_id`) and crate-root imports are reached via
//! `use super::*`.

use super::*;

pub(super) fn render_request_context_block(state: &TuiState) -> String {
    let used = state.side_panel.context_tokens_used;
    let window = state.side_panel.context_tokens_window;
    if used == 0 || window == 0 {
        return String::new();
    }
    let pct = (used as f32 / window as f32 * 100.0).clamp(0.0, 999.0);
    format!(
        "Request context\nused: {} / {} ({pct:.0}%)\nstored: {}  msg: {}\nsys: {}  tools: {}  wire: {}\n\n",
        format_token_count(used),
        format_token_count(window),
        format_token_count(state.side_panel.context_entry_tokens),
        format_token_count(state.side_panel.context_message_tokens),
        format_token_count(state.side_panel.context_system_tokens),
        format_token_count(state.side_panel.context_tool_schema_tokens),
        format_token_count(state.side_panel.context_overhead_tokens),
    )
}

/// Renders the side-panel Committee block as plain text. Shows planner
/// and reviewer cost / token totals so the operator can see how much
/// the multi-LLM orchestration is spending without leaving the chat.
/// Empty when committee is off and neither role has accumulated any
/// cost, so single-agent sessions keep the panel compact.
pub(super) fn render_committee_block(state: &TuiState) -> String {
    let off = matches!(state.committee_mode, peridot_common::CommitteeMode::Off);
    let no_spend = state.committee_planner_cost == 0.0
        && state.committee_reviewer_cost == 0.0
        && state.committee_planner_tokens == 0
        && state.committee_reviewer_tokens == 0;
    if off && no_spend {
        return String::new();
    }
    format!(
        "Committee ({})\nplanner:  ${:.4}  {} tok\nreviewer: ${:.4}  {} tok\n\n",
        state.committee_mode,
        state.committee_planner_cost,
        state.committee_planner_tokens,
        state.committee_reviewer_cost,
        state.committee_reviewer_tokens,
    )
}

pub(super) fn render_mcp_block(state: &TuiState) -> String {
    if state.side_panel.mcp_status.is_empty() {
        return String::new();
    }
    let locale = state.config.language;
    let mut lines = vec![tr(PhraseKey::McpPanelTitle, locale).to_string()];
    for server in state.side_panel.mcp_status.iter().take(5) {
        let status = if server.connected {
            tr(PhraseKey::McpConnected, locale)
        } else {
            tr(PhraseKey::McpDisconnected, locale)
        };
        let transport = server
            .transport
            .as_deref()
            .filter(|transport| !transport.is_empty())
            .map(|transport| format!(" [{transport}]"))
            .unwrap_or_default();
        lines.push(format!(
            "- {}{}: {} tools, {}",
            server.name, transport, server.tool_count, status
        ));
    }
    if state.side_panel.mcp_status.len() > 5 {
        lines.push(format!("... +{}", state.side_panel.mcp_status.len() - 5));
    }
    format!("{}\n\n", lines.join("\n"))
}

pub(super) fn render_code_map_block(state: &TuiState) -> String {
    let Some(summary) = state.side_panel.code_map.as_ref() else {
        return String::new();
    };
    let locale = state.config.language;
    let freshness = if !summary.index_exists {
        tr(PhraseKey::CodeMapMissing, locale)
    } else if summary.stale {
        tr(PhraseKey::CodeMapStale, locale)
    } else {
        tr(PhraseKey::CodeMapFresh, locale)
    };
    let mut lines = vec![
        tr(PhraseKey::CodeMapPanelTitle, locale).to_string(),
        format!(
            "{} · {} sym · {} TODOs",
            freshness, summary.symbol_count, summary.todo_count
        ),
        format!(
            "{} indexed file(s) · {} source file(s)",
            summary.walked_files, summary.source_files
        ),
    ];
    if let Some(generated_at) = summary.generated_at_unix {
        let suffix = if summary.refreshed {
            " (refreshed)"
        } else {
            ""
        };
        lines.push(format!("indexed at {generated_at}{suffix}"));
    }
    if summary.stale
        && let Some(newest) = summary.newest_source_mtime_unix
    {
        lines.push(format!("newest source {newest}"));
    }
    format!("{}\n\n", lines.join("\n"))
}

pub(super) fn render_attachment_block(state: &TuiState) -> String {
    if state.attachment_paths.is_empty() {
        return String::new();
    }
    let locale = state.config.language;
    let mut lines = vec![
        tr(PhraseKey::AttachmentPanelTitle, locale).to_string(),
        format!(
            "{} {}",
            state.attachment_paths.len(),
            tr(PhraseKey::AttachmentFilesAttached, locale)
        ),
    ];
    for path in state.attachment_paths.iter().take(5) {
        lines.push(format!("- {path}"));
    }
    if state.attachment_paths.len() > 5 {
        lines.push(format!(
            "... +{} {}",
            state.attachment_paths.len() - 5,
            tr(PhraseKey::AttachmentMore, locale)
        ));
    }
    format!("{}\n\n", lines.join("\n"))
}

pub(super) fn render_notes_block(state: &TuiState) -> String {
    if state.note_summary.count == 0 {
        return String::new();
    }
    let locale = state.config.language;
    let mut lines = vec![
        tr(PhraseKey::NotesPanelTitle, locale).to_string(),
        format!(
            "{} {}",
            state.note_summary.count,
            tr(PhraseKey::NotesCountSuffix, locale)
        ),
    ];
    if let Some(latest) = state.note_summary.latest.as_deref() {
        lines.push(format!(
            "{}: {}",
            tr(PhraseKey::NotesLatestLabel, locale),
            truncate_display_width(latest, 42)
        ));
    }
    format!("{}\n\n", lines.join("\n"))
}

/// Renders the side-panel Goal block as plain text (joined later into the
/// side panel string). When no goal is active the block collapses to an
/// empty string so the panel doesn't carry a "Goal" header for nothing.
/// Active goals show the objective (truncated to fit narrow panels), a
/// status label, plan progress percentage, and goal age — all of the
/// information the operator needs to glance at without leaving the chat.
pub(super) fn render_goal_block(state: &TuiState) -> String {
    let Some(status) = state.goal_status.as_ref() else {
        return String::new();
    };
    // `/goal clear` sets the status to `Cleared` rather than removing it
    // outright (the agent state machine wants to remember a goal was once
    // active). For the side panel that's noise — once the operator cleared
    // the goal the block should disappear, not linger with `<no
    // objective>`. Treat Cleared as "no goal" for rendering purposes.
    if matches!(status, GoalStatus::Cleared) {
        return String::new();
    }
    let status_label = goal_status_label(Some(status));
    // Objective truncated to fit in a typical 20-30 col side panel without
    // overflowing. The full text is still in `state.goal_text` and surfaced
    // via `/goal status` in the transcript.
    let objective = state
        .goal_text
        .as_deref()
        .map(|text| truncate_objective(text, 28))
        .unwrap_or_else(|| "<no objective>".to_string());
    let total = state.side_panel.plan.len();
    let done = state
        .side_panel
        .plan
        .iter()
        .filter(|step| step.done)
        .count();
    let progress_pct = if total > 0 {
        (done as f32 / total as f32 * 100.0).round() as u32
    } else {
        0
    };
    let bar = render_progress_bar(done, total, 10);
    let age = state
        .goal_started_at_unix
        .map(|started| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(started);
            state::format_duration_ms(now.saturating_sub(started) * 1000)
        })
        .unwrap_or_else(|| "0s".to_string());
    format!(
        "Goal ({status_label})\n{objective}\n{bar} {done}/{total} ({progress_pct}%)\nage: {age}\n\n"
    )
}

/// Truncates an objective string at character (not byte) boundary so CJK
/// inputs don't get mangled. Adds an ellipsis when truncation actually
/// trims something so the operator knows the full text is longer.
fn truncate_objective(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let head: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{head}\u{2026}")
}

/// Builds a Unicode block-character progress bar of the requested width.
/// `done >= total` saturates to a full bar; `total == 0` returns an empty
/// bar so a goal with no steps still renders cleanly.
fn render_progress_bar(done: usize, total: usize, width: usize) -> String {
    if total == 0 || width == 0 {
        return String::new();
    }
    let filled = ((done.min(total)) * width).div_ceil(total);
    let empty = width.saturating_sub(filled);
    format!(
        "[{}{}]",
        "\u{2588}".repeat(filled),
        "\u{2591}".repeat(empty)
    )
}

pub(super) fn should_render_welcome(state: &TuiState) -> bool {
    state.transcript.is_empty()
        && state.active_stream.is_none()
        && state.menu.is_none()
        && state.approval.is_none()
        && state.ask_user.is_none()
}

pub(super) fn render_welcome(state: &TuiState) -> String {
    let workspace = std::env::current_dir()
        .ok()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "<unknown workspace>".to_string());
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "there".to_string());
    format!(
        "Welcome back {user}!\n\n\
         Peridot is ready for an agent run.\n\
         model      {}\n\
         mode       {} · {}\n\
         workspace  {}\n\n\
         Type a task in the input line below and press Enter.\n\n\
         Try\n\
         - fix the failing tests and explain the change\n\
         - create a small utility and add focused tests\n\n\
         Getting started\n\
         1. Type a task  →  Enter to run\n\
         2. Slash commands  →  `/` opens the picker (Tab autocompletes)\n\
         3. Need to stop?  →  Esc interrupts the active run\n\
         4. Multi-line input  →  Ctrl+J (or Alt+Enter) for a newline\n\n\
         Slash commands\n\
         /plan  /execute  /goal <objective>  /safe  /auto  /yolo  /help  /lang en|ko\n\n\
         Keys\n\
         Enter sends  |  Ctrl+J / Alt+Enter newline  |  Esc interrupts/menu  |  Ctrl+P menu  |  Ctrl+] side panel  |  Ctrl-C twice quits",
        state.header.model, state.header.mode, state.header.permission, workspace
    )
}

pub(crate) fn render_subagent_monitor(subagents: &[SubagentMonitorItem]) -> String {
    if subagents.is_empty() {
        return "Subagents\n<none>".to_string();
    }
    let rendered = subagents
        .iter()
        .rev()
        .take(4)
        .rev()
        .map(|subagent| {
            let indent = if subagent.depth == 0 {
                String::new()
            } else {
                let mut s = String::new();
                for _ in 0..subagent.depth.saturating_sub(1) {
                    s.push_str("│  ");
                }
                s.push_str("└─ ");
                s
            };
            let summary = subagent
                .summary
                .as_ref()
                .map(|summary| format!(": {summary}"))
                .unwrap_or_default();
            let mut tail = format!(
                "{indent}{} {} [{}]{}",
                subagent.kind, subagent.task, subagent.status, summary
            );
            if subagent.tokens > 0 {
                tail.push_str(&format!("  ({} tok)", subagent.tokens));
            }
            tail
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("Subagents\n{rendered}")
}

pub(super) fn theme_accent(config: &TuiConfig) -> Color {
    match config.theme.as_str() {
        "light" => Color::Blue,
        "auto" => Color::Cyan,
        _ => Color::Green,
    }
}
