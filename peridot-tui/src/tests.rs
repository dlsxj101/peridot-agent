use peridot_common::{AskUserRequest, ExecutionMode, PermissionMode, TuiConfig};
use peridot_core::{GoalStatus, SlashCommand};

use super::input::*;
use super::render::*;
use super::*;

#[test]
fn selects_layout_from_terminal_size() {
    assert_eq!(select_layout(140, 40), LayoutMode::Full);
    assert_eq!(select_layout(90, 24), LayoutMode::Compact);
    assert_eq!(select_layout(60, 12), LayoutMode::Minimal);
}

#[test]
fn header_records_tokens_cost_and_cache_rate() {
    let mut header = HeaderState::new(ExecutionMode::Execute, PermissionMode::Auto, "mock");

    header.record_usage(80, 20, 20, 0, 0.05);

    assert_eq!(header.total_tokens, 120);
    assert_eq!(header.cost_usd, 0.05);
    assert!((header.cache_hit_rate - 0.2).abs() < f64::EPSILON);
}

#[test]
fn parses_input_slash_command() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.input = "/goal fix tests".to_string();

    assert_eq!(
        state.current_slash_command(),
        Some(SlashCommand::GoalStart("fix tests".to_string()))
    );
}

#[test]
fn renders_text_snapshot() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.push_transcript("tool file_write ok");
    state.side_panel.plan.push(PlanStep {
        label: "Implement hooks".to_string(),
        done: true,
    });

    let snapshot = render_text_snapshot(&state);

    assert!(snapshot.contains("PERIDOT | execute.auto | mock"));
    assert!(snapshot.contains("[x] Implement hooks"));
    assert!(snapshot.contains("tool file_write ok"));
}

#[test]
fn streaming_state_renders_and_finishes_into_transcript() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    state.begin_stream("assistant");
    state.push_stream_delta("hello");
    state.push_stream_delta(" world");

    let snapshot = render_text_snapshot(&state);
    assert!(snapshot.contains("assistant: hello world"));
    assert!(snapshot.contains("stream assistant: streaming"));

    state.finish_stream();

    assert!(state.active_stream.is_none());
    assert_eq!(state.transcript[0], "assistant: hello world");
    assert!(render_text_snapshot(&state).contains("stream assistant: done"));
    assert_eq!(state.side_panel.stats.steps, 1);
}

#[test]
fn records_tool_and_verification_activity() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    state.record_tool_activity("file_write", true, "wrote file");
    state.record_verification_activity("cargo test", false, "tests failed");

    let snapshot = render_text_snapshot(&state);
    assert!(snapshot.contains("tool file_write: ok: wrote file"));
    assert!(snapshot.contains("verify cargo test: failed: tests failed"));
    assert_eq!(state.side_panel.stats.steps, 2);
    assert_eq!(state.side_panel.stats.errors, 1);
}

#[test]
fn records_subagent_monitor_state() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Goal,
        PermissionMode::Auto,
        "mock",
    ));

    state.record_subagent_started("worktree", "refactor tools");
    state.record_subagent_completed("worktree", "refactor tools", "branch prepared");
    state.record_subagent_failed("teammate", "audit release", "runner unavailable");

    let snapshot = render_text_snapshot(&state);
    assert!(snapshot.contains("subagent worktree: running: refactor tools"));
    assert!(snapshot.contains("subagent worktree: done: branch prepared"));
    assert!(snapshot.contains("- worktree refactor tools [done]: branch prepared"));
    assert!(snapshot.contains("- teammate audit release [failed]: runner unavailable"));
    assert_eq!(state.side_panel.stats.steps, 2);
    assert_eq!(state.side_panel.stats.errors, 1);
}

#[test]
fn tui_config_hides_optional_metrics_and_side_panel() {
    let mut header = HeaderState::new(ExecutionMode::Execute, PermissionMode::Auto, "mock");
    header.record_usage(80, 20, 20, 0, 0.05);
    let mut state = TuiState::new(header).with_config(TuiConfig {
        show_token_count: false,
        show_cost: false,
        show_cache_rate: false,
        show_subagent_panel: false,
        ..TuiConfig::default()
    });
    state.side_panel.plan.push(PlanStep {
        label: "hidden status".to_string(),
        done: false,
    });

    let snapshot = render_text_snapshot(&state);

    assert!(snapshot.contains("PERIDOT | execute.auto | mock"));
    assert!(!snapshot.contains("tok"));
    assert!(!snapshot.contains("$"));
    assert!(!snapshot.contains("cache"));
    assert!(!snapshot.contains("hidden status"));
}

