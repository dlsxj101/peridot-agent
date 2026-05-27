use super::*;
use crate::mascot;
use ratatui::style::Modifier;
use state::{TranscriptEntry, TranscriptKind};

/// Minimal header text: `PERIDOT  <model>` — mode/permission/metrics go to the status bar.
pub(super) fn render_header_brief(state: &TuiState) -> String {
    format!("PERIDOT  {}", state.header.model)
}

/// Status-bar metrics text: mode/permission + optional tok/cost/cache + goal + agent.
pub(super) fn render_status_metrics(state: &TuiState) -> String {
    join_status_metric_parts(status_metric_parts(state).iter())
}

pub(super) fn render_status_metrics_for_width(state: &TuiState, max_width: usize) -> String {
    let parts = status_metric_parts(state);
    let full = join_status_metric_parts(parts.iter());
    if string_width(&full) <= max_width {
        return full;
    }

    for max_priority in (0..=3).rev() {
        let compact =
            join_status_metric_parts(parts.iter().filter(|part| part.priority <= max_priority));
        if string_width(&compact) <= max_width {
            return compact;
        }
    }

    truncate_display_width(&parts[0].text, max_width)
}

#[derive(Clone, Debug)]
struct StatusMetricPart {
    text: String,
    priority: u8,
}

impl StatusMetricPart {
    fn new(text: impl Into<String>, priority: u8) -> Self {
        Self {
            text: text.into(),
            priority,
        }
    }
}

fn status_metric_parts(state: &TuiState) -> Vec<StatusMetricPart> {
    let mut parts = vec![StatusMetricPart::new(
        format!("{} · {}", state.header.mode, state.header.permission),
        0,
    )];
    if let Some(workspace) = state.header.workspace_label.as_deref() {
        parts.push(StatusMetricPart::new(format!("workspace {workspace}"), 1));
    }
    if let Some(provider) = state.header.provider.as_deref() {
        parts.push(StatusMetricPart::new(format!("provider {provider}"), 3));
    }
    if state.current_turn > 0 {
        parts.push(StatusMetricPart::new(
            format!("turn {}", state.current_turn),
            1,
        ));
    }
    if state.committee_mode != peridot_common::CommitteeMode::Off {
        parts.push(StatusMetricPart::new(
            format!("committee {}", state.committee_mode),
            1,
        ));
    }
    let active_subagents = state
        .subagents
        .iter()
        .filter(|item| matches!(item.status.as_str(), "running" | "starting"))
        .count();
    if active_subagents > 0 {
        parts.push(StatusMetricPart::new(
            format!("subagents {active_subagents}"),
            2,
        ));
    }
    if state.config.show_token_count {
        parts.push(StatusMetricPart::new(
            format!("{} tok", state.header.total_tokens),
            2,
        ));
    }
    if state.config.show_cost {
        let total_turns = state.current_turn;
        if total_turns > 1 && state.header.cost_usd > 0.0 {
            let avg = state.header.cost_usd / total_turns as f64;
            parts.push(StatusMetricPart::new(
                format!("${:.4} (${avg:.4}/turn)", state.header.cost_usd),
                2,
            ));
        } else {
            parts.push(StatusMetricPart::new(
                format!("${:.4}", state.header.cost_usd),
                2,
            ));
        }
    }
    if let Some(total) = aggregate_usage_status(state) {
        parts.push(StatusMetricPart::new(total, 2));
    }
    if state.config.show_cache_rate {
        parts.push(StatusMetricPart::new(
            format!("cache {:.0}%", state.header.cache_hit_rate * 100.0),
            3,
        ));
    }
    if let Some(status) = state.goal_status.as_ref() {
        parts.push(StatusMetricPart::new(
            format!("goal {}", goal_status_label(Some(status))),
            1,
        ));
    }
    if state.agent_run_status != AgentRunStatus::Idle {
        parts.push(StatusMetricPart::new(
            format!("agent {}", agent_run_status_label(&state.agent_run_status)),
            0,
        ));
    }
    // Surface the elapsed counter once a task has started running. We render
    // it from the same `side_panel.stats.elapsed_seconds` that `tick_spinner`
    // refreshes every frame, so the status bar advances second by second
    // without the host loop having to broadcast tick events.
    if state.task_started_at_unix.is_some() || state.side_panel.stats.elapsed_seconds > 0 {
        // No prefix glyph — the stopwatch emoji `⏱` (U+23F1) is rendered as
        // half-width in many WSL fonts and clipped the digits that followed.
        // Bare digits with the trailing `s` suffix read fine on their own.
        parts.push(StatusMetricPart::new(
            state::format_duration_ms(state.side_panel.stats.elapsed_seconds * 1000),
            0,
        ));
    }
    parts
}

fn join_status_metric_parts<'a>(parts: impl Iterator<Item = &'a StatusMetricPart>) -> String {
    parts
        .map(|part| part.text.as_str())
        .collect::<Vec<_>>()
        .join("  |  ")
}

fn string_width(text: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(text)
}

fn truncate_display_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if string_width(text) <= max_width {
        return text.to_string();
    }

    let mut out = String::new();
    let mut used = 0usize;
    let content_width = max_width.saturating_sub(1);
    for ch in text.chars() {
        let width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + width > content_width {
            break;
        }
        out.push(ch);
        used += width;
    }
    out.push('\u{2026}');
    out
}

fn aggregate_usage_status(state: &TuiState) -> Option<String> {
    if state.sessions.len() <= 1 {
        return None;
    }

    let mut metrics = Vec::new();
    if state.config.show_token_count {
        let total_tokens = state.aggregate_tokens();
        if total_tokens > state.header.total_tokens {
            metrics.push(format!("{} tok", format_status_token_count(total_tokens)));
        }
    }
    if state.config.show_cost {
        let total_cost = state.aggregate_cost_usd();
        if total_cost > state.header.cost_usd + f64::EPSILON {
            metrics.push(format!("${total_cost:.4}"));
        }
    }

    if metrics.is_empty() {
        None
    } else {
        Some(format!("all {}", metrics.join(" / ")))
    }
}

fn format_status_token_count(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 10_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

pub(super) fn agent_run_status_label(status: &AgentRunStatus) -> &'static str {
    match status {
        AgentRunStatus::Idle => "idle",
        AgentRunStatus::Running => "running",
        AgentRunStatus::Succeeded => "done",
        AgentRunStatus::Failed => "failed",
        AgentRunStatus::WaitingApproval => "waiting-approval",
        AgentRunStatus::Interrupted => "interrupted",
    }
}

pub(super) fn goal_status_label(status: Option<&GoalStatus>) -> &'static str {
    match status {
        Some(GoalStatus::Running) => "running",
        Some(GoalStatus::Paused) => "paused",
        Some(GoalStatus::Done) => "done",
        Some(GoalStatus::Cleared) => "cleared",
        None => "inactive",
    }
}

pub(super) fn activity_kind_label(kind: &ActivityKind) -> &'static str {
    match kind {
        ActivityKind::Stream => "stream",
        ActivityKind::Tool => "tool",
        ActivityKind::Subagent => "subagent",
        ActivityKind::Verification => "verify",
    }
}

/// Trims a long session id (`session-628850-1778945666`) down to its trailing
/// timestamp chunk for the compact `Status` panel. The full id is still saved
/// to disk and surfaced in `/info`; this is purely cosmetic.
fn short_session_id(id: &str) -> String {
    id.rsplit_once('-')
        .map(|(_, tail)| tail.to_string())
        .unwrap_or_else(|| id.to_string())
}

