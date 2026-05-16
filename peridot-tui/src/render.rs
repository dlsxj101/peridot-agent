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
    let mut parts = vec![format!(
        "{} · {}",
        state.header.mode, state.header.permission
    )];
    if state.config.show_token_count {
        parts.push(format!("{} tok", state.header.total_tokens));
    }
    if state.config.show_cost {
        parts.push(format!("${:.4}", state.header.cost_usd));
    }
    if state.config.show_cache_rate {
        parts.push(format!("cache {:.0}%", state.header.cache_hit_rate * 100.0));
    }
    if let Some(status) = state.goal_status.as_ref() {
        parts.push(format!("goal {}", goal_status_label(Some(status))));
    }
    if state.agent_run_status != AgentRunStatus::Idle {
        parts.push(format!(
            "agent {}",
            agent_run_status_label(&state.agent_run_status)
        ));
    }
    parts.join("  |  ")
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

pub(super) fn render_activity_list(activities: &[RuntimeActivity]) -> String {
    if activities.is_empty() {
        return "Activity\n<none>".to_string();
    }
    let rendered = activities
        .iter()
        .rev()
        .take(5)
        .rev()
        .map(|activity| {
            format!(
                "{} {}: {}",
                activity_kind_label(&activity.kind),
                activity.label,
                activity.status
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("Activity\n{rendered}")
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
         4. Multi-line input  →  Shift+Enter for a newline\n\n\
         Slash commands\n\
         /plan  /execute  /goal <objective>  /safe  /auto  /yolo  /help  /lang en|ko\n\n\
         Keys\n\
         Enter sends  |  Shift+Enter newline  |  Esc interrupts/menu  |  Ctrl+P menu  |  Ctrl+] side panel  |  Ctrl-C quits",
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
        TranscriptKind::Debug | TranscriptKind::Thinking => !state.debug_view,
        _ => false,
    }
}

/// Builds a styled line for one transcript entry.
fn style_transcript_entry(state: &TuiState, entry: &TranscriptEntry) -> Line<'static> {
    let is_indented_detail = matches!(
        entry.kind,
        TranscriptKind::ToolStart | TranscriptKind::ToolOk | TranscriptKind::ToolFail
    ) && entry.text.starts_with("  ");
    if is_indented_detail {
        return Line::from(Span::styled(
            entry.text.clone(),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ));
    }
    match entry.kind {
        TranscriptKind::User => Line::from(vec![
            Span::styled(
                "\u{25B8} ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                entry.text.clone(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        TranscriptKind::Assistant => Line::from(vec![
            Span::styled(
                "\u{25C6} ",
                Style::default().fg(theme_accent(&state.config)),
            ),
            Span::raw(entry.text.clone()),
        ]),
        TranscriptKind::ToolStart => Line::from(vec![
            Span::styled("\u{276F} ", Style::default().fg(Color::DarkGray)),
            Span::styled(entry.text.clone(), Style::default().fg(Color::DarkGray)),
        ]),
        TranscriptKind::ToolOk => Line::from(vec![
            Span::styled("\u{2714} ", Style::default().fg(Color::Green)),
            Span::styled(entry.text.clone(), Style::default().fg(Color::Green)),
        ]),
        TranscriptKind::ToolFail => Line::from(vec![
            Span::styled("\u{2718} ", Style::default().fg(Color::Red)),
            Span::styled(
                entry.text.clone(),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
        ]),
        TranscriptKind::System => Line::from(Span::styled(
            entry.text.clone(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::DIM),
        )),
        TranscriptKind::Notice => Line::from(vec![
            Span::styled("\u{26A0} ", Style::default().fg(Color::Yellow)),
            Span::styled(entry.text.clone(), Style::default().fg(Color::Yellow)),
        ]),
        TranscriptKind::Error => Line::from(vec![
            Span::styled(
                "\u{26A0} ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                entry.text.clone(),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
        ]),
        TranscriptKind::Debug => Line::from(Span::styled(
            entry.text.clone(),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )),
        TranscriptKind::Thinking => Line::from(vec![
            Span::styled(
                "\u{2026} ",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM | Modifier::ITALIC),
            ),
            Span::styled(
                entry.text.clone(),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM | Modifier::ITALIC),
            ),
        ]),
        TranscriptKind::TurnSeparator => Line::from(Span::styled(
            format!("── {} {}", entry.text, "─".repeat(40)),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )),
    }
}

/// Builds a styled line for the active assistant stream.
/// Returns None when no delta has arrived yet (avoids a noisy placeholder).
fn style_active_stream(state: &TuiState, stream: &StreamState) -> Option<Line<'static>> {
    if stream.content.is_empty() {
        return None;
    }
    Some(Line::from(vec![
        Span::styled(
            "\u{25C6} ",
            Style::default()
                .fg(theme_accent(&state.config))
                .add_modifier(Modifier::DIM),
        ),
        Span::styled(
            "streaming...",
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::DIM | Modifier::ITALIC),
        ),
    ]))
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

