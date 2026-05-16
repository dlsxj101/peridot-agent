use peridot_common::{AskUserRequest, ExecutionMode, PermissionMode, TuiConfig};
use peridot_core::{GoalStatus, SlashCommand};

use super::input::*;
use super::render::*;
use super::state::{TranscriptEntry, TranscriptKind};
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
fn empty_tui_renders_welcome_and_usage_hints() {
    let state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    let snapshot = render_text_snapshot(&state);

    assert!(state.menu.is_none());
    assert!(snapshot.contains("Welcome back"));
    assert!(snapshot.contains("Type a task in the input line below and press Enter."));
    assert!(snapshot.contains("/plan  /execute  /goal <objective>"));
    assert!(snapshot.contains("Enter sends"));
    assert!(!snapshot.contains("Peridot Menu"));
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
    assert_eq!(state.transcript[0].text, "assistant: hello world");
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
fn tool_result_preview_reaches_main_transcript() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    state.apply_runtime_event(TuiRuntimeEvent::ToolFinished {
        name: "shell_exec".to_string(),
        success: true,
        summary: "command exited 0: cargo test".to_string(),
        output: serde_json::json!({
            "status": 0,
            "stdout": "running tests\nok\n",
            "stderr": ""
        }),
    });
    state.apply_runtime_event(TuiRuntimeEvent::ToolFinished {
        name: "file_write".to_string(),
        success: true,
        summary: "wrote /tmp/demo.rs".to_string(),
        output: serde_json::json!({ "path": "/tmp/demo.rs" }),
    });

    let snapshot = render_text_snapshot(&state);
    assert!(snapshot.contains("tool shell_exec: ok: command exited 0: cargo test"));
    assert!(snapshot.contains("  stdout:"));
    assert!(snapshot.contains("    running tests"));
    assert!(snapshot.contains("tool file_write: ok: wrote /tmp/demo.rs"));
    assert!(snapshot.contains("  path: \"/tmp/demo.rs\""));
}

#[test]
fn tool_started_preview_shows_patch_and_write_details() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    state.apply_runtime_event(TuiRuntimeEvent::ToolStarted {
        name: "file_patch".to_string(),
        parameters: serde_json::json!({
            "path": "src/lib.rs",
            "old_text": "fn old() {\n    todo!()\n}\n",
            "new_text": "fn old() {\n    42\n}\n"
        }),
    });
    state.apply_runtime_event(TuiRuntimeEvent::ToolStarted {
        name: "file_write".to_string(),
        parameters: serde_json::json!({
            "path": "README.md",
            "content": "# Peridot\n\nhello\n"
        }),
    });

    let snapshot = render_text_snapshot(&state);
    assert!(snapshot.contains("tool file_patch: running"));
    assert!(snapshot.contains("  path: src/lib.rs"));
    assert!(snapshot.contains("    - fn old() {"));
    assert!(snapshot.contains("    + fn old() {"));
    assert!(snapshot.contains("tool file_write: running"));
    assert!(snapshot.contains("  content:"));
    assert!(snapshot.contains("    # Peridot"));
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
            KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL)
        ),
        TuiEventOutcome::Continue
    );
    assert_eq!(state.input, "");

    assert_eq!(
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE)
        ),
        TuiEventOutcome::Continue
    );
    assert_eq!(
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
        ),
        TuiEventOutcome::Submit("o".to_string())
    );
}

#[test]
fn input_history_and_control_shortcuts_work() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    for character in "first".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    assert_eq!(
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
        ),
        TuiEventOutcome::Submit("first".to_string())
    );

    handle_key_event(&mut state, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(state.input, "first");
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(state.input, "");

    state.input = "clear me".to_string();
    handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
    );
    assert_eq!(state.input, "");

    state.push_transcript("old");
    handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
    );
    assert!(state.transcript.is_empty());
}

#[test]
fn input_cursor_supports_midline_editing() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    for character in "ac".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE),
    );
    assert_eq!(state.input, "abc");
    assert_eq!(state.input_cursor, 2);

    handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE),
    );
    assert_eq!(state.input, "ab");

    handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
    );
    assert_eq!(state.input_cursor, 0);
    handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL),
    );
    assert_eq!(state.input_cursor, 2);
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
    assert!(state.transcript.last().unwrap().text.contains("goal: paused"));
    assert!(render_text_snapshot(&state).contains("goal paused"));
}