#[test]
fn draws_with_ratatui_backend() {
    use ratatui::{Terminal, backend::TestBackend};

    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.resize(100, 30);
    state.push_transcript("hello tui");

    terminal.draw(|frame| draw(frame, &state)).unwrap();

    let buffer = terminal.backend().buffer();
    let rendered = format!("{buffer:?}");
    assert!(rendered.contains("PERIDOT"));
    assert!(rendered.contains("hello tui"));
}

#[test]
fn key_events_edit_and_submit_input() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    assert_eq!(
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE)
        ),
        TuiEventOutcome::Continue
    );
    assert_eq!(
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE)
        ),
        TuiEventOutcome::Continue
    );
    assert_eq!(
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)
        ),
        TuiEventOutcome::Continue
    );
    assert_eq!(state.input, "f");

    assert_eq!(
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
        ),
        TuiEventOutcome::Submit("f".to_string())
    );
}

#[test]
fn slash_commands_update_tui_state() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    for character in "/goal ship release".chars() {
        let outcome = handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
        assert_eq!(outcome, TuiEventOutcome::Continue);
    }

    let outcome = handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );

    assert_eq!(outcome, TuiEventOutcome::Continue);
    assert_eq!(state.header.mode, ExecutionMode::Goal);
    assert_eq!(state.goal_status, Some(GoalStatus::Running));
    assert_eq!(state.side_panel.plan[0].label, "ship release");
    assert_eq!(state.lifecycle_events[0].event, "mode_switch");
    assert_eq!(state.lifecycle_events[0].from, "execute");
    assert_eq!(state.lifecycle_events[0].to, "goal");

    apply_slash_command(&mut state, SlashCommand::GoalPause);
    assert_eq!(state.goal_status, Some(GoalStatus::Paused));
    apply_slash_command(&mut state, SlashCommand::GoalStatus);
    assert!(state.transcript.last().unwrap().contains("goal: paused"));
    assert!(render_text_snapshot(&state).contains("goal paused"));
}

#[test]
fn ask_user_panel_renders_and_accepts_choice() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.open_ask_user(AskUserRequest::SingleSelect {
        question: "Proceed?".to_string(),
        options: vec!["yes".to_string(), "no".to_string()],
        default_index: Some(0),
    });

    assert!(render_ask_user_panel(state.ask_user.as_ref().unwrap()).contains("> yes"));
    assert!(render_ask_user_panel(state.ask_user.as_ref().unwrap()).contains("[o] Other"));
    assert!(render_ask_user_panel(state.ask_user.as_ref().unwrap()).contains("[?] Explain"));
    assert_eq!(
        handle_key_event(&mut state, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
        TuiEventOutcome::Continue
    );
    assert_eq!(
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
        ),
        TuiEventOutcome::Continue
    );

    assert!(state.ask_user.is_none());
    assert!(state.transcript[0].contains("Proceed? -> no"));
}

#[test]
fn ask_user_panel_supports_explain_and_other() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.open_ask_user(AskUserRequest::SingleSelect {
        question: "Proceed?".to_string(),
        options: vec!["yes".to_string()],
        default_index: Some(0),
    });

    handle_key_event(&mut state, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
    assert!(
        render_ask_user_panel(state.ask_user.as_ref().unwrap())
            .contains("Peridot needs this decision")
    );

    handle_key_event(&mut state, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
    assert!(state.ask_user.as_ref().unwrap().choices.is_empty());
}

#[test]
fn escape_opens_menu_and_q_closes_it() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    assert_eq!(
        handle_key_event(&mut state, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        TuiEventOutcome::Continue
    );
    assert!(state.menu.is_some());
    assert!(render_menu(state.menu.as_ref().unwrap()).contains("Peridot Menu"));

    assert_eq!(
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)
        ),
        TuiEventOutcome::Continue
    );
    assert!(state.menu.is_none());
}
