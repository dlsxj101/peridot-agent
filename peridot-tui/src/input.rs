use super::*;
use state::{AgentRunStatus, TranscriptKind};

/// Runs the interactive terminal UI until the user quits or submits a task.
pub fn run_interactive(mut state: TuiState) -> io::Result<TuiExit> {
    let mut terminal = TerminalGuard::enter()?;
    let (width, height) = terminal_size()?;
    state.resize(width, height);
    let submitted = loop {
        state.tick_spinner();
        terminal.terminal.draw(|frame| draw(frame, &state))?;
        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) => match handle_key_event(&mut state, key) {
                    TuiEventOutcome::Continue => {}
                    TuiEventOutcome::Quit => break None,
                    TuiEventOutcome::Submit(task) => break Some(task),
                    TuiEventOutcome::Approval { .. } | TuiEventOutcome::Interrupt => {}
                },
                Event::Resize(width, height) => state.resize(width, height),
                _ => {}
            }
        }
    };
    Ok(TuiExit { state, submitted })
}

/// Runs the interactive terminal UI while background runtime events update it.
pub fn run_interactive_with_events<F>(
    mut state: TuiState,
    runtime_events: std::sync::mpsc::Receiver<TuiRuntimeEvent>,
    mut on_submit: F,
    mut on_approval: impl FnMut(ApprovalDecision, ApprovalScope, String, String, &mut TuiState),
    mut on_interrupt: impl FnMut(&mut TuiState),
) -> io::Result<TuiExit>
where
    F: FnMut(String, &mut TuiState),
{
    let mut terminal = TerminalGuard::enter()?;
    let (width, height) = terminal_size()?;
    state.resize(width, height);
    loop {
        for event in runtime_events.try_iter() {
            state.apply_runtime_event(event);
        }
        drain_input_queue(&mut state, &mut on_submit);
        state.tick_spinner();
        terminal.terminal.draw(|frame| draw(frame, &state))?;
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => match handle_key_event(&mut state, key) {
                    TuiEventOutcome::Continue => {}
                    TuiEventOutcome::Quit => break,
                    TuiEventOutcome::Submit(task) => on_submit(task, &mut state),
                    TuiEventOutcome::Approval {
                        decision,
                        scope,
                        tool_name,
                        reason,
                    } => on_approval(decision, scope, tool_name, reason, &mut state),
                    TuiEventOutcome::Interrupt => on_interrupt(&mut state),
                },
                Event::Resize(width, height) => state.resize(width, height),
                _ => {}
            }
        }
    }
    Ok(TuiExit {
        state,
        submitted: None,
    })
}

/// Applies a keyboard event to the TUI state.
pub fn handle_key_event(state: &mut TuiState, key: KeyEvent) -> TuiEventOutcome {
    if state.menu.is_some() {
        return handle_menu_key_event(state, key);
    }
    if state.approval.is_some() {
        return handle_approval_key_event(state, key);
    }
    if state.ask_user.is_some() {
        return handle_ask_user_key_event(state, key);
    }
    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            TuiEventOutcome::Quit
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            TuiEventOutcome::Quit
        }
        KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.backspace_input();
            TuiEventOutcome::Continue
        }
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.move_input_cursor_home();
            TuiEventOutcome::Continue
        }
        KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.move_input_cursor_end();
            TuiEventOutcome::Continue
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.clear_input();
            TuiEventOutcome::Continue
        }
        KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.transcript.clear();
            TuiEventOutcome::Continue
        }
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.menu = Some(MenuState::default());
            TuiEventOutcome::Continue
        }
        KeyCode::Char(']') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.config.show_subagent_panel = !state.config.show_subagent_panel;
            TuiEventOutcome::Continue
        }
        KeyCode::Esc => {
            if state.is_agent_busy() {
                TuiEventOutcome::Interrupt
            } else if !state.input.is_empty() {
                state.clear_input();
                TuiEventOutcome::Continue
            } else {
                state.menu = Some(MenuState::default());
                TuiEventOutcome::Continue
            }
        }
        KeyCode::Up => {
            state.previous_input_history();
            TuiEventOutcome::Continue
        }
        KeyCode::Down => {
            state.next_input_history();
            TuiEventOutcome::Continue
        }
        KeyCode::Left => {
            state.move_input_cursor_left();
            TuiEventOutcome::Continue
        }
        KeyCode::Right => {
            state.move_input_cursor_right();
            TuiEventOutcome::Continue
        }
        KeyCode::Home => {
            state.move_input_cursor_home();
            TuiEventOutcome::Continue
        }
        KeyCode::End => {
            state.move_input_cursor_end();
            TuiEventOutcome::Continue
        }
        KeyCode::Backspace => {
            state.backspace_input();
            TuiEventOutcome::Continue
        }
        KeyCode::Delete => {
            state.delete_input_char();
            TuiEventOutcome::Continue
        }
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
            state.insert_input_char('\n');
            TuiEventOutcome::Continue
        }
        KeyCode::Tab if state.input.starts_with('/') => {
            if let Some(spec) = crate::first_match(&state.input) {
                let target = if let Some(arg) = spec.arg_hint {
                    format!("{} {arg}", spec.name)
                } else {
                    spec.name.to_string()
                };
                state.input = target;
                state.input_cursor = state.input.chars().count();
            }
            TuiEventOutcome::Continue
        }
        KeyCode::Enter => submit_input(state),
        KeyCode::Char(character)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            state.insert_input_char(character);
            TuiEventOutcome::Continue
        }
        _ => TuiEventOutcome::Continue,
    }
}