fn render_request_context_block(state: &TuiState) -> String {
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
fn render_committee_block(state: &TuiState) -> String {
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

/// Renders the side-panel Goal block as plain text (joined later into the
/// side panel string). When no goal is active the block collapses to an
/// empty string so the panel doesn't carry a "Goal" header for nothing.
/// Active goals show the objective (truncated to fit narrow panels), a
/// status label, plan progress percentage, and goal age — all of the
/// information the operator needs to glance at without leaving the chat.
fn render_goal_block(state: &TuiState) -> String {
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

pub(super) fn render_subagent_monitor(subagents: &[SubagentMonitorItem]) -> String {
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

/// Renders a sticky one-to-three-line plan banner shown above the transcript.
/// Returns an empty vector when no plan is active.
fn sticky_plan_banner(state: &TuiState) -> Vec<Line<'static>> {
    if state.side_panel.plan.is_empty() {
        return Vec::new();
    }
    let total = state.side_panel.plan.len();
    let done = state
        .side_panel
        .plan
        .iter()
        .filter(|step| step.done)
        .count();
    let current = state
        .side_panel
        .plan
        .iter()
        .find(|step| !step.done)
        .map(|step| step.label.as_str())
        .unwrap_or("complete");
    let upcoming = state
        .side_panel
        .plan
        .iter()
        .filter(|step| !step.done)
        .nth(1)
        .map(|step| step.label.as_str());

    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("Plan ({done}/{total})  "),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("\u{25B6} {current}"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
    ])];
    if let Some(next) = upcoming
        && state.layout != LayoutMode::Minimal
    {
        lines.push(Line::from(Span::styled(
            format!("    next  {next}"),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )));
    }
    lines.push(Line::from(""));
    lines
}

/// Returns true when the entry should be hidden in normal (non-debug) view.
fn is_entry_hidden(state: &TuiState, entry: &TranscriptEntry) -> bool {
    match entry.kind {
        TranscriptKind::Debug => !state.debug_view,
        TranscriptKind::Thinking => !(state.debug_view || state.config.show_thinking),
        _ => false,
    }
}

/// Returns true when the entry should be hidden from the live TUI transcript
/// pane. The chat view is intentionally minimal — only the back-and-forth
/// conversation belongs there: `User` and `Assistant` text for the message
/// itself, `ToolOk` / `ToolFail` for the tool actions taken between turns,
/// and `Error` for real failures the operator must see. Everything else
/// (system bookkeeping, queue notices, turn separators, pre-tool-run
/// Builds the set of transcript entry indices that should be hidden because
/// their enclosing block is collapsed.
fn build_collapsed_set(
    state: &TuiState,
    blocks: &[crate::blocks::TranscriptBlock],
) -> std::collections::HashSet<usize> {
    let mut hidden = std::collections::HashSet::new();
    for block in blocks {
        let is_collapsed = state.collapse_all_tool_blocks
            || state.collapsed_blocks.contains(&block.header_index)
            || block.body_line_count > state.collapse_threshold;
        if is_collapsed {
            for idx in block.body_range.clone() {
                hidden.insert(idx);
            }
        }
    }
    hidden
}

/// For each collapsed block, maps the header index to a `[+N lines]` indicator
/// string that the render loop appends after the header entry.
fn build_collapse_indicators(
    blocks: &[crate::blocks::TranscriptBlock],
    collapsed: &std::collections::HashSet<usize>,
) -> std::collections::HashMap<usize, String> {
    let mut indicators = std::collections::HashMap::new();
    for block in blocks {
        if block.body_range.is_empty() {
            continue;
        }
        let any_hidden = block.body_range.clone().any(|i| collapsed.contains(&i));
        if any_hidden {
            indicators.insert(
                block.header_index,
                format!("  [+{} lines · Ctrl+E to expand]", block.body_line_count),
            );
        }
    }
    indicators
}

/// preambles, thinking/debug toggles) is meta-information and would just
/// clutter the chat. The text snapshot used by tests keeps the looser
/// [`is_entry_hidden`] filter so existing assertions on those entries stay
/// valid.
fn is_entry_hidden_in_chat(state: &TuiState, entry: &TranscriptEntry) -> bool {
    if is_entry_hidden(state, entry) {
        return true;
    }
    // Indented tool preview lines (`  path: ...`, `  preview: ...`, file
    // contents, diff bodies) are pushed alongside the tool's main summary
    // line by `record_tool_started` / `record_tool_result`. Claude Code and
    // Codex CLI both collapse this detail by default — the chat shows
    // `✔ file_read  read 1234 bytes` and the model still sees the full body
    // through its tool-result context, so the preview lines are pure noise
    // in the visible transcript. Keep them in the underlying transcript so
    // the text snapshot used by tests stays unchanged.
    let is_indented_tool_detail = matches!(
        entry.kind,
        TranscriptKind::ToolStart | TranscriptKind::ToolOk | TranscriptKind::ToolFail
    ) && entry.text.starts_with("  ");
    if is_indented_tool_detail && !state.debug_view {
        return true;
    }
    match entry.kind {
        TranscriptKind::Meta | TranscriptKind::ToolStart => !state.debug_view,
        TranscriptKind::TurnSeparator => false,
        _ => false,
    }
}

/// Builds styled lines for one transcript entry. The transcript is rendered as
/// a flat inline chat (Claude Code / Codex CLI style): assistant text has no
/// prefix, user input carries a subtle `> ` quote, and tool results compress
/// to a single colored glyph followed by their summary. Multi-line entries
/// expand on `\n` so every line wraps independently in the outer `Paragraph`.
fn style_transcript_entry(state: &TuiState, entry: &TranscriptEntry) -> Vec<Line<'static>> {
    let is_indented_detail = matches!(
        entry.kind,
        TranscriptKind::ToolStart | TranscriptKind::ToolOk | TranscriptKind::ToolFail
    ) && entry.text.starts_with("  ");
    if is_indented_detail {
        let style = Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM);
        return entry
            .text
            .lines()
            .map(|line| Line::from(Span::styled(line.to_string(), style)))
            .collect();
    }
    match entry.kind {
        TranscriptKind::User => render_user_block(&entry.text),
        TranscriptKind::Assistant => render_assistant_block(&entry.text, &state.config),
        TranscriptKind::ToolStart => render_prefixed_block(
            &entry.text,
            "\u{276F} ",
            Style::default().fg(Color::DarkGray),
            Style::default().fg(Color::DarkGray),
        ),
        TranscriptKind::ToolOk => render_prefixed_block(
            &entry.text,
            "\u{2714} ",
            Style::default().fg(Color::Green),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
        TranscriptKind::ToolFail => render_prefixed_block(
            &entry.text,
            "\u{2718} ",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            Style::default().fg(Color::Red),
        ),
        TranscriptKind::System => entry
            .text
            .lines()
            .map(|line| {
                Line::from(Span::styled(
                    line.to_string(),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ))
            })
            .collect(),
        TranscriptKind::Notice => render_prefixed_block(
            &entry.text,
            "\u{26A0} ",
            Style::default().fg(Color::Yellow),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::DIM),
        ),
        TranscriptKind::Error => render_prefixed_block(
            &entry.text,
            "\u{26A0} ",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        TranscriptKind::Debug => entry
            .text
            .lines()
            .map(|line| {
                Line::from(Span::styled(
                    line.to_string(),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ))
            })
            .collect(),
        TranscriptKind::Thinking => render_prefixed_block(
            &entry.text,
            "\u{2026} ",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM | Modifier::ITALIC),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM | Modifier::ITALIC),
        ),
        TranscriptKind::TurnSeparator => vec![Line::from(Span::styled(
            "\u{2500}".repeat(40),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ))],
        TranscriptKind::Meta => entry
            .text
            .lines()
            .map(|line| {
                Line::from(Span::styled(
                    line.to_string(),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ))
            })
            .collect(),
        TranscriptKind::Diff => entry
            .text
            .lines()
            .map(|line| Line::from(style_diff_line(line)))
            .collect(),
    }
}

/// Colours one `- foo` / `+ foo` / `... N more lines` diff line. We pick the
/// style by the line's leading marker — Vec<Span> is returned so wrapped
/// continuation rows keep the same colour without redrawing the marker.
fn style_diff_line(line: &str) -> Vec<Span<'static>> {
    let (style, marker, body) = if let Some(rest) = line.strip_prefix("- ") {
        (Style::default().fg(Color::Red), "- ", rest)
    } else if let Some(rest) = line.strip_prefix("+ ") {
        (Style::default().fg(Color::Green), "+ ", rest)
    } else {
        return vec![Span::styled(
            line.to_string(),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )];
    };
    vec![
        Span::styled(marker.to_string(), style.add_modifier(Modifier::BOLD)),
        Span::styled(body.to_string(), style),
    ]
}

