use super::*;
use state::{AgentRunStatus, SessionCommandEvent, TranscriptKind};

/// PageUp / PageDown jump distance, measured in transcript rows. Small enough
/// to avoid skipping past important context, large enough to traverse a long
/// transcript without dozens of keypresses. Mouse-wheel scrolling is
/// intentionally not supported: we leave mouse capture off so the operator
/// can drag-select transcript text to copy.
const PAGE_SCROLL_STEP: usize = 10;

/// Runs the interactive terminal UI until the user quits or submits a task.
pub fn run_interactive(mut state: TuiState) -> io::Result<TuiExit> {
    let mut terminal = TerminalGuard::enter()?;
    let (width, height) = terminal_size()?;
    state.resize(width, height);
    let mut ctrl_c_armed = false;
    let submitted = loop {
        // See the comment in `run_interactive_with_events` — child
        // processes can disable our raw mode, and re-asserting it
        // here is the cheapest place to recover.
        if let Ok(false) = crossterm::terminal::is_raw_mode_enabled() {
            let _ = enable_raw_mode();
        }
        state.tick_spinner();
        terminal.terminal.draw(|frame| draw(frame, &state))?;
        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if is_ctrl_c(key) {
                        if handle_ctrl_c_quit_confirmation(&mut state, key, &mut ctrl_c_armed) {
                            break None;
                        }
                        continue;
                    }
                    ctrl_c_armed = false;
                    match handle_key_event(&mut state, key) {
                        TuiEventOutcome::Continue => {}
                        TuiEventOutcome::Quit => break None,
                        TuiEventOutcome::Submit(task) => break Some(task),
                        TuiEventOutcome::Approval { .. }
                        | TuiEventOutcome::AskUserResolved { .. }
                        | TuiEventOutcome::Interrupt => {}
                    }
                }
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
    mut on_approval: impl FnMut(
        ApprovalDecision,
        ApprovalScope,
        String,
        String,
        serde_json::Value,
        Option<serde_json::Value>,
        &mut TuiState,
    ),
    mut on_ask_user_resolved: impl FnMut(String, AskUserAnswer, &mut TuiState),
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
    let mut ctrl_c_armed = false;
    loop {
        // Re-assert raw mode every tick. Child processes spawned by
        // `shell_exec` (npm, vite, spinner libraries, …) can reach
        // the controlling terminal directly and reset its termios on
        // exit, which leaves the TUI receiving raw escape sequences
        // (`[A`, `[B`, `[5~`) in the textarea instead of typed key
        // events. `enable_raw_mode` is idempotent — a single ioctl
        // call when already enabled — so the cost is negligible.
        if let Ok(false) = crossterm::terminal::is_raw_mode_enabled() {
            let _ = enable_raw_mode();
        }
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
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if is_ctrl_c(key) {
                        if handle_ctrl_c_quit_confirmation(&mut state, key, &mut ctrl_c_armed) {
                            break;
                        }
                        continue;
                    }
                    ctrl_c_armed = false;
                    match handle_key_event(&mut state, key) {
                        TuiEventOutcome::Continue => {}
                        TuiEventOutcome::Quit => break,
                        TuiEventOutcome::Submit(task) => on_submit(task, &mut state),
                        TuiEventOutcome::Approval {
                            decision,
                            scope,
                            tool_name,
                            reason,
                            parameters,
                            synthesised_parameters,
                        } => on_approval(
                            decision,
                            scope,
                            tool_name,
                            reason,
                            parameters,
                            synthesised_parameters,
                            &mut state,
                        ),
                        TuiEventOutcome::AskUserResolved { request_id, answer } => {
                            on_ask_user_resolved(request_id, answer, &mut state)
                        }
                        TuiEventOutcome::Interrupt => on_interrupt(&mut state),
                    }
                }
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

fn is_ctrl_c(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL))
}

pub(super) fn handle_ctrl_c_quit_confirmation(
    state: &mut TuiState,
    key: KeyEvent,
    armed: &mut bool,
) -> bool {
    if !is_ctrl_c(key) {
        return false;
    }
    if *armed {
        return true;
    }
    *armed = true;
    state.push_notice("press Ctrl+C again to quit");
    false
}

/// Hot-swaps `state` so that its transcript/header/plan/input history match
/// the new `state.current_session_id`. The previous foreground's contents are
/// stashed in `other_states` under `previous_id`; if the target session was
/// seen before, its state is restored, otherwise a fresh state inherits the
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
    new_state.hydrate_note_summary_from_directory();
    new_state.layout = state.layout.clone();
    let mut saved = std::mem::replace(state, new_state);
    saved.current_session_id = previous_id.to_string();
    other_states.insert(previous_id.to_string(), saved);
}