#[test]
fn utility_slash_commands_update_tui_surface() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.side_panel.plan.push(PlanStep {
        label: "write tests".to_string(),
        done: false,
    });

    apply_slash_command(&mut state, SlashCommand::Help);
    assert!(state.transcript.last().unwrap().text.contains("/model <name>"));

    apply_slash_command(&mut state, SlashCommand::PlanShow);
    assert!(
        state
            .transcript
            .iter()
            .any(|line| line.text.contains("[ ] 1. write tests"))
    );

    apply_slash_command(&mut state, SlashCommand::Model("next-model".to_string()));
    assert_eq!(state.header.model, "next-model");

    state.input = "/not-real".to_string();
    assert_eq!(
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
        ),
        TuiEventOutcome::Continue
    );
    assert!(state.transcript.last().unwrap().text.contains("/help"));

    apply_slash_command(&mut state, SlashCommand::Clear);
    assert!(state.transcript.is_empty());
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
    assert!(state.transcript[0].text.contains("Proceed? -> no"));
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

    let panel = state.ask_user.as_mut().unwrap();
    panel.freeform = "abc".to_string();
    handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL),
    );
    assert_eq!(state.ask_user.as_ref().unwrap().freeform, "ab");
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
    let menu = render_menu(state.menu.as_ref().unwrap());
    assert!(menu.contains("Peridot Menu"));
    assert!(menu.contains("Esc or q closes this menu and returns to chat input."));

    assert_eq!(
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)
        ),
        TuiEventOutcome::Continue
    );
    assert!(state.menu.is_none());
}

#[test]
fn runtime_events_update_tui_without_exiting() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    state.apply_runtime_event(TuiRuntimeEvent::RunStarted {
        task: "fix tests".to_string(),
    });
    state.apply_runtime_event(TuiRuntimeEvent::AssistantDelta {
        delta: "thinking".to_string(),
    });
    state.apply_runtime_event(TuiRuntimeEvent::AssistantFinished);
    state.apply_runtime_event(TuiRuntimeEvent::Thinking {
        text: "checking the failing test path".to_string(),
    });
    state.apply_runtime_event(TuiRuntimeEvent::ToolStarted {
        name: "verify_test".to_string(),
        parameters: serde_json::json!({}),
    });
    state.apply_runtime_event(TuiRuntimeEvent::ToolFinished {
        name: "verify_test".to_string(),
        success: true,
        summary: "passed".to_string(),
        output: serde_json::json!({}),
    });
    state.apply_runtime_event(TuiRuntimeEvent::Finished {
        stop_reason: "Done".to_string(),
        turns: 1,
        success: true,
    });

    let snapshot = render_text_snapshot(&state);
    assert!(snapshot.contains("agent done"));
    assert!(snapshot.contains("task: fix tests"));
    assert!(snapshot.contains("assistant: thinking"));
    assert!(
        !snapshot.contains("thinking: checking the failing test path"),
        "thinking text should be hidden in non-debug view"
    );
    assert!(snapshot.contains("tool verify_test: ok: passed"));
    assert!(snapshot.contains("run: stopped=Done turns=1"));

    state.debug_view = true;
    let debug_snapshot = render_text_snapshot(&state);
    assert!(debug_snapshot.contains("thinking: checking the failing test path"));
    state.debug_view = false;

    state.apply_runtime_event(TuiRuntimeEvent::SessionSaved {
        session_id: "session-test".to_string(),
    });
    let snapshot = render_text_snapshot(&state);
    assert!(snapshot.contains("session: saved session-test"));
}