/// User input: subtle `> ` quote prefix in cyan, content in white. Multi-line
/// quotes keep the prefix only on the first row and indent continuation lines
/// underneath the glyph so the quote reads as one block.
fn render_user_block(text: &str) -> Vec<Line<'static>> {
    let prefix_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let body_style = Style::default().fg(Color::Cyan);
    let indent = "  "; // two spaces, matching `> `'s width
    let mut lines = Vec::new();
    let mut iter = text.lines();
    if let Some(first) = iter.next() {
        let mut spans = vec![Span::styled("> ".to_string(), prefix_style)];
        spans.extend(style_markdown_inline(first, body_style));
        lines.push(Line::from(spans));
    }
    for rest in iter {
        let mut spans = vec![Span::raw(indent.to_string())];
        spans.extend(style_markdown_inline(rest, body_style));
        lines.push(Line::from(spans));
    }
    lines
}

/// Assistant message renderer. Handles three markdown shapes beyond the
/// inline bold/code styler: triple-backtick code fences, pipe tables, and
/// everything else (delegated to `style_markdown_inline`).
fn render_assistant_block(text: &str, _config: &TuiConfig) -> Vec<Line<'static>> {
    let body_style = Style::default().fg(Color::White);
    let code_style = Style::default().fg(Color::Rgb(180, 220, 255));
    let fence_style = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::DIM);
    let mut lines = Vec::new();
    let mut in_fence = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            let label = trimmed.trim_start_matches("```").trim();
            let rule = if in_fence {
                "\u{2514}\u{2500} code".to_string()
            } else if label.is_empty() {
                "\u{250C}\u{2500} code".to_string()
            } else {
                format!("\u{250C}\u{2500} code ({label})")
            };
            lines.push(Line::from(Span::styled(rule, fence_style)));
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            lines.push(Line::from(Span::styled(line.to_string(), code_style)));
            continue;
        }
        if is_table_row(line) {
            lines.push(Line::from(style_table_row(line, body_style)));
            continue;
        }
        lines.push(Line::from(style_markdown_inline(line, body_style)));
    }
    lines
}

/// Returns true when `line` looks like a markdown pipe-table row — starts
/// with `|` after trimming and carries at least two more `|` separators so
/// `|x` (a non-table use of pipes) doesn't trigger.
fn is_table_row(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|') && trimmed.matches('|').count() >= 2
}

/// Returns true for the separator row that follows a markdown pipe-table
/// header, e.g. `| --- | :--: | ---: |`. Only `|`, `-`, `:`, and spaces are
/// allowed; the row also needs at least three dashes so we don't mistake a
/// data row that happens to be `|-|-|` for a separator.
fn is_table_separator(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) && trimmed.matches('-').count() >= 3
}

/// Replaces the cell separators in a pipe-table row with `│` (data row) or
/// `┼─` rules (separator row). Cell contents flow through unchanged at
/// `base_style`; the divider columns use a dim DarkGray so they recede.
fn style_table_row(line: &str, base_style: Style) -> Vec<Span<'static>> {
    let border = Style::default().fg(Color::DarkGray);
    let trimmed = line.trim_end_matches(&[' ', '\t']);
    if is_table_separator(line) {
        let rule = trimmed
            .chars()
            .map(|c| match c {
                '|' => '\u{253C}',
                '-' | ':' => '\u{2500}',
                other => other,
            })
            .collect::<String>();
        return vec![Span::styled(rule, border.add_modifier(Modifier::DIM))];
    }
    let parts: Vec<&str> = trimmed.split('|').collect();
    let last_index = parts.len().saturating_sub(1);
    let mut spans = Vec::new();
    for (idx, cell) in parts.iter().enumerate() {
        let is_edge = idx == 0 || idx == last_index;
        if is_edge && cell.is_empty() {
            spans.push(Span::styled("\u{2502}".to_string(), border));
            continue;
        }
        spans.push(Span::styled(cell.to_string(), base_style));
        if idx < last_index {
            spans.push(Span::styled("\u{2502}".to_string(), border));
        }
    }
    spans
}

/// Lightweight inline markdown styling for one line of text. Recognises
/// `**bold**` and `` `code` `` segments and applies appropriate emphasis on top of
/// the supplied base style. Anything we do not recognise is passed through verbatim
/// using the base style, so unsupported markdown is never lost — it just renders
/// flat. The parser is intentionally simple (no nesting, no escapes) so it stays
/// predictable on streaming partial content.
fn style_markdown_inline(text: &str, base_style: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let bytes = text.as_bytes();
    let mut idx = 0;
    let mut plain_start = 0;
    let flush_plain = |start: usize, end: usize, out: &mut Vec<Span<'static>>| {
        if end > start {
            out.push(Span::styled(text[start..end].to_string(), base_style));
        }
    };
    while idx < bytes.len() {
        if bytes[idx] == b'*'
            && idx + 1 < bytes.len()
            && bytes[idx + 1] == b'*'
            && let Some(close) = find_marker(text, idx + 2, "**")
        {
            flush_plain(plain_start, idx, &mut spans);
            let segment = &text[idx + 2..close];
            spans.push(Span::styled(
                segment.to_string(),
                base_style.add_modifier(Modifier::BOLD),
            ));
            idx = close + 2;
            plain_start = idx;
            continue;
        }
        if bytes[idx] == b'`'
            && let Some(close) = find_marker(text, idx + 1, "`")
        {
            flush_plain(plain_start, idx, &mut spans);
            let segment = &text[idx + 1..close];
            spans.push(Span::styled(
                segment.to_string(),
                Style::default()
                    .fg(Color::Rgb(180, 220, 255))
                    .add_modifier(Modifier::BOLD),
            ));
            idx = close + 1;
            plain_start = idx;
            continue;
        }
        idx += 1;
    }
    flush_plain(plain_start, bytes.len(), &mut spans);
    if spans.is_empty() {
        spans.push(Span::styled(String::new(), base_style));
    }
    spans
}

/// Returns the byte index where `marker` next appears in `text` starting at
/// `from`, or `None` if the marker is not found. Used by the markdown inline
/// styler to close `**bold**` / `` `code` `` segments.
fn find_marker(text: &str, from: usize, marker: &str) -> Option<usize> {
    if from > text.len() {
        return None;
    }
    text[from..].find(marker).map(|offset| from + offset)
}

/// Renders a glyph-prefixed block where every line carries the prefix's column and
/// continuation lines are indented to match the leading glyph. Used for tool, notice,
/// error, and thinking entries.
fn render_prefixed_block(
    text: &str,
    glyph: &str,
    glyph_style: Style,
    body_style: Style,
) -> Vec<Line<'static>> {
    let indent = " ".repeat(glyph.chars().count());
    let mut lines = Vec::new();
    let mut iter = text.lines();
    if let Some(first) = iter.next() {
        lines.push(Line::from(vec![
            Span::styled(glyph.to_string(), glyph_style),
            Span::styled(first.to_string(), body_style),
        ]));
    } else {
        lines.push(Line::from(Span::styled(glyph.to_string(), glyph_style)));
    }
    for rest in iter {
        lines.push(Line::from(vec![
            Span::raw(indent.clone()),
            Span::styled(rest.to_string(), body_style),
        ]));
    }
    lines
}

/// Builds the live in-progress agent reply as inline lines. The braille
/// spinner sits on the first line so the user can see the model is still
/// generating; subsequent lines render plain. When nothing has streamed yet
/// we pick a placeholder that matches the actual underlying state — there
/// is no point claiming the model is "thinking" if it is sitting on the
/// network roundtrip or running a tool. The streaming bubble itself is
/// suppressed by `draw()` while a tool is executing (active_stream is
/// `None` during that phase), so this placeholder only fires during the
/// "waiting for the model's first token" window.
fn render_streaming_inline(state: &TuiState, stream: &StreamState) -> Vec<Line<'static>> {
    let body_style = Style::default().fg(Color::White);
    let spinner_style = Style::default().fg(Color::Cyan);
    let content = stream.content.trim();
    if content.is_empty() {
        let placeholder = streaming_placeholder_label(state);
        return vec![Line::from(vec![
            Span::styled(format!("{} ", state.spinner_frame()), spinner_style),
            Span::styled(
                placeholder.to_string(),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC | Modifier::DIM),
            ),
        ])];
    }
    let mut lines = Vec::new();
    let mut iter = content.lines();
    if let Some(first) = iter.next() {
        let mut spans = vec![Span::styled(
            format!("{} ", state.spinner_frame()),
            spinner_style,
        )];
        spans.extend(style_markdown_inline(first, body_style));
        lines.push(Line::from(spans));
    }
    for rest in iter {
        lines.push(Line::from(style_markdown_inline(rest, body_style)));
    }
    lines
}