/// Applies a keyboard event to the TUI state.
pub fn handle_key_event(state: &mut TuiState, key: KeyEvent) -> TuiEventOutcome {
    if key.kind != KeyEventKind::Press {
        return TuiEventOutcome::Continue;
    }
    if state.menu.is_some() {
        return handle_menu_key_event(state, key);
    }
    if state.approval.is_some() {
        return handle_approval_key_event(state, key);
    }
    if state.session_picker.is_some() {
        return handle_session_picker_key_event(state, key);
    }
    if state.branch_picker.is_some() {
        return handle_branch_picker_key_event(state, key);
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
        // Side panel toggle — three accepted shortcuts:
        //   Ctrl+]  — historical chord (kept for terminals that deliver it
        //             cleanly: iTerm2, Linux consoles, kitty, WezTerm…).
        //   F2      — terminal-agnostic fallback. WSL conpty / Windows
        //             Terminal sometimes swallows Ctrl+]; function keys are
        //             always reported correctly.
        //   /sidepanel slash command — discoverable via the slash picker.
        KeyCode::Char(']') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            toggle_sidepanel(state);
            TuiEventOutcome::Continue
        }
        KeyCode::F(2) => {
            toggle_sidepanel(state);
            TuiEventOutcome::Continue
        }
        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if state.sessions.len() > 1 {
                state.session_picker = Some(crate::SessionPickerState::opening());
            }
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
        // Shift+Up/Down scrolls the transcript. We need this fallback because
        // some terminals (notably Windows Terminal in certain WSL configs)
        // translate the mouse wheel into bare Up/Down arrow sequences even
        // with mouse capture enabled, which would otherwise cycle the input
        // history every time the user scrolls. Shift+arrow gives the operator
        // a way to navigate the transcript without fighting the terminal.
        KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => {
            state.scroll_up(1);
            TuiEventOutcome::Continue
        }
        KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
            state.scroll_down(1);
            TuiEventOutcome::Continue
        }
        // `@file` picker takes priority over input history when open —
        // Up/Down navigates the suggestion list so the operator can land
        // on a non-first match without leaving the keyboard.
        KeyCode::Up if state.at_picker.is_some() => {
            if let Some(picker) = state.at_picker.as_mut() {
                picker.selected = picker.selected.saturating_sub(1);
            }
            TuiEventOutcome::Continue
        }
        KeyCode::Down if state.at_picker.is_some() => {
            if let Some(picker) = state.at_picker.as_mut() {
                let matches = crate::at_picker::filter_paths(&state.at_picker_index, &picker.query);
                if !matches.is_empty() {
                    picker.selected = (picker.selected + 1).min(matches.len() - 1);
                }
            }
            TuiEventOutcome::Continue
        }
        KeyCode::Up if state.slash_picker.is_some() => {
            state.move_slash_picker_selection(-1);
            TuiEventOutcome::Continue
        }
        KeyCode::Down if state.slash_picker.is_some() => {
            state.move_slash_picker_selection(1);
            TuiEventOutcome::Continue
        }
        // For multi-line drafts (input contains `\n`) the arrow keys move
        // the cursor between logical lines first; history navigation only
        // kicks in when the cursor is already at the very top or bottom
        // line. Single-line inputs fall straight through to history so
        // existing muscle memory still works for chat replies.
        KeyCode::Up => {
            if !try_move_cursor_up(state) {
                state.previous_input_history();
            }
            TuiEventOutcome::Continue
        }
        KeyCode::Down => {
            if !try_move_cursor_down(state) {
                state.next_input_history();
            }
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
        KeyCode::PageUp => {
            state.scroll_up(PAGE_SCROLL_STEP);
            TuiEventOutcome::Continue
        }
        KeyCode::PageDown => {
            state.scroll_down(PAGE_SCROLL_STEP);
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
        // Multi-line input. Three accepted shapes, in order of how reliable
        // each is across terminals:
        //   - Ctrl+J     — bare LF code point, works everywhere (emacs/readline
        //                  convention). Recommended default.
        //   - Alt+Enter  — works on most terminals (Windows Terminal, iTerm,
        //                  gnome-terminal, etc).
        //   - Shift+Enter — only fires when the host terminal already speaks
        //                  CSI-u natively (kitty, WezTerm, recent xterm). We
        //                  intentionally do NOT push the protocol from the
        //                  app because doing so broke `Ctrl+]` on Windows
        //                  Terminal under WSL.
        KeyCode::Enter
            if key
                .modifiers
                .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) =>
        {
            state.insert_input_char('\n');
            TuiEventOutcome::Continue
        }
        KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.insert_input_char('\n');
            TuiEventOutcome::Continue
        }
        // Tab completes the highlighted `@file` suggestion when the picker
        // is active; otherwise falls through to the slash-command picker.
        KeyCode::Tab if state.at_picker.is_some() => {
            state.accept_at_picker();
            TuiEventOutcome::Continue
        }
        KeyCode::Tab if state.slash_picker.is_some() => {
            state.accept_slash_picker();
            TuiEventOutcome::Continue
        }
        KeyCode::Enter
            if state.slash_picker.is_some()
                && !state.slash_picker_exact_selection_is_runnable() =>
        {
            state.accept_slash_picker();
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
            apply_menu_selection(state, &selected)
        }
        _ => TuiEventOutcome::Continue,
    }
}

/// Routes keystrokes while the session picker overlay is open.
/// `↑`/`↓` move the selection, normal text edits the prefix filter,
/// `Enter` switches foreground, and `Esc` cancels.
pub(super) fn handle_session_picker_key_event(
    state: &mut TuiState,
    key: KeyEvent,
) -> TuiEventOutcome {
    let Some(picker) = state.session_picker.as_mut() else {
        return TuiEventOutcome::Continue;
    };
    match key.code {
        KeyCode::Up => {
            let count =
                crate::session_picker::filtered_sessions(&state.sessions, &picker.query).len();
            picker.move_selection(-1, count);
            TuiEventOutcome::Continue
        }
        KeyCode::Down => {
            let count =
                crate::session_picker::filtered_sessions(&state.sessions, &picker.query).len();
            picker.move_selection(1, count);
            TuiEventOutcome::Continue
        }
        KeyCode::Backspace => {
            picker.backspace_query();
            TuiEventOutcome::Continue
        }
        KeyCode::Enter => {
            let target = picker.selected_session_id(&state.sessions);
            state.session_picker = None;
            if let Some(id) = target {
                state.current_session_id = id.clone();
                if let Some(item) = state.sessions.iter_mut().find(|item| item.id == id) {
                    item.pending_attention = false;
                }
            }
            TuiEventOutcome::Continue
        }
        KeyCode::Esc => {
            state.session_picker = None;
            TuiEventOutcome::Continue
        }
        KeyCode::Char(character)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            picker.push_query_char(character);
            TuiEventOutcome::Continue
        }
        _ => TuiEventOutcome::Continue,
    }
}