pub(super) fn handle_menu_key_event(state: &mut TuiState, key: KeyEvent) -> TuiEventOutcome {
    let Some(menu) = state.menu.as_mut() else {
        return TuiEventOutcome::Continue;
    };
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            state.menu = None;
            TuiEventOutcome::Continue
        }
        KeyCode::Up => {
            menu.selected_index = menu.selected_index.saturating_sub(1);
            TuiEventOutcome::Continue
        }
        KeyCode::Down => {
            menu.selected_index = (menu.selected_index + 1).min(menu.options.len() - 1);
            TuiEventOutcome::Continue
        }
        KeyCode::Enter => {
            let selected = menu
                .options
                .get(menu.selected_index)
                .cloned()
                .unwrap_or_default();
            state.menu = None;
            if selected == "Quit" {
                TuiEventOutcome::Quit
            } else if selected == "Debug" {
                state.debug_view = !state.debug_view;
                let label = if state.debug_view { "on" } else { "off" };
                state.push_transcript_entry(TranscriptKind::Notice, format!("debug: {label}"));
                TuiEventOutcome::Continue
            } else {
                state.push_transcript_entry(TranscriptKind::Notice, format!("menu: {selected}"));
                TuiEventOutcome::Continue
            }
        }
        _ => TuiEventOutcome::Continue,
    }
}

pub(super) fn handle_ask_user_key_event(state: &mut TuiState, key: KeyEvent) -> TuiEventOutcome {
    let Some(panel) = state.ask_user.as_mut() else {
        return TuiEventOutcome::Continue;
    };
    match key.code {
        KeyCode::Esc => {
            state.ask_user = None;
            TuiEventOutcome::Continue
        }
        KeyCode::Up => {
            panel.selected_index = panel.selected_index.saturating_sub(1);
            TuiEventOutcome::Continue
        }
        KeyCode::Down => {
            if !panel.choices.is_empty() {
                panel.selected_index = (panel.selected_index + 1).min(panel.choices.len() - 1);
            }
            TuiEventOutcome::Continue
        }
        KeyCode::Backspace if panel.choices.is_empty() => {
            panel.freeform.pop();
            TuiEventOutcome::Continue
        }
        KeyCode::Char('h')
            if key.modifiers.contains(KeyModifiers::CONTROL) && panel.choices.is_empty() =>
        {
            panel.freeform.pop();
            TuiEventOutcome::Continue
        }
        KeyCode::Char(character) if panel.choices.is_empty() => {
            panel.freeform.push(character);
            TuiEventOutcome::Continue
        }
        KeyCode::Char('?') if panel.explain_index.is_some() => {
            panel.showing_explanation = !panel.showing_explanation;
            TuiEventOutcome::Continue
        }
        KeyCode::Enter => {
            if panel.explain_index == Some(panel.selected_index) {
                panel.showing_explanation = !panel.showing_explanation;
                return TuiEventOutcome::Continue;
            }
            if panel.other_index == Some(panel.selected_index) {
                panel.enter_other_mode();
                return TuiEventOutcome::Continue;
            }
            let question = panel.question.clone();
            let answer = panel.selected_answer();
            state.ask_user = None;
            state.push_transcript_entry(
                TranscriptKind::Assistant,
                format!("ask_user: {question} -> {answer}"),
            );
            TuiEventOutcome::Continue
        }
        _ => TuiEventOutcome::Continue,
    }
}