/// Picks the dim italic line shown in the streaming bubble before the model
/// emits its first delta. We avoid the historical "thinking..." string
/// because it implies reasoning content — the placeholder fires whenever
/// the stream is empty (model warmup, network latency, queue waits), and
/// most of that time is not reasoning at all. When a tool happens to be
/// active (rare with our current draw rules), the status bar already names
/// the tool, so we fall through to a generic label here.
fn streaming_placeholder_label(state: &TuiState) -> &'static str {
    if !state.active_tools.is_empty() {
        "running tool…"
    } else {
        "generating reply…"
    }
}

/// Human-readable description of the current agent activity.
pub(super) fn agent_status_summary(state: &TuiState) -> String {
    let locale = state.config.language;
    if state.ask_user.is_some() {
        return tr(PhraseKey::StatusWaitingUser, locale).to_string();
    }
    if state.approval.is_some() || state.agent_run_status == AgentRunStatus::WaitingApproval {
        return tr(PhraseKey::StatusWaitingApproval, locale).to_string();
    }
    if !state.active_tools.is_empty() {
        let names = state.active_tools.join(", ");
        return format!("{} {names}", tr(PhraseKey::StatusToolRunning, locale));
    }
    if state.active_stream.is_some() {
        // Don't prepend a spinner here — `render_status_bar` already inserts one
        // before this string for any busy state, so duplicating would render
        // `● ⠴ ⠴ processing...`.
        return tr(PhraseKey::StatusProcessing, locale).to_string();
    }
    match state.agent_run_status {
        AgentRunStatus::Idle => tr(PhraseKey::StatusIdle, locale).to_string(),
        AgentRunStatus::Running => tr(PhraseKey::StatusProcessing, locale).to_string(),
        AgentRunStatus::Succeeded => tr(PhraseKey::StatusDone, locale).to_string(),
        AgentRunStatus::Failed => tr(PhraseKey::StatusFailed, locale).to_string(),
        AgentRunStatus::WaitingApproval => tr(PhraseKey::StatusWaitingApproval, locale).to_string(),
        AgentRunStatus::Interrupted => tr(PhraseKey::StatusInterrupted, locale).to_string(),
    }
}