/// Renders a 1-line agent status bar (icon, label, queue depth, metrics).
fn render_status_bar(state: &TuiState) -> Line<'static> {
    let waiting_user = state.ask_user.is_some()
        || state.approval.is_some()
        || state.agent_run_status == AgentRunStatus::WaitingApproval;
    let (icon, icon_style) = if waiting_user {
        ("\u{25CF}", Style::default().fg(Color::Rgb(255, 165, 0)))
    } else if !state.active_tools.is_empty() {
        (
            "\u{25CF}",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    } else if state.active_stream.is_some() || state.agent_run_status == AgentRunStatus::Running {
        (
            "\u{25CF}",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    } else if state.agent_run_status == AgentRunStatus::Failed {
        ("\u{25CF}", Style::default().fg(Color::Red))
    } else if state.agent_run_status == AgentRunStatus::Interrupted {
        ("\u{25CF}", Style::default().fg(Color::Magenta))
    } else {
        ("\u{25CF}", Style::default().fg(Color::Green))
    };

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
    spans.push(Span::styled(
        format!("  · {}", render_status_metrics(state)),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    ));
    Line::from(spans)
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
    if state.layout == LayoutMode::Full && state.config.show_subagent_panel {
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
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(tab_height),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(3),
        ])
        .split(area);

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
    if let Some(version) = state.header.update_available.as_ref() {
        header_spans.push(Span::raw("  "));
        header_spans.push(Span::styled(
            format!("· update {version} :update"),
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
    let body_block = Block::default()
        .title(body_title(state))
        .borders(Borders::ALL);
    if let Some(menu) = &state.menu {
        frame.render_widget(
            Paragraph::new(render_menu(menu))
                .block(body_block)
                .wrap(Wrap { trim: false }),
            body_chunks[0],
        );
    } else if let Some(panel) = &state.approval {
        frame.render_widget(
            Paragraph::new(render_approval_panel(panel))
                .block(body_block)
                .wrap(Wrap { trim: false }),
            body_chunks[0],
        );
    } else if let Some(panel) = &state.ask_user {
        frame.render_widget(
            Paragraph::new(render_ask_user_panel(panel))
                .block(body_block)
                .wrap(Wrap { trim: false }),
            body_chunks[0],
        );
    } else if should_render_welcome(state) {
        frame.render_widget(
            Paragraph::new(render_welcome(state))
                .block(body_block)
                .wrap(Wrap { trim: false }),
            body_chunks[0],
        );
    } else {
        let banner_lines = sticky_plan_banner(state);
        let capacity = body_chunks[0].height.saturating_sub(2) as usize;
        let visible: Vec<&TranscriptEntry> = state
            .transcript
            .iter()
            .filter(|entry| !is_entry_hidden(state, entry))
            .collect();
        let stream_line = state
            .active_stream
            .as_ref()
            .and_then(|stream| style_active_stream(state, stream));
        let stream_reserve = if stream_line.is_some() { 1 } else { 0 };
        let take = capacity
            .saturating_sub(stream_reserve)
            .saturating_sub(banner_lines.len());
        let mut lines: Vec<Line<'static>> = banner_lines;
        lines.extend(
            visible
                .iter()
                .rev()
                .take(take)
                .rev()
                .map(|entry| style_transcript_entry(state, entry)),
        );
        if let Some(line) = stream_line {
            lines.push(line);
        }
        frame.render_widget(
            Paragraph::new(lines)
                .block(body_block)
                .wrap(Wrap { trim: false }),
            body_chunks[0],
        );
    }

    if state.layout == LayoutMode::Full && state.config.show_subagent_panel && body_chunks.len() > 1
    {
        let side_block = Block::default().title("Status").borders(Borders::ALL);
        let side_area = body_chunks[1];
        frame.render_widget(side_block, side_area);
        let inner = Rect {
            x: side_area.x + 1,
            y: side_area.y + 1,
            width: side_area.width.saturating_sub(2),
            height: side_area.height.saturating_sub(2),
        };
        let mascot_height = if state.config.show_mascot && inner.height >= 6 && inner.width >= 8 {
            let mascot_area = Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width.min(8),
                height: 4,
            };
            render_mascot(state, mascot_area, frame.buffer_mut());
            5
        } else {
            0
        };
        let info_area = Rect {
            x: inner.x,
            y: inner.y + mascot_height,
            width: inner.width,
            height: inner.height.saturating_sub(mascot_height),
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
        let goal = state
            .goal_status
            .as_ref()
            .map(|status| format!("Goal: {}\n\n", goal_status_label(Some(status))))
            .unwrap_or_default();
        let side = format!(
            "{goal}Plan {done}/{}\n{}\n\nSession\nagent: {}\nsteps: {}\nerrors: {}\nelapsed: {}s\n\n{}\n\n{}",
            state.side_panel.plan.len(),
            plan,
            agent_run_status_label(&state.agent_run_status),
            state.side_panel.stats.steps,
            state.side_panel.stats.errors,
            state.side_panel.stats.elapsed_seconds,
            render_subagent_monitor(&state.subagents),
            render_activity_list(&state.activities)
        );
        frame.render_widget(Paragraph::new(side).wrap(Wrap { trim: false }), info_area);
    }

    frame.render_widget(Paragraph::new(render_status_bar(state)), chunks[3]);

    let input_area = chunks[4];
    let input_line = Line::from(vec![
        Span::styled(
            "\u{276F} ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(state.input.clone()),
    ]);
    frame.render_widget(
        Paragraph::new(input_line).block(Block::default().borders(Borders::ALL)),
        input_area,
    );
    let cursor_x =
        input_area.x + 2 + (state.input_cursor as u16).min(input_area.width.saturating_sub(4));
    frame.set_cursor_position(Position::new(cursor_x, input_area.y + 1));

    render_slash_picker(frame, state, input_area);
}

/// Floats a small autocomplete overlay above the input box when the buffer
/// starts with `/`. Hidden when other modal panels are active.
fn render_slash_picker(frame: &mut Frame<'_>, state: &TuiState, input_area: Rect) {
    if !state.input.starts_with('/') {
        return;
    }
    if state.menu.is_some() || state.approval.is_some() || state.ask_user.is_some() {
        return;
    }
    let suggestions = filtered_specs(&state.input);
    if suggestions.is_empty() {
        return;
    }
    let lines: Vec<Line<'static>> = suggestions
        .iter()
        .take(6)
        .map(|spec| {
            let label = if let Some(arg) = spec.arg_hint {
                format!("{}  {arg}", spec.name)
            } else {
                spec.name.to_string()
            };
            Line::from(vec![
                Span::styled(
                    label,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    spec.description.to_string(),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ),
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
        Paragraph::new(lines).block(Block::default().title("commands").borders(Borders::ALL)),
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
            let marker = if index == panel.selected_index {
                ">"
            } else {
                " "
            };
            format!("{marker} {choice}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    if panel.showing_explanation {
        let explanation = panel
            .explanation
            .as_deref()
            .unwrap_or("No explanation provided.");
        format!("{}\n\n{}\n\n{}", panel.question, choices, explanation)
    } else {
        format!("{}\n\n{}", panel.question, choices)
    }
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

    if let Some(diff) = panel.diff_preview.as_ref() {
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