pub(super) fn handle_approval_key_event(state: &mut TuiState, key: KeyEvent) -> TuiEventOutcome {
    let Some(panel) = state.approval.as_mut() else {
        return TuiEventOutcome::Continue;
    };
    match key.code {
        KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('d') => {
            let tool_name = panel.tool_name.clone();
            let reason = panel.reason.clone();
            state.record_approval_decision(ApprovalDecision::Deny);
            TuiEventOutcome::Approval {
                decision: ApprovalDecision::Deny,
                scope: ApprovalScope::Once,
                tool_name,
                reason,
            }
        }
        KeyCode::Char('y') | KeyCode::Char('a') => {
            let tool_name = panel.tool_name.clone();
            let reason = panel.reason.clone();
            state.record_approval_decision(ApprovalDecision::Approve);
            TuiEventOutcome::Approval {
                decision: ApprovalDecision::Approve,
                scope: ApprovalScope::Once,
                tool_name,
                reason,
            }
        }
        KeyCode::Up => {
            panel.selected_index = panel.selected_index.saturating_sub(1);
            TuiEventOutcome::Continue
        }
        KeyCode::Down => {
            panel.selected_index = (panel.selected_index + 1).min(panel.choices().len() - 1);
            TuiEventOutcome::Continue
        }
        KeyCode::Enter => {
            let (decision, scope) = panel.selected_decision();
            let tool_name = panel.tool_name.clone();
            let reason = panel.reason.clone();
            state.record_approval_decision(decision.clone());
            TuiEventOutcome::Approval {
                decision,
                scope,
                tool_name,
                reason,
            }
        }
        _ => TuiEventOutcome::Continue,
    }
}

pub(super) fn submit_input(state: &mut TuiState) -> TuiEventOutcome {
    let input = state.input.trim().to_string();
    state.clear_input();
    if input.is_empty() {
        return TuiEventOutcome::Continue;
    }
    state.record_input_history(&input);
    if input == "/quit" || input == "/exit" {
        return TuiEventOutcome::Quit;
    }
    state.push_user(input.clone());
    if let Some(command) = parse_slash_command(&input) {
        apply_slash_command(state, command);
        return TuiEventOutcome::Continue;
    }
    if input.starts_with('/') {
        state.push_error(format!("unknown command: {input}"));
        state.push_notice("type /help for available commands");
        return TuiEventOutcome::Continue;
    }
    if state.is_agent_busy() {
        let locale = state.config.language;
        let current = state
            .last_task
            .as_deref()
            .map(|task| format!("{} {task}", crate::tr(PhraseKey::NoticeRunning, locale)))
            .unwrap_or_else(|| crate::tr(PhraseKey::NoticeRunningGeneric, locale).to_string());
        state.input_queue.push(input);
        let queued = state.input_queue.len();
        state.push_transcript_entry(
            TranscriptKind::Notice,
            format!(
                "{} (#{queued}) — {current}",
                crate::tr(PhraseKey::NoticeQueued, locale)
            ),
        );
        return TuiEventOutcome::Continue;
    }
    TuiEventOutcome::Submit(input)
}

/// Dispatches the next queued input when the agent is idle.
pub(super) fn drain_input_queue<F>(state: &mut TuiState, on_submit: &mut F)
where
    F: FnMut(String, &mut TuiState),
{
    if state.input_queue.is_empty() {
        return;
    }
    if state.is_agent_busy() {
        return;
    }
    let task = state.input_queue.remove(0);
    state.agent_run_status = AgentRunStatus::Running;
    on_submit(task, state);
}