/// Picks the status-bar mood glyph + color from the same state machine
/// that drives the deer mascot, so the 1-cell indicator on the left of
/// the status bar always tracks the mascot's current emotion. Using a
/// distinct glyph per mood (not just a recoloured `●`) gives the operator
/// a second visual channel — useful when the terminal palette is muted
/// or when the colour itself is hard to distinguish.
fn mood_indicator(state: &TuiState) -> (&'static str, Style) {
    use crate::mascot::{MascotState, mascot_state_from};
    match mascot_state_from(state) {
        // ◔ quarter-fill — quietly waiting, low energy.
        MascotState::Idle => (
            "\u{25D4}",
            Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
        ),
        // ◑ half-fill — thinking / streaming.
        MascotState::Thinking => (
            "\u{25D1}",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        // ◉ targeted dot — focused tool execution.
        MascotState::ToolRunning => (
            "\u{25C9}",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        // ◕ three-quarter — waiting on operator decision.
        MascotState::ApprovalWaiting => ("\u{25D5}", Style::default().fg(Color::Rgb(255, 165, 0))),
        MascotState::AskUser => ("\u{25D4}", Style::default().fg(Color::Magenta)),
        // ◉ filled bullseye — task completed.
        MascotState::Done => (
            "\u{25C9}",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        // ◓ bottom-fill — failure (ears down).
        MascotState::Failed => (
            "\u{25D3}",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        MascotState::Interrupted => (
            "\u{25D4}",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ),
    }
}

/// Renders a 1-line agent status bar (icon, label, queue depth, metrics).
fn render_status_bar(state: &TuiState, width: u16) -> Line<'static> {
    let (icon, icon_style) = mood_indicator(state);

    let mut spans = vec![Span::styled(format!("{icon} "), icon_style)];
    let busy = state.is_agent_busy() || !state.active_tools.is_empty();
    if busy {
        spans.push(Span::styled(
            format!("{} ", state.spinner_frame()),
            Style::default().fg(Color::Cyan),
        ));
    }
    spans.push(Span::raw(agent_status_summary(state)));
    if !state.input_queue.is_empty() {
        spans.push(Span::styled(
            format!(
                "  | {} {}",
                tr(PhraseKey::StatusQueueSuffix, state.config.language),
                state.input_queue.len()
            ),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::DIM),
        ));
    }
    let pending_attention = state.pending_attention_count();
    if pending_attention > 0 {
        spans.push(Span::styled(
            format!(
                "  | \u{26A0} {}{}",
                pending_attention,
                tr(
                    PhraseKey::StatusSessionsAttentionSuffix,
                    state.config.language
                ),
            ),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }
    // Request-context gauge — this is the estimated provider request
    // footprint, not just the stored conversation entries. It includes
    // system prompt, messages, tool schemas and protocol overhead.
    let ctx_used = state.side_panel.context_tokens_used;
    let ctx_window = state.side_panel.context_tokens_window;
    if ctx_used > 0 && ctx_window > 0 {
        let pct = (ctx_used as f32 / ctx_window as f32 * 100.0).clamp(0.0, 999.0);
        let style = if pct >= 90.0 {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        } else if pct >= 75.0 {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM)
        };
        spans.push(Span::styled(
            format!(
                "  · req {}/{} ({pct:.0}%)",
                format_token_count(ctx_used),
                format_token_count(ctx_window),
            ),
            style,
        ));
    }
    let metrics_prefix = "  · ";
    let occupied_width = spans
        .iter()
        .map(|span| string_width(span.content.as_ref()))
        .sum::<usize>();
    let metrics_width = usize::from(width).saturating_sub(occupied_width + metrics_prefix.len());
    let metrics = render_status_metrics_for_width(state, metrics_width);
    if !metrics.is_empty() {
        spans.push(Span::styled(
            format!("{metrics_prefix}{metrics}"),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ));
    }
    Line::from(spans)
}

/// Renders a token count in compact form — `48000` becomes `48k`,
/// `1048576` becomes `1.0M`. Below 1000 the raw number is shown so
/// small contexts don't lose precision.
fn format_token_count(tokens: usize) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{}k", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

/// Renders a deterministic text snapshot for tests and headless previews.
pub fn render_text_snapshot(state: &TuiState) -> String {
    let mut output = String::new();
    let _ = writeln!(output, "{}", render_header_brief(state));
    let _ = writeln!(output, "metrics: {}", render_status_metrics(state));
    if !state.sessions.is_empty() {
        let _ = writeln!(output, "tabs: {}", crate::render_tab_bar_text(state));
    }
    let pending_attention = state.pending_attention_count();
    if pending_attention > 0 {
        let _ = writeln!(
            output,
            "attention: {pending_attention}{}",
            tr(
                PhraseKey::StatusSessionsAttentionSuffix,
                state.config.language
            )
        );
    }
    let _ = writeln!(output, "layout: {:?}", state.layout);
    let _ = writeln!(output);
    if should_render_welcome(state) {
        let _ = writeln!(output, "{}", render_welcome(state));
    } else {
        if !state.side_panel.plan.is_empty() {
            let total = state.side_panel.plan.len();
            let done = state
                .side_panel
                .plan
                .iter()
                .filter(|step| step.done)
                .count();
            let current = state
                .side_panel
                .plan
                .iter()
                .find(|step| !step.done)
                .map(|step| step.label.as_str())
                .unwrap_or("complete");
            let _ = writeln!(output, "banner: Plan ({done}/{total}) > {current}");
        }
        let visible = state
            .transcript
            .iter()
            .filter(|entry| !is_entry_hidden(state, entry))
            .collect::<Vec<_>>();
        for entry in visible.iter().rev().take(20).rev() {
            let _ = writeln!(output, "{}", entry.text);
        }
    }
    if let Some(stream) = &state.active_stream
        && !stream.content.is_empty()
    {
        let _ = writeln!(output, "stream: {}", stream.content);
    }
    let _ = writeln!(output, "status: {}", agent_status_summary(state));
    if state.config.show_mascot {
        let _ = writeln!(output, "mascot: {}", mascot::mascot_text_summary(state));
    }
    // Diagnostic snapshot always emits plan/activity/subagent data — the
    // live `show_subagent_panel` toggle only affects the on-screen UI; a
    // headless preview needs the full state regardless of cosmetic flags.
    if state.layout == LayoutMode::Full {
        let done = state
            .side_panel
            .plan
            .iter()
            .filter(|step| step.done)
            .count();
        let _ = writeln!(output);
        if state.goal_status.is_some() {
            let _ = writeln!(
                output,
                "Goal status: {}",
                goal_status_label(state.goal_status.as_ref())
            );
        }
        let _ = writeln!(output, "Plan {done}/{}", state.side_panel.plan.len());
        for step in &state.side_panel.plan {
            let marker = if step.done { "[x]" } else { "[ ]" };
            let _ = writeln!(output, "{marker} {}", step.label);
        }
        let _ = writeln!(
            output,
            "Session steps={} errors={} elapsed={}s",
            state.side_panel.stats.steps,
            state.side_panel.stats.errors,
            state.side_panel.stats.elapsed_seconds
        );
        if state.side_panel.context_tokens_used > 0 && state.side_panel.context_tokens_window > 0 {
            let _ = writeln!(
                output,
                "Request context: {}/{} (stored={} msg={} sys={} tools={} overhead={})",
                state.side_panel.context_tokens_used,
                state.side_panel.context_tokens_window,
                state.side_panel.context_entry_tokens,
                state.side_panel.context_message_tokens,
                state.side_panel.context_system_tokens,
                state.side_panel.context_tool_schema_tokens,
                state.side_panel.context_overhead_tokens,
            );
        }
        if state.agent_run_status != AgentRunStatus::Idle {
            let _ = writeln!(
                output,
                "Agent status: {}",
                agent_run_status_label(&state.agent_run_status)
            );
        }
        if !state.activities.is_empty() {
            let _ = writeln!(output, "Activity");
            for activity in &state.activities {
                let _ = writeln!(
                    output,
                    "- {} {}: {}",
                    activity_kind_label(&activity.kind),
                    activity.label,
                    activity.status
                );
            }
        }
        if !state.subagents.is_empty() {
            let _ = writeln!(output, "Subagents");
            for subagent in &state.subagents {
                let summary = subagent
                    .summary
                    .as_ref()
                    .map(|summary| format!(": {summary}"))
                    .unwrap_or_default();
                let _ = writeln!(
                    output,
                    "- {} {} [{}]{}",
                    subagent.kind, subagent.task, subagent.status, summary
                );
            }
        }
    }
    let _ = write!(output, "> {}", state.input);
    output
}

/// Draws the TUI state with Ratatui.
pub fn draw(frame: &mut Frame<'_>, state: &TuiState) {
    let area = frame.area();
    let tab_height = crate::tab_bar_height(state);
    // Dynamic input height: grow with each `\n` so multi-line drafts are
    // visible in the box (not just spaces). Floor 3 (one content row +
    // top/bottom border), ceiling 10 so a long draft can't swallow the
    // transcript. When content exceeds the visible window the cursor is
    // kept visible by scrolling the inner Paragraph below.
    let input_rows = state.input.split('\n').count() as u16;
    let input_height = (input_rows + 2).clamp(3, 10);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(tab_height),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(input_height),
        ])
        .split(area);

    // Header packs the identity (PERIDOT + model) plus the most useful
    // bits the side panel used to carry — session, steps, elapsed,
    // subagent count — so the operator can run with the side panel
    // toggled off (default) and still see what the deer is up to.
    let mut header_spans = vec![
        Span::styled(
            "PERIDOT",
            Style::default()
                .fg(theme_accent(&state.config))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(state.header.model.clone(), Style::default().fg(Color::Gray)),
    ];
    let dim = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::DIM);
    let mut push_dim = |label: String| {
        header_spans.push(Span::styled(format!(" · {label}"), dim));
    };
    if !state.current_session_id.is_empty() {
        push_dim(format!(
            "session {}",
            short_session_id(&state.current_session_id)
        ));
    }
    if state.side_panel.stats.steps > 0 {
        push_dim(format!("steps {}", state.side_panel.stats.steps));
    }
    if state.task_started_at_unix.is_some() || state.side_panel.stats.elapsed_seconds > 0 {
        push_dim(state::format_duration_ms(
            state.side_panel.stats.elapsed_seconds * 1000,
        ));
    }
    let active_subagents = state
        .subagents
        .iter()
        .filter(|item| matches!(item.status.as_str(), "running" | "starting"))
        .count();
    if active_subagents > 0 || !state.config.show_subagent_panel {
        push_dim(format!("subagents {active_subagents}"));
    }
    if let Some(version) = state.header.update_available.as_ref() {
        header_spans.push(Span::styled(
            format!(" · update {version} :update"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::DIM),
        ));
    }
    let header = Paragraph::new(Line::from(header_spans));
    frame.render_widget(header, chunks[0]);

    if tab_height > 0 {
        frame.render_widget(Paragraph::new(crate::render_tab_bar(state)), chunks[1]);
    }

    let body_chunks = if state.layout == LayoutMode::Full && state.config.show_subagent_panel {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(75), Constraint::Percentage(25)])
            .split(chunks[2])
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(100)])
            .split(chunks[2])
    };
    // Overlays (menu, approval, branch picker, ask_user) keep the bordered
    // box look — they are modal popovers and benefit from a clear frame.
    // The transcript itself and the welcome splash run borderless so
    // (a) drag-selection grabs only chat content and (b) red ToolFail /
    // Error spans can't leak SGR into adjacent border cells (ratatui 0.30
    // doesn't emit a reset between cells with `fg=None`).
    let overlay_block = || {
        Block::default()
            .title(body_title(state))
            .borders(Borders::ALL)
    };
    if let Some(menu) = &state.menu {
        frame.render_widget(
            Paragraph::new(render_menu(menu))
                .block(overlay_block())
                .wrap(Wrap { trim: false }),
            body_chunks[0],
        );
    } else if let Some(panel) = &state.approval {
        frame.render_widget(
            Paragraph::new(render_approval_panel(panel))
                .block(overlay_block())
                .wrap(Wrap { trim: false }),
            body_chunks[0],
        );
    } else if let Some(picker) = &state.session_picker {
        frame.render_widget(
            Paragraph::new(render_session_picker(
                picker,
                &state.sessions,
                &state.current_session_id,
            ))
            .block(overlay_block())
            .wrap(Wrap { trim: false }),
            body_chunks[0],
        );
    } else if let Some(picker) = &state.branch_picker {
        frame.render_widget(
            Paragraph::new(render_branch_picker(picker))
                .block(overlay_block())
                .wrap(Wrap { trim: false }),
            body_chunks[0],
        );
    } else if let Some(panel) = &state.ask_user {
        frame.render_widget(
            Paragraph::new(render_ask_user_panel(panel))
                .block(overlay_block())
                .wrap(Wrap { trim: false }),
            body_chunks[0],
        );
    } else if should_render_welcome(state) {
        // Welcome splash: render the full 8×4 mascot in the upper-left and
        // place the welcome text to its right when the pane is wide enough.
        // On very narrow terminals (<32 cols) the mascot is skipped so the
        // text stays readable.
        let area = body_chunks[0];
        let mascot_block_w: u16 = 10; // 8 sprite cells + 2 col gap
        let show_mascot_here = state.config.show_mascot && area.width >= 32 && area.height >= 6;
        if show_mascot_here {
            let mascot_area = Rect {
                x: area.x + 1,
                y: area.y + 1,
                width: 8,
                height: 4,
            };
            render_mascot(state, mascot_area, frame.buffer_mut());
        }
        let text_x_offset: u16 = if show_mascot_here {
            mascot_block_w + 1
        } else {
            2
        };
        let text_area = Rect {
            x: area.x + text_x_offset.min(area.width.saturating_sub(1)),
            y: area.y,
            width: area.width.saturating_sub(text_x_offset + 1),
            height: area.height,
        };
        frame.render_widget(
            Paragraph::new(render_welcome(state)).wrap(Wrap { trim: false }),
            text_area,
        );
    } else {
        // Inline chat transcript (Claude Code / Codex CLI style): every visible
        // entry is rendered as a flat line stream — no bordered bubbles, no
        // speaker columns, just a top-to-bottom flow with subtle glyph cues.
        // Wrapping is handled by `Wrap { trim: false }`, and the viewport
        // window is computed wrap-aware via `Paragraph::line_count` + `.scroll`
        // so the tail of the agent's last reply never gets clipped just
        // because a line wrapped one row beyond our naive line-count estimate.
        //
        // No surrounding `Block` — instead we shrink the rect by one column
        // on each side for a touch of breathing room. This is what makes the
        // transcript copy-friendly: a terminal drag-select grabs only the
        // text cells, not Unicode `│` border characters.
        let body_area = body_chunks[0];
        // When the operator runs with the side panel toggled off, the deer
        // mascot floats in the top-right corner of the body area and the
        // transcript starts five rows below the body top (1 title row +
        // 4 mascot rows). When the side panel is on, the side panel owns
        // the mascot and the transcript spans the full body area like
        // before. The width/height clamps below keep the floating mascot
        // off the screen on very narrow terminals so transcript content
        // remains readable.
        let inline_mascot = !state.config.show_subagent_panel
            && state.config.show_mascot
            && body_area.width >= 32
            && body_area.height >= 8;
        let mascot_strip_height: u16 = if inline_mascot { 5 } else { 0 };
        let title_line = Line::from(Span::styled(
            format!("─── {} ", body_title(state)),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ));
        let content_area = Rect {
            x: body_area.x + 1,
            y: body_area.y + mascot_strip_height,
            width: body_area.width.saturating_sub(2),
            height: body_area.height.saturating_sub(mascot_strip_height),
        };
        let inner_width = content_area.width;
        let inner_height = content_area.height;
        let banner_lines = sticky_plan_banner(state);
        let following_tail = !state.is_scrolled_back();
        let mut all_lines: Vec<Line<'static>> = Vec::new();
        if !inline_mascot {
            // No mascot strip — keep the legacy layout: title is the very
            // first row of the paragraph, with a blank line after it.
            all_lines.push(title_line.clone());
            all_lines.push(Line::from(""));
        }
        all_lines.extend(banner_lines);
        let blocks = crate::blocks::identify_transcript_blocks(&state.transcript);
        let collapsed_indices = build_collapsed_set(state, &blocks);
        let collapse_indicators = build_collapse_indicators(&blocks, &collapsed_indices);
        for (idx, entry) in state.transcript.iter().enumerate() {
            if is_entry_hidden_in_chat(state, entry) {
                continue;
            }
            if collapsed_indices.contains(&idx) {
                continue;
            }
            let mut entry_lines = style_transcript_entry(state, entry);
            if entry.kind == TranscriptKind::User {
                entry_lines.push(Line::from(""));
            }
            all_lines.extend(entry_lines);
            if let Some(indicator) = collapse_indicators.get(&idx) {
                all_lines.push(Line::from(Span::styled(
                    indicator.clone(),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                )));
            }
        }
        if following_tail && let Some(stream) = state.active_stream.as_ref() {
            all_lines.extend(render_streaming_inline(state, stream));
        }
        // Build once; we use it for both line counting and rendering.
        let paragraph = Paragraph::new(all_lines).wrap(Wrap { trim: false });
        let total_rows = paragraph.line_count(inner_width) as u16;
        let max_scroll = total_rows.saturating_sub(inner_height);
        let scroll_rows = state.scroll_offset.min(max_scroll as usize) as u16;
        let scroll = max_scroll.saturating_sub(scroll_rows);
        frame.render_widget(paragraph.scroll((scroll, 0)), content_area);

        if inline_mascot {
            // Title row sits to the left of the mascot, sharing body_area.y.
            // We carve out 9 cells on the right (8 sprite + 1 padding).
            let mascot_w: u16 = 8;
            let right_pad: u16 = 1;
            let title_area = Rect {
                x: body_area.x + 1,
                y: body_area.y,
                width: body_area.width.saturating_sub(2 + mascot_w + right_pad),
                height: 1,
            };
            frame.render_widget(Paragraph::new(title_line), title_area);
            let mascot_area = Rect {
                x: body_area.x + body_area.width.saturating_sub(mascot_w + right_pad),
                y: body_area.y + 1,
                width: mascot_w,
                height: 4,
            };
            render_mascot(state, mascot_area, frame.buffer_mut());
        }
        if !following_tail && content_area.height >= 1 {
            // Floating hint pinned to the top of the transcript pane so the
            // user always knows they're behind the tail. Without a border
            // there is no row to overlay, so we paint directly into the
            // first content row — the paragraph below will be scrolled
            // forward one row to keep the hint visible.
            let hint_area = Rect {
                x: content_area.x,
                y: content_area.y,
                width: content_area.width,
                height: 1,
            };
            let hint = Line::from(Span::styled(
                format!(
                    "\u{2191} scrolled back {} row{} · PageDown or Shift+\u{2193} to follow",
                    state.scroll_offset,
                    if state.scroll_offset == 1 { "" } else { "s" }
                ),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::DIM),
            ));
            frame.render_widget(Paragraph::new(hint), hint_area);
        }
    }

    if state.layout == LayoutMode::Full && state.config.show_subagent_panel && body_chunks.len() > 1
    {
        // Side panel keeps a single dim `│` on the left edge — enough to
        // visually separate it from the transcript without the four-sided
        // box that historically dragged the transcript copy with it.
        let side_block = Block::default().borders(Borders::LEFT).border_style(
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        );
        let side_area = body_chunks[1];
        frame.render_widget(side_block, side_area);
        let inner = Rect {
            // +2 = 1 col for the border, 1 col for a breathing-room gutter.
            x: side_area.x + 2,
            y: side_area.y,
            width: side_area.width.saturating_sub(3),
            height: side_area.height,
        };
        // Dim "Status" title sits inline as the first row, since the block
        // no longer carries a top border to host a title attribute.
        let title_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1.min(inner.height),
        };
        if title_area.height > 0 {
            let title_line = Line::from(Span::styled(
                "Status",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ));
            frame.render_widget(Paragraph::new(title_line), title_area);
        }
        let body_inner = Rect {
            x: inner.x,
            y: inner.y + 1.min(inner.height),
            width: inner.width,
            height: inner.height.saturating_sub(1),
        };
        let mascot_height =
            if state.config.show_mascot && body_inner.height >= 6 && body_inner.width >= 8 {
                let mascot_area = Rect {
                    x: body_inner.x,
                    y: body_inner.y,
                    width: body_inner.width.min(8),
                    height: 4,
                };
                render_mascot(state, mascot_area, frame.buffer_mut());
                5
            } else {
                0
            };
        let info_area = Rect {
            x: body_inner.x,
            y: body_inner.y + mascot_height,
            width: body_inner.width,
            height: body_inner.height.saturating_sub(mascot_height),
        };
        let done = state
            .side_panel
            .plan
            .iter()
            .filter(|step| step.done)
            .count();
        let plan = state
            .side_panel
            .plan
            .iter()
            .map(|step| {
                let marker = if step.done { "[x]" } else { "[ ]" };
                format!("{marker} {}", step.label)
            })
            .collect::<Vec<_>>()
            .join("\n");
        let goal = render_goal_block(state);
        // Session id is rendered as a short suffix when it fits; the directory
        // entries use long ids like `session-628850-1778945666`, so we keep
        // the last numeric chunk for compactness. The Activity feed and
        // streaming-event spam used to live below this block — those were
        // noisy, mostly duplicated the status bar metrics, and pushed the
        // useful counters off-screen, so they are intentionally omitted.
        let session_id_line = if state.current_session_id.is_empty() {
            String::new()
        } else {
            format!("id: {}\n", short_session_id(&state.current_session_id))
        };
        let committee_block = render_committee_block(state);
        let request_context_block = render_request_context_block(state);
        let side = format!(
            "{goal}Plan {done}/{}\n{}\n\nSession\n{session_id_line}agent: {}\nsteps: {}\nerrors: {}\nelapsed: {}s\n\n{}{}{}",
            state.side_panel.plan.len(),
            plan,
            agent_run_status_label(&state.agent_run_status),
            state.side_panel.stats.steps,
            state.side_panel.stats.errors,
            state.side_panel.stats.elapsed_seconds,
            request_context_block,
            committee_block,
            render_subagent_monitor(&state.subagents),
        );
        frame.render_widget(Paragraph::new(side).wrap(Wrap { trim: false }), info_area);
    }

    frame.render_widget(
        Paragraph::new(render_status_bar(state, chunks[3].width)),
        chunks[3],
    );

    let input_area = chunks[4];
    let prompt_glyph = "\u{276F} "; // ❯ + space, two display columns
    let prompt_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    // Logical line / column of the cursor inside `state.input`. Computed
    // once and reused for both the visible cursor position and the
    // scroll offset that keeps the cursor on-screen.
    let prefix_chars: String = state.input.chars().take(state.input_cursor).collect();
    let cursor_line = prefix_chars.matches('\n').count();
    // Cursor X is measured in *terminal cells*, not characters.
    // Wide glyphs (한국어, 中文, emoji) occupy two cells each; if we
    // count `chars()` the rendered caret lags one cell per CJK
    // character typed, so the user's visible cursor ends up mid-
    // glyph or trailing the last character. `UnicodeWidthStr`
    // returns the same width the terminal will actually draw.
    let cursor_col_chars = prefix_chars
        .rsplit('\n')
        .next()
        .map(unicode_width::UnicodeWidthStr::width)
        .unwrap_or(0);
    // Render each logical line on its own visible row. The first row
    // carries the `❯ ` glyph; continuation rows get a 2-space gutter so
    // they align underneath the prompt content (and the cursor x maths
    // works for both first-line and continuation columns).
    let mut input_lines: Vec<Line<'static>> = Vec::new();
    for (idx, line) in state.input.split('\n').enumerate() {
        let mut spans = Vec::with_capacity(2);
        if idx == 0 {
            spans.push(Span::styled(prompt_glyph, prompt_style));
        } else {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::raw(line.to_string()));
        input_lines.push(Line::from(spans));
    }
    let char_count = state.input.chars().count();
    let line_count = state.input.split('\n').count();
    let title = if char_count == 0 {
        " Enter sends · Ctrl+J newline ".to_string()
    } else if line_count > 1 {
        format!(" {char_count} chars · {line_count} lines · Ctrl+J newline ")
    } else {
        format!(" {char_count} chars · Ctrl+J newline ")
    };
    // Inner content rows = total box height minus the two border rows.
    let visible_rows = input_area.height.saturating_sub(2);
    // Scroll the paragraph so the cursor row is always visible. When the
    // draft fits in the box (rare overflow case still possible at the
    // upper clamp of 10), scroll stays at 0.
    let scroll = (cursor_line as u16).saturating_sub(visible_rows.saturating_sub(1));
    frame.render_widget(
        Paragraph::new(input_lines)
            .block(Block::default().borders(Borders::ALL).title(title))
            .scroll((scroll, 0)),
        input_area,
    );
    // Cursor sits inside the box, offset for the border and the prompt
    // gutter. The y position accounts for the scroll above so multi-line
    // drafts longer than `visible_rows` still anchor the cursor on-screen.
    let cursor_indent = 2u16; // `❯ ` (line 0) and `  ` (continuation) both 2 cells
    let cursor_x = input_area.x
        + 1
        + cursor_indent
        + (cursor_col_chars as u16).min(input_area.width.saturating_sub(cursor_indent + 2));
    let cursor_y_in_box = (cursor_line as u16).saturating_sub(scroll);
    let cursor_y = input_area.y + 1 + cursor_y_in_box;
    // Hide the textarea caret while the agent is working AND the
    // operator hasn't started a draft. With it always on, terminals
    // that draw the blinking caret on top of the previous frame
    // make it look like a stray cursor is flashing next to the
    // spinner / elapsed counter in the status bar — that's just
    // the input caret painted briefly before ratatui repositions
    // it. Suppressing it here removes the visual noise without
    // hurting actual editing: the moment the user types a
    // character, `state.input` is non-empty and the caret returns.
    let agent_busy = matches!(state.agent_run_status, AgentRunStatus::Running);
    let user_has_draft = !state.input.is_empty();
    if !agent_busy || user_has_draft {
        frame.set_cursor_position(Position::new(cursor_x, cursor_y));
    }

    render_slash_picker(frame, state, input_area);
    render_at_picker(frame, state, input_area);
}

