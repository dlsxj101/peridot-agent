use super::*;

/// Runs the interactive terminal UI until the user quits or submits a task.
pub fn run_interactive(mut state: TuiState) -> io::Result<TuiExit> {
    let mut terminal = TerminalGuard::enter()?;
    let (width, height) = terminal_size()?;
    state.resize(width, height);
    let submitted = loop {
        terminal.terminal.draw(|frame| draw(frame, &state))?;
        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) => match handle_key_event(&mut state, key) {
                    TuiEventOutcome::Continue => {}
                    TuiEventOutcome::Quit => break None,
                    TuiEventOutcome::Submit(task) => break Some(task),
                },
                Event::Resize(width, height) => state.resize(width, height),
                _ => {}
            }
        }
    };
    Ok(TuiExit { state, submitted })
}

/// Applies a keyboard event to the TUI state.
pub fn handle_key_event(state: &mut TuiState, key: KeyEvent) -> TuiEventOutcome {
    if state.menu.is_some() {
        return handle_menu_key_event(state, key);
    }
    if state.ask_user.is_some() {
        return handle_ask_user_key_event(state, key);
    }
    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            TuiEventOutcome::Quit
        }
        KeyCode::Esc => {
            state.menu = Some(MenuState::default());
            TuiEventOutcome::Continue
        }
        KeyCode::Backspace => {
            state.input.pop();
            TuiEventOutcome::Continue
        }
        KeyCode::Enter => submit_input(state),
        KeyCode::Char(character) => {
            state.input.push(character);
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
            } else {
                state.push_transcript(format!("menu: {selected}"));
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
            state.push_transcript(format!("ask_user: {question} -> {answer}"));
            TuiEventOutcome::Continue
        }
        _ => TuiEventOutcome::Continue,
    }
}

pub(super) fn submit_input(state: &mut TuiState) -> TuiEventOutcome {
    let input = state.input.trim().to_string();
    state.input.clear();
    if input.is_empty() {
        return TuiEventOutcome::Continue;
    }
    if input == "/quit" || input == "/exit" {
        return TuiEventOutcome::Quit;
    }
    state.push_transcript(format!("> {input}"));
    if let Some(command) = parse_slash_command(&input) {
        apply_slash_command(state, command);
        return TuiEventOutcome::Continue;
    }
    TuiEventOutcome::Submit(input)
}

pub(super) fn apply_slash_command(state: &mut TuiState, command: SlashCommand) {
    match command {
        SlashCommand::Plan => {
            record_mode_switch(state, ExecutionMode::Plan);
            state.header.mode = ExecutionMode::Plan;
            state.push_transcript("mode: plan");
        }
        SlashCommand::Execute => {
            record_mode_switch(state, ExecutionMode::Execute);
            state.header.mode = ExecutionMode::Execute;
            state.push_transcript("mode: execute");
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
