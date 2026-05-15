use super::*;

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
    parts.join(" | ")
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

/// Renders a deterministic text snapshot for tests and headless previews.
pub fn render_text_snapshot(state: &TuiState) -> String {
    let mut output = String::new();
    let _ = writeln!(output, "{}", render_header_text(state));
    let _ = writeln!(output, "layout: {:?}", state.layout);
    let _ = writeln!(output);
    for line in state.transcript.iter().rev().take(20).rev() {
        let _ = writeln!(output, "{line}");
    }
    if let Some(stream) = &state.active_stream {
        let _ = writeln!(output, "{}: {}", stream.label, stream.content);
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
    let transcript = if let Some(menu) = &state.menu {
        render_menu(menu)
    } else if let Some(panel) = &state.ask_user {
        render_ask_user_panel(panel)
    } else {
        let mut transcript = state
            .transcript
            .iter()
            .rev()
            .take(body_chunks[0].height.saturating_sub(2) as usize)
            .rev()
            .cloned()
            .collect::<Vec<_>>();
        if let Some(stream) = &state.active_stream {
            transcript.push(format!("{}: {}", stream.label, stream.content));
        }
        transcript.join("\n")
    };
    frame.render_widget(
        Paragraph::new(transcript).block(
            Block::default()
                .title(body_title(state))
                .borders(Borders::ALL),
        ),
        body_chunks[0],
    );

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
            "{goal}Plan {done}/{}\n{}\n\nSession\nsteps: {}\nerrors: {}\nelapsed: {}s\n\n{}\n\n{}",
            state.side_panel.plan.len(),
            plan,
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

    frame.render_widget(
        Paragraph::new(format!("> {}", state.input))
            .block(Block::default().title("Input").borders(Borders::ALL)),
        chunks[2],
    );
}

pub(super) fn body_title(state: &TuiState) -> &'static str {
    if state.menu.is_some() {
        "Menu"
    } else if state.ask_user.is_some() {
        "Ask User"
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
    format!("Peridot Menu\n\n{options}")
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