/// Floats a small autocomplete overlay above the input box when the buffer
/// starts with `/`. Hidden when other modal panels are active.
fn render_slash_picker(frame: &mut Frame<'_>, state: &TuiState, input_area: Rect) {
    let Some(picker) = state.slash_picker.as_ref() else {
        return;
    };
    if state.menu.is_some() || state.approval.is_some() || state.ask_user.is_some() {
        return;
    }
    if let Some(context) = crate::slash_picker::slash_argument_context_with_dynamic(
        &picker.query,
        &state.skill_suggestions,
        &state.sessions,
    ) {
        let selected = picker.selected.min(context.options.len().saturating_sub(1));
        let visible_limit = 6usize;
        let start = selected
            .saturating_sub(visible_limit.saturating_sub(1))
            .min(context.options.len().saturating_sub(visible_limit));
        let lines: Vec<Line<'static>> = context
            .options
            .iter()
            .enumerate()
            .skip(start)
            .take(visible_limit)
            .map(|(idx, option)| {
                let is_selected = idx == selected;
                let marker = if is_selected { "\u{25B8}" } else { " " };
                let label_style = if is_selected {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Cyan)
                };
                let desc_style = if is_selected {
                    Style::default().fg(Color::Gray)
                } else {
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM)
                };
                Line::from(vec![
                    Span::styled(format!("{marker} "), label_style),
                    Span::styled(option.to_string(), label_style),
                    Span::raw("  "),
                    Span::styled(context.command_name.clone(), desc_style),
                ])
            })
            .collect();
        let height = (lines.len() as u16).min(6).saturating_add(2);
        let width = input_area.width;
        if input_area.y < height {
            return;
        }
        let area = Rect {
            x: input_area.x,
            y: input_area.y.saturating_sub(height),
            width,
            height,
        };
        frame.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .title(format!("options: {}", context.command_name))
                    .borders(Borders::ALL),
            ),
            area,
        );
        return;
    }

    let suggestions =
        crate::slash_picker::filtered_suggestions(&picker.query, &state.skill_suggestions);
    if suggestions.is_empty() {
        return;
    }
    let selected = picker.selected.min(suggestions.len().saturating_sub(1));
    let visible_limit = 6usize;
    let start = selected
        .saturating_sub(visible_limit.saturating_sub(1))
        .min(suggestions.len().saturating_sub(visible_limit));
    let lines: Vec<Line<'static>> = suggestions
        .iter()
        .enumerate()
        .skip(start)
        .take(visible_limit)
        .map(|(idx, spec)| {
            let is_selected = idx == selected;
            let marker = if is_selected { "\u{25B8}" } else { " " };
            let label_style = if is_selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Cyan)
            };
            let desc_style = if is_selected {
                Style::default().fg(Color::Gray)
            } else {
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM)
            };
            let label = if let Some(arg) = spec.arg_hint.as_deref() {
                format!("{}  {arg}", spec.name)
            } else {
                spec.name.to_string()
            };
            Line::from(vec![
                Span::styled(format!("{marker} "), label_style),
                Span::styled(label, label_style),
                Span::raw("  "),
                Span::styled(spec.description.to_string(), desc_style),
            ])
        })
        .collect();
    let height = (lines.len() as u16).min(6).saturating_add(2);
    let width = input_area.width;
    if input_area.y < height {
        return;
    }
    let area = Rect {
        x: input_area.x,
        y: input_area.y.saturating_sub(height),
        width,
        height,
    };
    let title = if picker.query == "/" {
        "commands".to_string()
    } else {
        format!("commands: {}", picker.query)
    };
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().title(title).borders(Borders::ALL)),
        area,
    );
}

