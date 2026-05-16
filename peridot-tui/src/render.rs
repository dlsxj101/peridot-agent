use super::*;
use ratatui::style::Modifier;
use state::{TranscriptEntry, TranscriptKind};

pub(super) fn render_header_text(state: &TuiState) -> String {
    format!("PERIDOT | {}", render_header_status(state))
}

pub(super) fn render_header_status(state: &TuiState) -> String {
    let mut parts = vec![
        format!("{}.{}", state.header.mode, state.header.permission),
        state.header.model.clone(),
    ];
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
    parts.join(" | ")
}

pub(super) fn agent_run_status_label(status: &AgentRunStatus) -> &'static str {
    match status {
        AgentRunStatus::Idle => "idle",
        AgentRunStatus::Running => "running",
        AgentRunStatus::Succeeded => "done",
        AgentRunStatus::Failed => "failed",
        AgentRunStatus::WaitingApproval => "waiting-approval",
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
         mode       {}.{}\n\
         workspace  {}\n\n\
         Type a task in the input line below and press Enter.\n\n\
         Try\n\
         - fix the failing tests and explain the change\n\
         - create a small utility and add focused tests\n\n\
         Slash commands\n\
         /plan  /execute  /goal <objective>  /safe  /auto  /yolo  /help\n\n\
         Keys\n\
         Enter sends  |  Esc opens/closes menu  |  Up/Down history  |  Ctrl-C quits",
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
            let summary = subagent
                .summary
                .as_ref()
                .map(|summary| format!(": {summary}"))
                .unwrap_or_default();
            format!(
                "{} {} [{}]{}",
                subagent.kind, subagent.task, subagent.status, summary
            )
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

/// Returns true when the entry should be hidden in normal (non-debug) view.
fn is_entry_hidden(state: &TuiState, entry: &TranscriptEntry) -> bool {
    matches!(entry.kind, TranscriptKind::Debug) && !state.debug_view
}

/// Builds a styled line for one transcript entry.
fn style_transcript_entry(state: &TuiState, entry: &TranscriptEntry) -> Line<'static> {
    match entry.kind {
        TranscriptKind::User => Line::from(vec![
            Span::styled(
                "> ",
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
        TranscriptKind::ToolStart => {
            let glyph = if state
                .active_tools
                .iter()
                .any(|name| entry.text.starts_with(&format!("tool {name}:")))
            {
                state.spinner_frame().to_string()
            } else {
                "\u{2022}".to_string()
            };
            Line::from(vec![
                Span::styled(
                    format!("{glyph} "),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(entry.text.clone(), Style::default().fg(Color::DarkGray)),
            ])
        }
        TranscriptKind::ToolOk => Line::from(vec![
            Span::styled(
                "\u{2714} ",
                Style::default().fg(Color::Green),
            ),
            Span::styled(entry.text.clone(), Style::default().fg(Color::Green)),
        ]),
        TranscriptKind::ToolFail => Line::from(vec![
            Span::styled(
                "\u{2718} ",
                Style::default().fg(Color::Red),
            ),
            Span::styled(
                entry.text.clone(),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
        ]),
        TranscriptKind::System => Line::from(Span::styled(
            entry.text.clone(),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::DIM),
        )),
        TranscriptKind::Notice => Line::from(vec![
            Span::styled("\u{1F4CC} ", Style::default().fg(Color::Yellow)),
            Span::styled(entry.text.clone(), Style::default().fg(Color::Yellow)),
        ]),
        TranscriptKind::Error => Line::from(vec![
            Span::styled(
                "! ",
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                entry.text.clone(),
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        TranscriptKind::Debug => Line::from(Span::styled(
            entry.text.clone(),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )),
    }
}

/// Builds a styled line for the active assistant stream.
fn style_active_stream(state: &TuiState, stream: &StreamState) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            "\u{25C6} ",
            Style::default()
                .fg(theme_accent(&state.config))
                .add_modifier(Modifier::DIM),
        ),
        Span::styled(
            format!("{}: streaming...", stream.label),
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::DIM | Modifier::ITALIC),
        ),
    ])
}

/// Human-readable description of the current agent activity.
pub(super) fn agent_status_summary(state: &TuiState) -> String {
    if state.ask_user.is_some() {
        return "사용자 응답 대기".to_string();
    }
    if state.approval.is_some() || state.agent_run_status == AgentRunStatus::WaitingApproval {
        return "승인 대기 중".to_string();
    }
    if !state.active_tools.is_empty() {
        let names = state.active_tools.join(", ");
        return format!("도구 실행 중: {names}");
    }
    if state.active_stream.is_some() {
        return "처리 중...".to_string();
    }
    match state.agent_run_status {
        AgentRunStatus::Idle => "대기 중".to_string(),
        AgentRunStatus::Running => "처리 중...".to_string(),
        AgentRunStatus::Succeeded => "완료".to_string(),
        AgentRunStatus::Failed => "실패".to_string(),
        AgentRunStatus::WaitingApproval => "승인 대기 중".to_string(),
    }
}

/// Renders a 1-line agent status bar (icon, label, queue depth).
fn render_status_bar(state: &TuiState) -> Line<'static> {
    let (icon, icon_style) = if state.ask_user.is_some() {
        ("\u{25CF}", Style::default().fg(Color::Rgb(255, 165, 0)))
    } else if state.approval.is_some()
        || state.agent_run_status == AgentRunStatus::WaitingApproval
    {
        ("\u{25CF}", Style::default().fg(Color::Rgb(255, 165, 0)))
    } else if !state.active_tools.is_empty() {
        (
            "\u{25CF}",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    } else if state.active_stream.is_some()
        || state.agent_run_status == AgentRunStatus::Running
    {
        (
            "\u{25CF}",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    } else if state.agent_run_status == AgentRunStatus::Failed {
        ("\u{25CF}", Style::default().fg(Color::Red))
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
            format!("  | 대기열 {} 건", state.input_queue.len()),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::DIM),
        ));
    }
    Line::from(spans)
}

