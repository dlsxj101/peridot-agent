use super::*;
use state::{AgentRunStatus, SessionCommandEvent, TranscriptKind};

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
///
/// `runtime_events` carries `(session_id, event)` tuples — the foreground
/// session feeds the main transcript while other ids only update
/// [`SessionDirectoryItem`](crate::SessionDirectoryItem) counters via
/// [`TuiState::record_background_event`].
///
/// `on_session_command` is invoked whenever a slash command queues a
/// [`SessionCommandEvent`]; the host translates it into a real
/// `SessionRouter` mutation.
///
/// `on_persist` is called on every tick after state has been updated, giving
/// the host a chance to throttle and serialise `TuiState` to disk so a crash
/// or `Ctrl+C` does not lose the current session. The mutable handle also
/// lets the host drain queues such as `pending_notes` after writing them.
#[allow(clippy::too_many_arguments)]
pub fn run_interactive_with_events<F>(
    mut state: TuiState,
    runtime_events: std::sync::mpsc::Receiver<(String, TuiRuntimeEvent)>,
    mut on_submit: F,
    mut on_approval: impl FnMut(ApprovalDecision, ApprovalScope, String, String, &mut TuiState),
    mut on_interrupt: impl FnMut(&mut TuiState),
    mut on_session_command: impl FnMut(SessionCommandEvent, &mut TuiState),
    mut on_persist: impl FnMut(&mut TuiState),
) -> io::Result<TuiExit>
where
    F: FnMut(String, &mut TuiState),
{
    let mut terminal = TerminalGuard::enter()?;
    let (width, height) = terminal_size()?;
    state.resize(width, height);
    let mut other_states: std::collections::HashMap<String, TuiState> =
        std::collections::HashMap::new();
    let mut last_foreground = state.current_session_id.clone();
    loop {
        for (session_id, event) in runtime_events.try_iter() {
            if state.current_session_id.is_empty() || session_id == state.current_session_id {
                state.apply_runtime_event(event);
            } else {
                state.record_background_event(&session_id, &event);
                if let Some(other) = other_states.get_mut(&session_id) {
                    other.apply_runtime_event(event);
                }
            }
        }
        let pending = state.drain_pending_session_commands();
        for cmd in pending {
            on_session_command(cmd, &mut state);
        }
        if state.current_session_id != last_foreground
            && !state.current_session_id.is_empty()
            && !last_foreground.is_empty()
        {
            swap_foreground_state(&mut state, &mut other_states, &last_foreground);
            last_foreground = state.current_session_id.clone();
        }
        drain_input_queue(&mut state, &mut on_submit);
        state.tick_spinner();
        terminal.terminal.draw(|frame| draw(frame, &state))?;
        on_persist(&mut state);
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
    on_persist(&mut state);
    Ok(TuiExit {
        state,
        submitted: None,
    })
}

/// Hot-swaps `state` so that its transcript/header/plan match the new
/// `state.current_session_id`. The previous foreground's contents are stashed
/// in `other_states` under `previous_id`; if the target session was seen
/// before, its state is restored, otherwise a fresh state inherits the
/// header / config / sessions directory from the master view. Called from the
/// main loop the moment `current_session_id` diverges from the foreground we
/// last drew.
pub(super) fn swap_foreground_state(
    state: &mut TuiState,
    other_states: &mut std::collections::HashMap<String, TuiState>,
    previous_id: &str,
) {
    let target_id = state.current_session_id.clone();
    if target_id.is_empty() || previous_id.is_empty() || target_id == previous_id {
        return;
    }
    let mut new_state = other_states.remove(&target_id).unwrap_or_else(|| {
        let mut fresh = TuiState::new(state.header.clone()).with_config(state.config.clone());
        fresh.current_session_id = target_id.clone();
        fresh
    });
    new_state.sessions = state.sessions.clone();
    new_state.current_session_id = target_id.clone();
    new_state.layout = state.layout.clone();
    let mut saved = std::mem::replace(state, new_state);
    saved.current_session_id = previous_id.to_string();
    other_states.insert(previous_id.to_string(), saved);
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
        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            crate::cycle_foreground(state);
            TuiEventOutcome::Continue
        }
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            crate::cycle_foreground(state);
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
        KeyCode::Enter
            if key.modifiers.contains(KeyModifiers::SHIFT) && panel.choices.is_empty() =>
        {
            panel.freeform.push('\n');
            TuiEventOutcome::Continue
        }
        KeyCode::Char('j')
            if key.modifiers.contains(KeyModifiers::CONTROL) && panel.choices.is_empty() =>
        {
            panel.freeform.push('\n');
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
            let provider = state.header.provider.as_deref().unwrap_or("default");
            state.push_transcript(format!(
                "cost: ${:.4} · tokens: {} · cache: {:.0}% · model: {} · provider: {} · turn: {}",
                state.header.cost_usd,
                state.header.total_tokens,
                state.header.cache_hit_rate * 100.0,
                state.header.model,
                provider,
                state.current_turn,
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
        SlashCommand::Provider(provider) => {
            let from = state.header.provider.clone().unwrap_or_default();
            state.header.provider = Some(provider.clone());
            if from.is_empty() {
                state.push_transcript(format!("provider: {provider}"));
            } else {
                state.push_transcript(format!("provider: {from} -> {provider}"));
            }
        }
        SlashCommand::Note(text) => {
            let body = text.trim();
            if body.is_empty() {
                state.push_error("note: text must not be empty");
            } else {
                state.push_pending_note(body.to_string());
                state.push_transcript(format!("note: {body}"));
            }
        }
        SlashCommand::Info => {
            let session_id = if state.current_session_id.is_empty() {
                "<none>".to_string()
            } else {
                state.current_session_id.clone()
            };
            let workspace = state
                .header
                .workspace_label
                .as_deref()
                .unwrap_or("<unknown>");
            let provider = state.header.provider.as_deref().unwrap_or("default");
            state.push_transcript(format!(
                "info: session {} · workspace {} · model {} · provider {} · mode {} · permission {} · turn {} · tokens {} · cost ${:.4}",
                session_id,
                workspace,
                state.header.model,
                provider,
                state.header.mode,
                state.header.permission,
                state.current_turn,
                state.header.total_tokens,
                state.header.cost_usd,
            ));
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
        SlashCommand::Fork(task) => {
            state.push_transcript(format!("fork: {task} — spawning"));
            state.push_pending_session_command(SessionCommandEvent::Fork(task));
        }
        SlashCommand::Teammate(task) => {
            state.push_transcript(format!("teammate: {task} — spawning worktree"));
            state.push_pending_session_command(SessionCommandEvent::Teammate(task));
        }
        SlashCommand::Worktree { branch, task } => {
            state.push_transcript(format!("worktree: {task} on branch {branch} — spawning"));
            state.push_pending_session_command(SessionCommandEvent::Worktree { branch, task });
        }
        SlashCommand::SessionNew(task) => {
            let suffix = task
                .as_deref()
                .map(|task| format!(" with task '{task}'"))
                .unwrap_or_default();
            state.push_transcript(format!("session: opening new session{suffix}"));
            state.push_pending_session_command(SessionCommandEvent::SessionNew(task));
        }
        SlashCommand::SessionSwitch(target) => {
            state.push_transcript(format!("session: switching to {target}"));
            state.push_pending_session_command(SessionCommandEvent::SessionSwitch(target));
        }
        SlashCommand::SessionClose(target) => {
            state.push_transcript(format!("session: closing {target}"));
            state.push_pending_session_command(SessionCommandEvent::SessionClose(target));
        }
        SlashCommand::SessionList => {
            if state.sessions.is_empty() {
                state.push_transcript("sessions: <none>");
            } else {
                let summary = state
                    .sessions
                    .iter()
                    .map(|item| {
                        let marker = if item.id == state.current_session_id {
                            "*"
                        } else {
                            " "
                        };
                        format!("{marker} {} ({})", item.title, item.id)
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                state.push_transcript(format!("sessions:\n{summary}"));
            }
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