/// Floats the `@file` picker above the input box. Hidden when other
/// modal panels are active so two overlays can't overlap; otherwise
/// mirrors the slash picker layout (overlay above input, top-aligned
/// against the input border).
fn render_at_picker(frame: &mut Frame<'_>, state: &TuiState, input_area: Rect) {
    let Some(picker) = state.at_picker.as_ref() else {
        return;
    };
    if state.menu.is_some() || state.approval.is_some() || state.ask_user.is_some() {
        return;
    }
    let matches = crate::at_picker::filter_paths(&state.at_picker_index, &picker.query);
    if matches.is_empty() {
        return;
    }
    let highlight = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let dim = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::DIM);
    let lines: Vec<Line<'static>> = matches
        .iter()
        .enumerate()
        .map(|(idx, path)| {
            let marker = if idx == picker.selected {
                "\u{25B8}"
            } else {
                " "
            };
            let style = if idx == picker.selected {
                highlight
            } else {
                Style::default()
            };
            Line::from(vec![
                Span::styled(format!("{marker} "), style),
                Span::styled((*path).clone(), style),
            ])
        })
        .collect();
    let height = (lines.len() as u16).min(crate::at_picker::AT_PICKER_LIMIT as u16) + 2;
    if input_area.y < height {
        return;
    }
    let area = Rect {
        x: input_area.x,
        y: input_area.y.saturating_sub(height),
        width: input_area.width,
        height,
    };
    let title = if picker.query.is_empty() {
        "@file".to_string()
    } else {
        format!("@file: {}", picker.query)
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().title(title).borders(Borders::ALL))
            .style(dim),
        area,
    );
}