/// Renders a deterministic text snapshot for tests and headless previews.
pub fn render_text_snapshot(state: &TuiState) -> String {
    let mut output = String::new();
    let _ = writeln!(output, "{}", render_header_text(state));
    let _ = writeln!(output, "layout: {:?}", state.layout);
    let _ = writeln!(output);
    if should_render_welcome(state) {
        let _ = writeln!(output, "{}", render_welcome(state));
    } else {
        let visible = state
            .transcript
            .iter()
            .filter(|entry| !is_entry_hidden(state, entry))
            .collect::<Vec<_>>();
        for entry in visible.iter().rev().take(20).rev() {
            let _ = writeln!(output, "{}", entry.text);
        }
    }
    if let Some(stream) = &state.active_stream {
        let _ = writeln!(output, "{}: {}", stream.label, stream.content);
    }
    let _ = writeln!(output, "status: {}", agent_status_summary(state));
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
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(3),
        ])
        .split(area);

    let header = Paragraph::new(Line::from(vec![
        Span::styled("PERIDOT", Style::default().fg(theme_accent(&state.config))),
        Span::raw(format!(" | {}", render_header_status(state))),
    ]))
    .block(Block::default().borders(Borders::ALL));
    frame.render_widget(header, chunks[0]);

    let body_chunks = if state.layout == LayoutMode::Full && state.config.show_subagent_panel {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
            .split(chunks[1])
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(100)])
            .split(chunks[1])
    };
    let body_block = Block::default()
        .title(body_title(state))
        .borders(Borders::ALL);
    if let Some(menu) = &state.menu {
        frame.render_widget(
            Paragraph::new(render_menu(menu)).block(body_block),
            body_chunks[0],
        );
    } else if let Some(panel) = &state.approval {
        frame.render_widget(
            Paragraph::new(render_approval_panel(panel)).block(body_block),
            body_chunks[0],
        );
    } else if let Some(panel) = &state.ask_user {
        frame.render_widget(
            Paragraph::new(render_ask_user_panel(panel)).block(body_block),
            body_chunks[0],
        );
    } else if should_render_welcome(state) {
        frame.render_widget(
            Paragraph::new(render_welcome(state)).block(body_block),
            body_chunks[0],
        );
    } else {
        let capacity = body_chunks[0].height.saturating_sub(2) as usize;
        let visible: Vec<&TranscriptEntry> = state
            .transcript
            .iter()
            .filter(|entry| !is_entry_hidden(state, entry))
            .collect();
        let stream_lines = if state.active_stream.is_some() { 1 } else { 0 };
        let take = capacity.saturating_sub(stream_lines);
        let mut lines: Vec<Line<'static>> = visible
            .iter()
            .rev()
            .take(take)
            .rev()
            .map(|entry| style_transcript_entry(state, entry))
            .collect();
        if let Some(stream) = &state.active_stream {
            lines.push(style_active_stream(state, stream));
        }
        frame.render_widget(Paragraph::new(lines).block(body_block), body_chunks[0]);
    }

    if state.layout == LayoutMode::Full && state.config.show_subagent_panel && body_chunks.len() > 1
    {
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
        frame.render_widget(
            Paragraph::new(side).block(Block::default().title("Status").borders(Borders::ALL)),
            body_chunks[1],
        );
    }

    frame.render_widget(Paragraph::new(render_status_bar(state)), chunks[2]);

    let input_area = chunks[3];
    let input_line = Line::from(vec![
        Span::styled(
            "> ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(state.input.clone()),
    ]);
    frame.render_widget(
        Paragraph::new(input_line)
            .block(Block::default().title(input_title()).borders(Borders::ALL)),
        input_area,
    );
    let cursor_x =
        input_area.x + 2 + (state.input_cursor as u16).min(input_area.width.saturating_sub(4));
    frame.set_cursor_position(Position::new(cursor_x, input_area.y + 1));
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

pub(super) fn input_title() -> &'static str {
    "Input - Enter sends | / commands | Esc menu | Ctrl-C quit"
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
    format!(
        "Approval required\n\nTool: {}\nReason: {}\n\n{}",
        panel.tool_name, panel.reason, choices
    )
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