pub(super) fn apply_slash_command(state: &mut TuiState, command: SlashCommand) {
    match command {
        SlashCommand::Plan => {
            record_mode_switch(state, ExecutionMode::Plan);
            state.header.mode = ExecutionMode::Plan;
            state.push_transcript_entry(TranscriptKind::Notice, "mode: plan");
        }
        SlashCommand::Execute => {
            record_mode_switch(state, ExecutionMode::Execute);
            state.header.mode = ExecutionMode::Execute;
            state.push_transcript_entry(TranscriptKind::Notice, "mode: execute");
        }
        SlashCommand::GoalStart(goal) => {
            record_mode_switch(state, ExecutionMode::Goal);
            state.header.mode = ExecutionMode::Goal;
            state.goal_status = Some(GoalStatus::Running);
            state.side_panel.plan.push(PlanStep {
                label: goal.clone(),
                done: false,
            });
            state.push_transcript(format!("goal: {goal}"));
        }
        SlashCommand::GoalPause => {
            state.goal_status = Some(GoalStatus::Paused);
            state.push_transcript("goal: paused");
        }
        SlashCommand::GoalResume => {
            state.goal_status = Some(GoalStatus::Running);
            state.push_transcript("goal: resumed");
        }
        SlashCommand::GoalClear => {
            state.goal_status = Some(GoalStatus::Cleared);
            state.side_panel.plan.clear();
            state.push_transcript("goal: cleared");
        }
        SlashCommand::GoalStatus => {
            let done = state
                .side_panel
                .plan
                .iter()
                .filter(|step| step.done)
                .count();
            state.push_transcript(format!(
                "goal: {} {done}/{} steps done",
                goal_status_label(state.goal_status.as_ref()),
                state.side_panel.plan.len()
            ));
        }
        SlashCommand::Safe => {
            record_permission_switch(state, PermissionMode::Safe);
            state.header.permission = PermissionMode::Safe;
            state.push_transcript("permission: safe");
        }
        SlashCommand::Auto => {
            record_permission_switch(state, PermissionMode::Auto);
            state.header.permission = PermissionMode::Auto;
            state.push_transcript("permission: auto");
        }
        SlashCommand::Yolo => {
            record_permission_switch(state, PermissionMode::Yolo);
            state.header.permission = PermissionMode::Yolo;
            state.push_transcript("permission: yolo");
        }
        SlashCommand::Clear => {
            state.transcript.clear();
        }
        SlashCommand::Help => {
            state.push_transcript("commands: /plan /execute /goal <objective> /goal pause|resume|clear|status /safe /auto /yolo /model <name> /cost /plan show /clear /compact /session save /diff /undo /help");
        }
        SlashCommand::Cost => {
            state.push_transcript(format!(
                "cost: ${:.4}, tokens: {}, cache: {:.0}%",
                state.header.cost_usd,
                state.header.total_tokens,
                state.header.cache_hit_rate * 100.0
            ));
        }
        SlashCommand::PlanShow => {
            if state.side_panel.plan.is_empty() {
                state.push_transcript("plan: <empty>");
            } else {
                let done = state
                    .side_panel
                    .plan
                    .iter()
                    .filter(|step| step.done)
                    .count();
                state.push_transcript(format!(
                    "plan: {done}/{} steps",
                    state.side_panel.plan.len()
                ));
                for (index, step) in state.side_panel.plan.clone().iter().enumerate() {
                    let marker = if step.done { "[x]" } else { "[ ]" };
                    state.push_transcript(format!("{marker} {}. {}", index + 1, step.label));
                }
            }
        }
        SlashCommand::Model(model) => {
            let from = state.header.model.clone();
            state.header.model = model.clone();
            state.push_transcript(format!("model: {from} -> {model}"));
        }
        SlashCommand::Lang(locale) => {
            state.config.language = locale;
            state.push_transcript(format!("lang: {locale}"));
        }
        SlashCommand::Compact => {
            state.push_transcript("compact: queued for next agent turn");
        }
        SlashCommand::SessionSave => {
            state.push_transcript("session: save requested");
        }
        SlashCommand::Diff => {
            state.push_transcript("diff: use the agent run stream for tool-backed diff output");
        }
        SlashCommand::Undo => {
            state.push_transcript(
                "undo: requires tool approval in a run; ask Peridot to undo the last change",
            );
        }
    }
}

pub(super) fn record_mode_switch(state: &mut TuiState, to: ExecutionMode) {
    if state.header.mode == to {
        return;
    }
    state.lifecycle_events.push(TuiLifecycleEvent {
        event: "mode_switch".to_string(),
        from: state.header.mode.to_string(),
        to: to.to_string(),
    });
}

pub(super) fn record_permission_switch(state: &mut TuiState, to: PermissionMode) {
    if state.header.permission == to {
        return;
    }
    state.lifecycle_events.push(TuiLifecycleEvent {
        event: "permission_switch".to_string(),
        from: state.header.permission.to_string(),
        to: to.to_string(),
    });
}