pub(super) fn body_title(state: &TuiState) -> &'static str {
    if state.menu.is_some() {
        "Menu"
    } else if state.approval.is_some() {
        "Approval"
    } else if state.ask_user.is_some() {
        "Ask User"
    } else if should_render_welcome(state) {
        "Welcome"
    } else {
        "Transcript"
    }
}

pub(super) fn render_menu(menu: &MenuState) -> String {
    let options = menu
        .options
        .iter()
        .enumerate()
        .map(|(index, option)| {
            let marker = if index == menu.selected_index {
                ">"
            } else {
                " "
            };
            format!("{marker} {option}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Peridot Menu\n\n\
         Enter selects a menu item. Esc or q closes this menu and returns to chat input.\n\n\
         {options}"
    )
}

pub(super) fn render_ask_user_panel(panel: &AskUserPanel) -> String {
    if panel.choices.is_empty() {
        return format!("{}\n\n> {}", panel.question, panel.freeform);
    }
    let choices = panel
        .choices
        .iter()
        .enumerate()
        .map(|(index, choice)| {
            let cursor = if index == panel.selected_index {
                ">"
            } else {
                " "
            };
            // Multi-select mode shows `[x]` / `[ ]` checkboxes in front
            // of every choice so the operator can see what they've
            // already toggled on. Single-select just shows the cursor
            // marker like before.
            if panel.multi_select {
                let check = if panel.is_toggled(index) { "x" } else { " " };
                format!("{cursor} [{check}] {choice}")
            } else {
                format!("{cursor} {choice}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let footer = if panel.multi_select {
        Some("[Space] toggle  [Enter] commit selection".to_string())
    } else {
        None
    };
    let mut sections = vec![panel.question.clone(), String::new(), choices];
    if panel.showing_explanation {
        let explanation = panel
            .explanation
            .as_deref()
            .unwrap_or("No explanation provided.");
        sections.push(String::new());
        sections.push(explanation.to_string());
    }
    if let Some(f) = footer {
        sections.push(String::new());
        sections.push(f);
    }
    sections.join("\n")
}

pub(super) fn render_branch_picker(picker: &crate::BranchPickerState) -> String {
    let mut sections = vec!["Branch from turn".to_string(), String::new()];
    if !picker.loaded {
        sections.push("  loading turn list…".to_string());
        sections.push(String::new());
        sections.push("  q / Esc to cancel".to_string());
        return sections.join("\n");
    }
    if picker.turns.is_empty() {
        sections.push("  (no turns recorded in this session)".to_string());
        sections.push(String::new());
        sections.push("  q / Esc to cancel".to_string());
        return sections.join("\n");
    }
    for (index, turn) in picker.turns.iter().enumerate() {
        let cursor = if index == picker.selected { ">" } else { " " };
        sections.push(format!(
            "  {cursor} turn {id:>3}  [{source:<9}] {preview}",
            id = turn.turn_id,
            source = turn.source,
            preview = turn.preview,
        ));
    }
    sections.push(String::new());
    sections.push("  ↑/↓ navigate  •  Enter fork  •  q / Esc cancel".to_string());
    sections.join("\n")
}

pub(super) fn render_session_picker(
    picker: &crate::SessionPickerState,
    sessions: &[crate::SessionDirectoryItem],
    current_session_id: &str,
) -> String {
    let mut sections = vec!["Switch session".to_string(), String::new()];
    sections.push(format!("  filter: {}", picker.query));
    sections.push(String::new());

    let matches = crate::session_picker::filtered_sessions(sessions, &picker.query);
    if matches.is_empty() {
        sections.push("  (no matching sessions)".to_string());
    } else {
        for (index, item) in matches.iter().enumerate() {
            let cursor = if index == picker.selected { ">" } else { " " };
            let current = if item.id == current_session_id {
                "*"
            } else {
                " "
            };
            let attention = if item.pending_attention { "!" } else { " " };
            sections.push(format!(
                "  {cursor}{current}{attention} {title}  [{status}]  {id}",
                title = item.title,
                status = format!("{:?}", item.status).to_ascii_lowercase(),
                id = item.id,
            ));
        }
    }

    sections.push(String::new());
    sections.push("  type to filter  •  ↑/↓ navigate  •  Enter switch  •  Esc cancel".to_string());
    sections.join("\n")
}

pub(super) fn render_approval_panel(panel: &ApprovalPanel) -> String {
    let choices = panel
        .choices()
        .iter()
        .enumerate()
        .map(|(index, choice)| {
            let marker = if index == panel.selected_index {
                ">"
            } else {
                " "
            };
            format!("{marker} {choice}")
        })
        .collect::<Vec<_>>()
        .join("\n");

    let mut sections = vec![
        "Approval required".to_string(),
        String::new(),
        format!("Tool: {}", panel.tool_name),
        format!("Reason: {}", panel.reason),
    ];

    if !panel.tool_params.is_null() {
        let pretty = serde_json::to_string_pretty(&panel.tool_params)
            .unwrap_or_else(|_| panel.tool_params.to_string());
        let preview: String = pretty
            .lines()
            .take(8)
            .map(|line| format!("  {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        if !preview.is_empty() {
            sections.push(String::new());
            sections.push("Params:".to_string());
            sections.push(preview);
        }
    }

    if !panel.hunks.is_empty() {
        sections.push(String::new());
        let accepted = panel
            .hunk_accepted
            .iter()
            .filter(|accepted| **accepted)
            .count();
        sections.push(format!(
            "Hunks: {accepted}/{total} staged  (Tab/Space toggles, \u{2190}/\u{2192} navigates)",
            total = panel.hunks.len()
        ));
        for (index, hunk) in panel.hunks.iter().enumerate() {
            let focused = panel.focused_hunk == Some(index);
            let cursor = if focused { ">" } else { " " };
            let accepted = panel.hunk_accepted.get(index).copied().unwrap_or(true);
            let stage = if accepted { "[x]" } else { "[ ]" };
            sections.push(format!(
                "  {cursor} {stage} hunk {idx}: {label}",
                idx = index + 1,
                label = hunk.label()
            ));
            if focused {
                let preview = hunk.unified_preview();
                for line in preview.lines().take(12) {
                    sections.push(format!("      {line}"));
                }
                let total_lines = preview.lines().count();
                if total_lines > 12 {
                    sections.push(format!("      ... +{} more lines", total_lines - 12));
                }
            }
        }
    } else if let Some(diff) = panel.diff_preview.as_ref() {
        sections.push(String::new());
        sections.push("Diff:".to_string());
        sections.push(
            diff.lines()
                .map(|line| format!("  {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }

    sections.push(String::new());
    sections.push(choices);
    sections.join("\n")
}

/// Selects a layout mode from terminal dimensions.
pub fn select_layout(width: u16, height: u16) -> LayoutMode {
    if width >= 120 && height >= 32 {
        LayoutMode::Full
    } else if width >= 80 && height >= 20 {
        LayoutMode::Compact
    } else {
        LayoutMode::Minimal
    }
}