#[test]
fn assistant_json_action_renders_only_user_facing_text() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    let raw = "The user's previous message...\n\
        Analysis: I should ask.\n\
        Decision: ask.\n\
        {\"action\": \"agent_ask_user\", \"parameters\": {\"question\": \"How can I help you?\"}}";
    state.apply_runtime_event(TuiRuntimeEvent::AssistantDelta {
        delta: raw.to_string(),
    });
    state.apply_runtime_event(TuiRuntimeEvent::AssistantFinished);

    let visible: Vec<&TranscriptEntry> = state
        .transcript
        .iter()
        .filter(|entry| entry.kind == TranscriptKind::Assistant)
        .collect();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].text, "assistant: ask: How can I help you?");

    let snapshot = render_text_snapshot(&state);
    assert!(snapshot.contains("ask: How can I help you?"));
    assert!(!snapshot.contains("Analysis:"));
    assert!(!snapshot.contains("\"action\""));
}

#[test]
fn assistant_tool_call_action_emits_no_visible_assistant_line() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    let raw = "{\"action\": \"file_read\", \"parameters\": {\"path\": \"src/lib.rs\"}}";
    state.apply_runtime_event(TuiRuntimeEvent::AssistantDelta {
        delta: raw.to_string(),
    });
    state.apply_runtime_event(TuiRuntimeEvent::AssistantFinished);
    assert!(
        !state
            .transcript
            .iter()
            .any(|entry| entry.kind == TranscriptKind::Assistant)
    );
}

#[test]
fn busy_agent_queues_input_and_drains_when_idle() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.apply_runtime_event(TuiRuntimeEvent::RunStarted {
        task: "first task".to_string(),
    });
    assert_eq!(state.agent_run_status, AgentRunStatus::Running);

    for character in "second".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    let outcome = handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
    assert_eq!(outcome, TuiEventOutcome::Continue);
    assert_eq!(state.input_queue, vec!["second".to_string()]);
    assert!(
        state
            .transcript
            .iter()
            .any(|entry| entry.kind == TranscriptKind::Notice
                && entry.text.contains("대기열에 추가됨"))
    );

    state.apply_runtime_event(TuiRuntimeEvent::Finished {
        stop_reason: "Done".to_string(),
        turns: 1,
        success: true,
    });
    let mut submitted: Vec<String> = Vec::new();
    let mut on_submit = |task: String, state: &mut TuiState| {
        submitted.push(task);
        state.last_task = Some("second".to_string());
    };
    drain_input_queue(&mut state, &mut on_submit);

    assert_eq!(submitted, vec!["second".to_string()]);
    assert!(state.input_queue.is_empty());
    assert_eq!(state.agent_run_status, AgentRunStatus::Running);
}

#[test]
fn status_bar_reflects_active_tool_and_spinner() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.apply_runtime_event(TuiRuntimeEvent::ToolStarted {
        name: "shell_exec".to_string(),
        parameters: serde_json::json!({}),
    });
    let snapshot = render_text_snapshot(&state);
    assert!(snapshot.contains("status: 도구 실행 중: shell_exec"));

    state.apply_runtime_event(TuiRuntimeEvent::ToolFinished {
        name: "shell_exec".to_string(),
        success: true,
        summary: "ok".to_string(),
        output: serde_json::json!({}),
    });
    assert!(state.active_tools.is_empty());
}

#[test]
fn approval_runtime_event_opens_panel_and_records_decision() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    state.apply_runtime_event(TuiRuntimeEvent::ApprovalRequested {
        tool_name: "shell_exec".to_string(),
        reason: "dependency installation requires explicit user approval".to_string(),
    });

    assert_eq!(state.agent_run_status, AgentRunStatus::WaitingApproval);
    assert!(render_text_snapshot(&state).contains("agent waiting-approval"));
    assert!(
        render_approval_panel(state.approval.as_ref().unwrap()).contains("dependency installation")
    );

    state.apply_runtime_event(TuiRuntimeEvent::Finished {
        stop_reason: "ApprovalRequired".to_string(),
        turns: 0,
        success: false,
    });
    assert_eq!(state.agent_run_status, AgentRunStatus::WaitingApproval);
    assert!(state.approval.is_some());
    assert!(
        state
            .transcript
            .last()
            .unwrap()
            .text
            .contains("stopped=ApprovalRequired")
    );

    handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
    );

    assert!(state.approval.is_none());
    assert_eq!(state.agent_run_status, AgentRunStatus::Running);
    assert!(
        state
            .transcript
            .last()
            .unwrap()
            .text
            .contains("approval: shell_exec approved")
    );
}