/// Routes keystrokes while the branch picker overlay is open.
/// `↑`/`↓` move the selection, `Enter` commits the chosen turn id
/// (queues `SessionCommandEvent::BranchTurn`), `q` / `Esc` cancels.
pub(super) fn handle_branch_picker_key_event(
    state: &mut TuiState,
    key: KeyEvent,
) -> TuiEventOutcome {
    let Some(picker) = state.branch_picker.as_mut() else {
        return TuiEventOutcome::Continue;
    };
    match key.code {
        KeyCode::Up => {
            picker.move_selection(-1);
            TuiEventOutcome::Continue
        }
        KeyCode::Down => {
            picker.move_selection(1);
            TuiEventOutcome::Continue
        }
        KeyCode::Enter => {
            let turn_id = picker.selected_turn_id();
            state.branch_picker = None;
            match turn_id {
                Some(id) => {
                    state.push_transcript(format!("branch: forking at turn {id}…"));
                    state.push_pending_session_command(SessionCommandEvent::BranchTurn(id));
                }
                None => {
                    state.push_error("branch: nothing to fork from".to_string());
                }
            }
            TuiEventOutcome::Continue
        }
        KeyCode::Esc | KeyCode::Char('q') => {
            state.branch_picker = None;
            state.push_transcript("branch: cancelled".to_string());
            TuiEventOutcome::Continue
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
            let request_id = panel.request_id.clone();
            state.ask_user = None;
            // When the panel is tied to an `AskUserPort` request, surface
            // the cancel so the CLI can fall back to the synthesised
            // default; otherwise just close the panel.
            if let Some(request_id) = request_id {
                return TuiEventOutcome::AskUserResolved {
                    request_id,
                    answer: AskUserAnswer::Cancelled,
                };
            }
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
        // Space toggles the highlighted choice in multi-select mode.
        // Single-select / free-form panels treat Space as a regular char,
        // but we only reach this arm when `panel.choices` is non-empty
        // (the free-form Char arm earlier consumed Space for typing).
        KeyCode::Char(' ') if panel.multi_select => {
            panel.toggle_selected();
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
            let answer_text = panel.selected_answer();
            let request_id = panel.request_id.clone();
            let structured = panel.structured_answer();
            state.ask_user = None;
            state.push_transcript_entry(
                TranscriptKind::Assistant,
                format!("ask_user: {question} -> {answer_text}"),
            );
            if let (Some(request_id), Some(answer)) = (request_id, structured) {
                return TuiEventOutcome::AskUserResolved { request_id, answer };
            }
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
            let parameters = panel.tool_params.clone();
            state.record_approval_decision(ApprovalDecision::Deny);
            TuiEventOutcome::Approval {
                decision: ApprovalDecision::Deny,
                scope: ApprovalScope::Once,
                tool_name,
                reason,
                parameters,
                synthesised_parameters: None,
            }
        }
        KeyCode::Char('y') | KeyCode::Char('a') => {
            let synthesised = panel_synthesised_parameters(panel);
            let tool_name = panel.tool_name.clone();
            let reason = panel.reason.clone();
            let parameters = panel.tool_params.clone();
            state.record_approval_decision(ApprovalDecision::Approve);
            TuiEventOutcome::Approval {
                decision: ApprovalDecision::Approve,
                scope: ApprovalScope::Once,
                tool_name,
                reason,
                parameters,
                synthesised_parameters: synthesised,
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
        // Per-hunk staging: ←/→ moves the focused hunk, Tab/Space toggles
        // the focused hunk's acceptance flag. No-op when no hunks present.
        KeyCode::Left => {
            panel.move_hunk_focus(-1);
            TuiEventOutcome::Continue
        }
        KeyCode::Right => {
            panel.move_hunk_focus(1);
            TuiEventOutcome::Continue
        }
        KeyCode::Tab | KeyCode::Char(' ') => {
            panel.toggle_focused_hunk();
            TuiEventOutcome::Continue
        }
        KeyCode::Enter => {
            let (decision, scope) = panel.selected_decision();
            let synthesised = if decision == ApprovalDecision::Approve {
                panel_synthesised_parameters(panel)
            } else {
                None
            };
            let tool_name = panel.tool_name.clone();
            let reason = panel.reason.clone();
            let parameters = panel.tool_params.clone();
            state.record_approval_decision(decision.clone());
            TuiEventOutcome::Approval {
                decision,
                scope,
                tool_name,
                reason,
                parameters,
                synthesised_parameters: synthesised,
            }
        }
        _ => TuiEventOutcome::Continue,
    }
}

/// Builds the partial-patch parameter object when the operator
/// rejected at least one hunk. Returns `None` when there are no hunks,
/// when every hunk is accepted (the original parameters still hold),
/// or when synthesis fails (missing `old_text` field).
fn panel_synthesised_parameters(panel: &ApprovalPanel) -> Option<serde_json::Value> {
    if panel.hunks.is_empty() || panel.all_hunks_accepted() {
        return None;
    }
    let partial = panel.synthesised_new_text()?;
    let mut params = panel.tool_params.clone();
    if let Some(obj) = params.as_object_mut() {
        obj.insert("new_text".to_string(), serde_json::Value::String(partial));
        Some(params)
    } else {
        None
    }
}

pub(super) fn submit_input(state: &mut TuiState) -> TuiEventOutcome {
    let input = state.input.trim().to_string();
    state.clear_input();
    if input.is_empty() {
        return TuiEventOutcome::Continue;
    }
    // Snap the view back to the tail before recording the message so the user
    // actually sees their own input — submitting from a scrolled-up state
    // would otherwise hide the new entry below the visible window.
    state.scroll_to_tail();
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
    if state.agent_run_status == AgentRunStatus::Interrupted {
        return;
    }
    if state.is_agent_busy() {
        return;
    }
    let task = state.input_queue.remove(0);
    state.agent_run_status = AgentRunStatus::Running;
    on_submit(task, state);
}

fn apply_slash_state_delta(state: &mut TuiState, delta: &peridot_core::SlashStateDelta) {
    if let Some(mode) = delta.mode {
        state.header.mode = mode;
    }
    if let Some(permission) = delta.permission {
        state.header.permission = permission;
    }
    if let Some(model) = delta.model.as_ref() {
        state.header.model = model.clone();
        state.add_model_suggestion(model);
    }
    if let Some(provider) = delta.provider.as_ref() {
        state.header.provider = Some(provider.clone());
    }
    if let Some(reasoning_effort) = delta.reasoning_effort {
        state.reasoning_effort = reasoning_effort;
    }
    if let Some(service_tier) = delta.service_tier.as_ref() {
        state.service_tier = service_tier.clone();
    }
    if let Some(committee_mode) = delta.committee_mode {
        state.committee_mode = committee_mode;
    }
    if let Some(locale) = delta.locale {
        state.config.language = locale;
    }
    if let Some(subagent_default_model) = delta.subagent_default_model.as_ref() {
        state.subagent_default_model = subagent_default_model.clone();
        if let Some(model) = subagent_default_model.as_ref() {
            state.add_model_suggestion(model);
        }
    }
}

pub(super) fn apply_slash_command(state: &mut TuiState, command: SlashCommand) {
    let previous_model = state.header.model.clone();
    let previous_provider = state.header.provider.clone();
    let previous_committee_mode = state.committee_mode;
    let previous_subagent_model = state.subagent_default_model.clone();
    let previous_reasoning_effort = state.reasoning_effort;
    let previous_service_tier = state
        .service_tier
        .clone()
        .unwrap_or_else(|| "standard".to_string());
    let delta = peridot_core::slash_state_delta(&command, state.service_tier.as_deref());
    if let Some(mode) = delta.mode {
        record_mode_switch(state, mode);
    }
    if let Some(permission) = delta.permission {
        record_permission_switch(state, permission);
    }
    apply_slash_state_delta(state, &delta);
    match command {
        SlashCommand::Plan => {
            state.push_transcript_entry(TranscriptKind::Notice, "mode: plan");
        }
        SlashCommand::Execute => {
            state.push_transcript_entry(TranscriptKind::Notice, "mode: execute");
        }
        SlashCommand::GoalMode => {
            state.push_transcript_entry(TranscriptKind::Notice, "mode: goal");
        }
        SlashCommand::GoalStart(goal) => {
            state.goal_status = Some(GoalStatus::Running);
            state.goal_text = Some(goal.clone());
            state.goal_started_at_unix = Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or_default(),
            );
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
            state.goal_text = None;
            state.goal_started_at_unix = None;
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
            state.push_transcript("permission: safe");
        }
        SlashCommand::Auto => {
            state.push_transcript("permission: auto");
        }
        SlashCommand::Yolo => {
            state.push_transcript("permission: yolo");
        }
        SlashCommand::Clear => {
            // Deep clear: visible UI surface here, agent context on
            // the host. The host wipes the running agent and opens a
            // fresh session so the next user message starts with no
            // recall of prior turns and zero token spend.
            state.reset_for_clear();
            state.push_transcript("clear: transcript + context wiped, new session");
            state.push_pending_session_command(SessionCommandEvent::ClearAndRestart);
        }
        SlashCommand::Help => {
            let mut lines: Vec<String> = Vec::new();
            lines.push("commands:".to_string());
            for spec in crate::slash_command_catalog() {
                let hint = spec
                    .arg_hint
                    .map(|hint| format!(" {hint}"))
                    .unwrap_or_default();
                lines.push(format!(
                    "  {}{hint}  ·  {} [{}]",
                    spec.name, spec.description, spec.category
                ));
            }
            if !state.skill_suggestions.is_empty() {
                lines.push("skills:".to_string());
                for skill in &state.skill_suggestions {
                    let description = if skill.description.trim().is_empty() {
                        "stored auto-skill"
                    } else {
                        skill.description.trim()
                    };
                    lines.push(format!("  /{}  ·  {} [skill]", skill.name, description));
                }
            }
            state.push_transcript(lines.join("\n"));
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
            if state.committee_mode != peridot_common::CommitteeMode::Off {
                state.push_transcript(format!(
                    "committee cost: planner ${:.4} ({} tok) · reviewer ${:.4} ({} tok)",
                    state.committee_planner_cost,
                    state.committee_planner_tokens,
                    state.committee_reviewer_cost,
                    state.committee_reviewer_tokens,
                ));
            }
            if state.sessions.len() > 1 {
                state.push_transcript(format!(
                    "aggregate: ${:.4} · {} tok across {} sessions",
                    state.aggregate_cost_usd(),
                    state.aggregate_tokens(),
                    state.sessions.len(),
                ));
            }
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
            state.push_transcript(format!("model: {previous_model} -> {model}"));
        }
        SlashCommand::Provider(provider) => {
            let from = previous_provider.unwrap_or_default();
            if from.is_empty() {
                state.push_transcript(format!("provider: {provider}"));
            } else {
                state.push_transcript(format!("provider: {from} -> {provider}"));
            }
        }
        SlashCommand::Committee(mode) => {
            state.push_transcript(format!("committee: {previous_committee_mode} -> {mode}"));
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
        SlashCommand::Notes(last) => {
            state.push_transcript("notes: loading...");
            state.push_pending_session_command(SessionCommandEvent::Notes(last));
        }
        SlashCommand::NotesClear => {
            state.push_transcript("notes: clearing...");
            state.push_pending_session_command(SessionCommandEvent::NotesClear);
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
            if state.committee_mode != peridot_common::CommitteeMode::Off {
                state.push_transcript(format!(
                    "  committee: mode {} · planner ${:.4} ({} tok) · reviewer ${:.4} ({} tok)",
                    state.committee_mode,
                    state.committee_planner_cost,
                    state.committee_planner_tokens,
                    state.committee_reviewer_cost,
                    state.committee_reviewer_tokens,
                ));
            }
        }
        SlashCommand::ContextTop => {
            state.push_pending_session_command(SessionCommandEvent::ContextTop);
        }
        SlashCommand::Lang(locale) => {
            state.push_transcript(format!("lang: {locale}"));
        }
        SlashCommand::Compact => {
            state.push_pending_session_command(SessionCommandEvent::CompactContext);
        }
        SlashCommand::SidepanelToggle => {
            toggle_sidepanel(state);
        }
        SlashCommand::Collapse => {
            state.collapse_all_tool_blocks = !state.collapse_all_tool_blocks;
            state.collapsed_blocks.clear();
            let label = if state.collapse_all_tool_blocks {
                "collapsed"
            } else {
                "expanded"
            };
            state.push_transcript(format!("transcript: tool blocks {label}"));
        }
        SlashCommand::AutoFix(action) => {
            use peridot_core::AutoFixAction;
            match action {
                AutoFixAction::On => {
                    state.auto_fix_enabled = true;
                    state.push_transcript(format!(
                        "autofix: enabled (max {} attempts)",
                        state.auto_fix_max_attempts
                    ));
                }
                AutoFixAction::Off => {
                    state.auto_fix_enabled = false;
                    state.push_transcript("autofix: disabled".to_string());
                }
                AutoFixAction::MaxAttempts(n) => {
                    state.auto_fix_enabled = true;
                    state.auto_fix_max_attempts = n;
                    state.push_transcript(format!("autofix: enabled (max {n} attempts)"));
                }
            }
        }
        SlashCommand::SessionSave => {
            state.push_transcript("session: save requested");
        }
        SlashCommand::Diff => {
            state.push_transcript("diff: use the agent run stream for tool-backed diff output");
        }
        SlashCommand::Undo => {
            state.push_pending_session_command(SessionCommandEvent::UndoLastCheckpoint);
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
        SlashCommand::SessionDelete(target) => {
            state.push_transcript(format!("session: deleting {target}"));
            state.push_pending_session_command(SessionCommandEvent::SessionDelete(target));
        }
        SlashCommand::SessionRename { target, title } => {
            state.push_transcript(format!("session: renaming {target} to {title}"));
            state
                .push_pending_session_command(SessionCommandEvent::SessionRename { target, title });
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
        SlashCommand::SessionListStatus(status) => {
            state.push_transcript(format!("sessions: loading {status} sessions..."));
            state.push_pending_session_command(SessionCommandEvent::SessionListStatus(status));
        }
        SlashCommand::SessionPrune {
            status,
            older_than_days,
            dry_run,
        } => {
            let mode = if dry_run { "dry-run" } else { "removing" };
            state.push_transcript(format!("session prune: {mode} matching sessions..."));
            state.push_pending_session_command(SessionCommandEvent::SessionPrune {
                status,
                older_than_days,
                dry_run,
            });
        }
        SlashCommand::SessionCount => {
            state.push_transcript("session count: loading lifecycle totals...");
            state.push_pending_session_command(SessionCommandEvent::SessionCount);
        }
        SlashCommand::SessionSearch(query) => {
            state.push_transcript(format!("session search: searching for '{query}'..."));
            state.push_pending_session_command(SessionCommandEvent::SessionSearch(query));
        }
        SlashCommand::SessionShow(target) => {
            state.push_transcript(format!("session show: loading {target}..."));
            state.push_pending_session_command(SessionCommandEvent::SessionShow(target));
        }
        SlashCommand::SessionLocate(target) => {
            state.push_transcript(format!("session locate: resolving {target}..."));
            state.push_pending_session_command(SessionCommandEvent::SessionLocate(target));
        }
        SlashCommand::SessionResume(target) => {
            state.push_transcript(format!("session resume: loading {target}..."));
            state.push_pending_session_command(SessionCommandEvent::SessionResume(target));
        }
        SlashCommand::SessionReplay { target, last } => {
            let suffix = last
                .map(|count| format!(" (last {count})"))
                .unwrap_or_default();
            state.push_transcript(format!("session replay: loading {target}{suffix}..."));
            state.push_pending_session_command(SessionCommandEvent::SessionReplay { target, last });
        }
        SlashCommand::SessionExport { target, artifacts } => {
            state.push_transcript(format!("session export: writing artifacts for {target}..."));
            state.push_pending_session_command(SessionCommandEvent::SessionExport {
                target,
                artifacts,
            });
        }
        SlashCommand::SessionImport { from, id, force } => {
            state.push_transcript(format!("session import: importing {from}..."));
            state.push_pending_session_command(SessionCommandEvent::SessionImport {
                from,
                id,
                force,
            });
        }
        SlashCommand::SubagentModel(change) => match change {
            peridot_core::SubagentModelChange::Set(name) => {
                let from = previous_subagent_model
                    .clone()
                    .unwrap_or_else(|| "<inherit caller>".to_string());
                state.push_transcript(format!("subagent model: {from} -> {name}"));
            }
            peridot_core::SubagentModelChange::Reset => {
                let from = previous_subagent_model
                    .clone()
                    .unwrap_or_else(|| "<inherit caller>".to_string());
                state.push_transcript(format!("subagent model: {from} -> <inherit caller>"));
            }
        },
        SlashCommand::Reasoning(effort) => {
            state.push_transcript(format!(
                "reasoning: {previous_reasoning_effort} -> {effort}"
            ));
        }
        SlashCommand::Fast(_change) => {
            let to = state
                .service_tier
                .clone()
                .unwrap_or_else(|| "standard".to_string());
            state.push_transcript(format!("fast: {previous_service_tier} -> {to}"));
        }
        SlashCommand::McpList => {
            state.push_transcript("mcp: listing servers from config.toml…");
            state.push_pending_session_command(SessionCommandEvent::McpList);
        }
        SlashCommand::McpAdd {
            name,
            transport,
            target,
        } => {
            state.push_transcript(format!(
                "mcp: adding server '{name}' ({transport}) → config.toml"
            ));
            state.push_pending_session_command(SessionCommandEvent::McpAdd {
                name,
                transport,
                target,
            });
        }
        SlashCommand::McpRemove(name) => {
            state.push_transcript(format!("mcp: removing server '{name}' from config.toml"));
            state.push_pending_session_command(SessionCommandEvent::McpRemove(name));
        }
        SlashCommand::McpTest(name) => {
            state.push_transcript(format!("mcp: testing '{name}'…"));
            state.push_pending_session_command(SessionCommandEvent::McpTest(name));
        }
        SlashCommand::Todos => {
            state.push_transcript("todos: scanning project…");
            state.push_pending_session_command(SessionCommandEvent::ScanTodos);
        }
        SlashCommand::CodeMap => {
            state.push_transcript("codemap: loading workspace code map index…");
            state.push_pending_session_command(SessionCommandEvent::CodeMap);
        }
        SlashCommand::CodeMapStatus => {
            state.push_transcript("codemap: checking workspace code map status…");
            state.push_pending_session_command(SessionCommandEvent::CodeMapStatus);
        }
        SlashCommand::CodeMapRefresh => {
            state.push_transcript("codemap: refreshing workspace code map index…");
            state.push_pending_session_command(SessionCommandEvent::CodeMapRefresh);
        }
        SlashCommand::CodeMapFind(query) => {
            state.push_transcript(format!(
                "codemap: searching workspace code map for '{query}'…"
            ));
            state.push_pending_session_command(SessionCommandEvent::CodeMapFind(query));
        }
        SlashCommand::CodeMapLocate(query) => {
            state.push_transcript(format!("codemap: locating symbol '{query}'…"));
            state.push_pending_session_command(SessionCommandEvent::CodeMapLocate(query));
        }
        SlashCommand::CodeMapOutline(path) => {
            state.push_transcript(format!("codemap: outlining file '{path}'…"));
            state.push_pending_session_command(SessionCommandEvent::CodeMapOutline(path));
        }
        SlashCommand::CodeMapRefs(query) => {
            state.push_transcript(format!("codemap: finding references for '{query}'…"));
            state.push_pending_session_command(SessionCommandEvent::CodeMapRefs(query));
        }
        SlashCommand::Attachments => {
            state.push_transcript("attachments: loading session attachment inventory…");
            state.push_pending_session_command(SessionCommandEvent::Attachments);
        }
        SlashCommand::Attach(path) => {
            state.push_transcript(format!("attach: loading {path}…"));
            state.push_pending_session_command(SessionCommandEvent::Attach(path));
        }
        SlashCommand::Detach(path) => {
            state.push_transcript(format!("detach: removing {path} from session context…"));
            state.push_pending_session_command(SessionCommandEvent::Detach(path));
        }
        SlashCommand::Export(artifacts) => {
            state.push_transcript("export: writing session artifacts…");
            state.push_pending_session_command(SessionCommandEvent::Export(artifacts));
        }
        SlashCommand::Rewind => apply_rewind(state),
        SlashCommand::BranchSave(name) => {
            state.push_transcript(format!("branch: saving '{name}'…"));
            state.push_pending_session_command(SessionCommandEvent::BranchSave(name));
        }
        SlashCommand::BranchRestore(name) => {
            if state.is_agent_busy() {
                state.push_error("branch restore: refusing while agent is running — wait for the task to finish or interrupt it first");
            } else {
                state.push_transcript(format!("branch: restoring '{name}'…"));
                state.push_pending_session_command(SessionCommandEvent::BranchRestore(name));
            }
        }
        SlashCommand::BranchList => {
            state.push_transcript("branch: listing snapshots…");
            state.push_pending_session_command(SessionCommandEvent::BranchList);
        }
        SlashCommand::BranchTurn(turn_id) => {
            if state.is_agent_busy() {
                state.push_error(
                    "branch turn: refusing while agent is running — wait or interrupt first",
                );
            } else {
                state.push_transcript(format!("branch: forking at turn {turn_id}…"));
                state.push_pending_session_command(SessionCommandEvent::BranchTurn(turn_id));
            }
        }
        SlashCommand::BranchTree => {
            state.push_transcript("branch: loading DAG journal…");
            state.push_pending_session_command(SessionCommandEvent::BranchTree);
        }
        SlashCommand::BranchSwitch(index) => {
            if state.is_agent_busy() {
                state.push_error(
                    "branch switch: refusing while agent is running — wait or interrupt first",
                );
            } else {
                state.push_transcript(format!("branch: switching to limb [{index}]…"));
                state.push_pending_session_command(SessionCommandEvent::BranchSwitch(index));
            }
        }
        SlashCommand::BranchPicker => {
            if state.is_agent_busy() {
                state.push_error(
                    "branch: refusing to open picker while agent is running — wait or interrupt first",
                );
            } else {
                state.branch_picker = Some(crate::BranchPickerState::opening());
                state.push_transcript("branch: opening picker…");
                state.push_pending_session_command(SessionCommandEvent::BranchPickerOpen);
            }
        }
        SlashCommand::Skill { name, args } => {
            state.push_transcript(format!("skill `{name}`: loading..."));
            state.push_pending_session_command(SessionCommandEvent::Skill { name, args });
        }
        SlashCommand::SkillList => {
            state.push_transcript(crate::tr(
                PhraseKey::NoticeSkillsLoading,
                state.config.language,
            ));
            state.push_pending_session_command(SessionCommandEvent::SkillList);
        }
        SlashCommand::SkillShow(name) => {
            state.push_transcript(format!("skill `{name}`: loading details..."));
            state.push_pending_session_command(SessionCommandEvent::SkillShow(name));
        }
        SlashCommand::SkillSearch(query) => {
            state.push_transcript(format!("skills: searching `{query}`..."));
            state.push_pending_session_command(SessionCommandEvent::SkillSearch(query));
        }
        SlashCommand::SkillArchived(query) => {
            let suffix = if query.trim().is_empty() {
                String::new()
            } else {
                format!(" matching `{}`", query.trim())
            };
            state.push_transcript(format!("skills: listing archived{suffix}..."));
            state.push_pending_session_command(SessionCommandEvent::SkillArchived(query));
        }
        SlashCommand::SkillPin(name) => {
            state.push_transcript(format!("skill `{name}`: pinning..."));
            state.push_pending_session_command(SessionCommandEvent::SkillPin(name));
        }
        SlashCommand::SkillUnpin(name) => {
            state.push_transcript(format!("skill `{name}`: unpinning..."));
            state.push_pending_session_command(SessionCommandEvent::SkillUnpin(name));
        }
        SlashCommand::SkillArchive(name) => {
            state.push_transcript(format!("skill `{name}`: archiving..."));
            state.push_pending_session_command(SessionCommandEvent::SkillArchive(name));
        }
        SlashCommand::SkillRestore(name) => {
            state.push_transcript(format!("skill `{name}`: restoring..."));
            state.push_pending_session_command(SessionCommandEvent::SkillRestore(name));
        }
    }
}

/// Pops the visible transcript back to (but not including) the operator's
/// last `User` entry, reloads that prompt into the input buffer for
/// editing, and asks the CLI host to remove the same turn from the
/// context snapshot.
fn apply_rewind(state: &mut TuiState) {
    if state.is_agent_busy() {
        state.push_error("rewind: refusing while agent is running");
        return;
    }
    // `submit_input` pushes the slash command itself as a User entry
    // before invoking the handler so the operator sees their own
    // command land in the transcript. For `/rewind` that's a problem —
    // rposition would land on "/rewind" instead of the prior real
    // message. Pop it so the search targets the actual exchange we
    // want to roll back.
    if state
        .transcript
        .last()
        .is_some_and(|e| e.kind == TranscriptKind::User && e.text.trim() == "/rewind")
    {
        state.transcript.pop();
    }
    let Some(user_idx) = state
        .transcript
        .iter()
        .rposition(|entry| entry.kind == TranscriptKind::User)
    else {
        state.push_error("rewind: no user message to roll back to");
        return;
    };
    let prior_text = state.transcript[user_idx].text.clone();
    state.transcript.truncate(user_idx);
    state.input = prior_text;
    state.input_cursor = state.input.chars().count();
    state.input_history_cursor = None;
    state.refresh_at_picker();
    state.push_pending_session_command(SessionCommandEvent::RewindContext);
    state.push_transcript_entry(
        TranscriptKind::Notice,
        "rewind: restored the last prompt to the input box and queued context rollback.",
    );
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

/// Computes the (line index, column-in-chars) of the input cursor inside
/// `state.input`. Both indices are 0-based. Returns `(0, 0)` for an empty
/// buffer or a cursor that sits at position 0.
fn input_cursor_line_col(state: &TuiState) -> (usize, usize) {
    let prefix: String = state.input.chars().take(state.input_cursor).collect();
    let line = prefix.matches('\n').count();
    let col = prefix
        .rsplit('\n')
        .next()
        .map(|tail| tail.chars().count())
        .unwrap_or(0);
    (line, col)
}

/// Returns the character offset of the start of `target_line` (0-based)
/// inside `state.input`. `target_line` past the end clamps to the last
/// line so callers can dead-reckon a position even on overflow.
fn line_start_char_offset(input: &str, target_line: usize) -> usize {
    let mut count = 0usize;
    let mut offset = 0usize;
    for ch in input.chars() {
        if count == target_line {
            break;
        }
        offset += 1;
        if ch == '\n' {
            count += 1;
        }
    }
    offset
}

/// Tries to move the cursor to the previous logical line, snapping the
/// column to the line's length if it would overshoot. Returns `false`
/// when the cursor is already on line 0 — callers fall back to history
/// in that case.
fn try_move_cursor_up(state: &mut TuiState) -> bool {
    let (line, col) = input_cursor_line_col(state);
    if line == 0 {
        return false;
    }
    let lines: Vec<&str> = state.input.split('\n').collect();
    let target_line = line - 1;
    let target_col = col.min(lines[target_line].chars().count());
    state.input_cursor = line_start_char_offset(&state.input, target_line) + target_col;
    true
}

/// Mirror of [`try_move_cursor_up`] for the Down arrow.
fn try_move_cursor_down(state: &mut TuiState) -> bool {
    let (line, col) = input_cursor_line_col(state);
    let lines: Vec<&str> = state.input.split('\n').collect();
    if line + 1 >= lines.len() {
        return false;
    }
    let target_line = line + 1;
    let target_col = col.min(lines[target_line].chars().count());
    state.input_cursor = line_start_char_offset(&state.input, target_line) + target_col;
    true
}

/// Flips the Status side-panel visibility and reports the new state in the
/// transcript so the operator gets immediate feedback. Wired to both the
/// `Ctrl+]` / `F2` key handlers and the `/sidepanel` slash command — they
/// all funnel through here so the user-visible behaviour stays identical
/// regardless of how the toggle was triggered.
pub(super) fn toggle_sidepanel(state: &mut TuiState) {
    state.config.show_subagent_panel = !state.config.show_subagent_panel;
    let label = if state.config.show_subagent_panel {
        "on"
    } else {
        "off"
    };
    state.push_transcript_entry(
        TranscriptKind::Notice,
        format!("sidepanel: {label} (Ctrl+] / F2 / /sidepanel toggles)"),
    );
}

/// Acts on the operator's menu choice. Each option resolves to an existing
/// slash-command behaviour or a small in-place change so the menu finally
/// does something instead of just echoing a `menu: …` notice. Unknown
/// labels still produce a notice — that keeps custom menu options
/// (if any) visible while developing.
fn apply_menu_selection(state: &mut TuiState, selected: &str) -> TuiEventOutcome {
    match selected {
        "Quit" => TuiEventOutcome::Quit,
        "Debug" => {
            state.debug_view = !state.debug_view;
            let label = if state.debug_view { "on" } else { "off" };
            state.push_transcript_entry(TranscriptKind::Notice, format!("debug: {label}"));
            TuiEventOutcome::Continue
        }
        "Mode" => {
            // Cycle Plan → Execute → Goal → Plan. Each step also records a
            // lifecycle event so the run-log keeps the transition trail.
            let next = match state.header.mode {
                ExecutionMode::Plan => ExecutionMode::Execute,
                ExecutionMode::Execute => ExecutionMode::Goal,
                ExecutionMode::Goal => ExecutionMode::Plan,
            };
            record_mode_switch(state, next);
            state.header.mode = next;
            state.push_transcript_entry(TranscriptKind::Notice, format!("mode: {next}"));
            TuiEventOutcome::Continue
        }
        "Permission" => {
            let next = match state.header.permission {
                PermissionMode::Safe => PermissionMode::Auto,
                PermissionMode::Auto => PermissionMode::Yolo,
                PermissionMode::Yolo => PermissionMode::Safe,
            };
            record_permission_switch(state, next);
            state.header.permission = next;
            state.push_transcript_entry(TranscriptKind::Notice, format!("permission: {next}"));
            TuiEventOutcome::Continue
        }
        "Save Session" => {
            // Equivalent to `/session save` — file-backed save lives on
            // the CLI side; for now we just surface the intent.
            state.push_transcript_entry(
                TranscriptKind::Notice,
                "session: save requested (sessions auto-persist to disk every tick)",
            );
            TuiEventOutcome::Continue
        }
        "History" => {
            if state.input_history.is_empty() {
                state.push_transcript_entry(
                    TranscriptKind::Notice,
                    "history: <empty> — press Up/Down at the prompt to recall past inputs",
                );
            } else {
                let recent: Vec<String> = state
                    .input_history
                    .iter()
                    .rev()
                    .take(10)
                    .enumerate()
                    .map(|(idx, entry)| format!("  {}. {}", idx + 1, entry))
                    .collect();
                state.push_transcript_entry(
                    TranscriptKind::Notice,
                    format!("history (10 most recent):\n{}", recent.join("\n")),
                );
            }
            TuiEventOutcome::Continue
        }
        "Settings" => {
            let body = format!(
                "settings:\n\
                 - config file: ~/.peridot/config.toml (global) and <project>/.peridot/config.toml\n\
                 - inspect: `peridot config show` (text) or `peridot config show --output json`\n\
                 - edit:    `peridot config set <key> <value>` (e.g. models.main, defaults.mode)\n\
                 - theme / language / panel visibility live under the `[tui]` section.\n\
                 current: theme={}, lang={}, sidepanel={}, mascot={}",
                state.config.theme,
                state.config.language,
                state.config.show_subagent_panel,
                state.config.show_mascot,
            );
            state.push_transcript_entry(TranscriptKind::Notice, body);
            TuiEventOutcome::Continue
        }
        "Keybindings" => {
            state.push_transcript_entry(
                TranscriptKind::Notice,
                "keybindings:\n\
                 - Enter             submit\n\
                 - Ctrl+J / Alt+Enter newline (Shift+Enter only on native CSI-u terminals)\n\
                 - Esc               interrupt run / open menu\n\
                 - Ctrl+P            menu\n\
                 - Ctrl+] / F2 / /sidepanel  toggle Status side panel\n\
                 - Ctrl+L            clear transcript\n\
                 - Ctrl+U            clear input\n\
                 - Ctrl+A / Ctrl+E   home / end\n\
                 - Ctrl+T            open session picker\n\
                 - Ctrl+W            cycle session\n\
                 - PageUp / PageDown scroll transcript\n\
                 - Shift+Up / Down   scroll by line\n\
                 - Ctrl+C twice / Ctrl+D quit",
            );
            TuiEventOutcome::Continue
        }
        other => {
            state.push_transcript_entry(TranscriptKind::Notice, format!("menu: {other}"));
            TuiEventOutcome::Continue
        }
    }
}
