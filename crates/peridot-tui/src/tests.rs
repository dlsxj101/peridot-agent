use peridot_common::{AskUserRequest, ExecutionMode, Locale, PermissionMode, TuiConfig};
use peridot_core::{FileDiffPayload, GoalStatus, SlashCommand};

use super::fixtures::{TestScenario, fixture_state};
use super::input::swap_foreground_state;
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

    assert!(snapshot.contains("PERIDOT  mock"));
    assert!(snapshot.contains("metrics: execute · auto"));
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
    assert!(snapshot.contains("stream: hello world"));
    assert!(snapshot.contains("stream assistant: streaming"));

    state.finish_stream();

    assert!(state.active_stream.is_none());
    assert_eq!(state.transcript[0].text, "hello world");
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
fn agent_done_summary_renders_as_assistant_reply() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    state.apply_runtime_event(TuiRuntimeEvent::ToolFinished {
        name: "agent_done".to_string(),
        success: true,
        summary: "프로젝트 분석을 완료했습니다.".to_string(),
        output: serde_json::json!({}),
    });

    let assistant = state
        .transcript
        .iter()
        .find(|entry| entry.kind == TranscriptKind::Assistant)
        .expect("agent_done summary should be shown as assistant text");
    assert_eq!(assistant.text, "프로젝트 분석을 완료했습니다.");
    assert!(!state.transcript.iter().any(|entry| {
        matches!(entry.kind, TranscriptKind::ToolOk) && entry.text.contains("agent_done")
    }));
}

#[test]
fn interrupted_state_does_not_auto_drain_input_queue() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.input_queue.push("queued task".to_string());
    state.apply_runtime_event(TuiRuntimeEvent::Interrupted {
        stage: "turn_error".to_string(),
    });
    let mut submitted: Vec<String> = Vec::new();
    let mut on_submit = |task: String, _state: &mut TuiState| submitted.push(task);

    drain_input_queue(&mut state, &mut on_submit);

    assert!(submitted.is_empty());
    assert_eq!(state.input_queue, vec!["queued task".to_string()]);
}

#[test]
fn ctrl_c_quit_confirmation_consumes_first_press() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    let mut armed = false;

    assert!(!handle_ctrl_c_quit_confirmation(
        &mut state, key, &mut armed
    ));
    assert!(armed);
    assert!(state.transcript.iter().any(|entry| {
        entry.kind == TranscriptKind::Notice && entry.text.contains("Ctrl+C again")
    }));

    assert!(handle_ctrl_c_quit_confirmation(&mut state, key, &mut armed));
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
    // ToolStarted preview only carries the path now; diff bodies arrive
    // via FileDiff after the mutation runs (verified separately by
    // `file_diff_event_renders_unified_diff_lines`).
    assert!(snapshot.contains("tool file_patch: running"));
    assert!(snapshot.contains("  path: src/lib.rs"));
    assert!(snapshot.contains("tool file_write: running"));
    assert!(snapshot.contains("  content:"));
    assert!(snapshot.contains("    # Peridot"));
}

#[test]
fn file_diff_event_renders_unified_diff_lines() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    // file_patch case: both before and after exist; the LCS-driven hunk
    // algorithm should emit `- todo!()` removed line and `+ 42` added line
    // for the body change. The unchanged `fn old() {` / `}` lines are NOT
    // in the diff because they match across both versions.
    state.apply_runtime_event(TuiRuntimeEvent::FileDiff(FileDiffPayload {
        tool_name: "file_patch".to_string(),
        path: "src/lib.rs".to_string(),
        before: Some("fn old() {\n    todo!()\n}\n".to_string()),
        after: "fn old() {\n    42\n}\n".to_string(),
    }));
    // file_write of a brand-new file: before is None, after is the full
    // content. Every line is an addition.
    state.apply_runtime_event(TuiRuntimeEvent::FileDiff(FileDiffPayload {
        tool_name: "file_write".to_string(),
        path: "README.md".to_string(),
        before: None,
        after: "# Peridot\n\nhello\n".to_string(),
    }));

    let snapshot = render_text_snapshot(&state);
    assert!(snapshot.contains("diff: src/lib.rs"));
    assert!(snapshot.contains("- "));
    assert!(snapshot.contains("    todo!()"));
    assert!(snapshot.contains("+ "));
    assert!(snapshot.contains("    42"));
    assert!(snapshot.contains("diff: README.md (new file)"));
    assert!(snapshot.contains("+ # Peridot"));
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
fn tui_config_hides_optional_metrics() {
    // Verifies the `show_token_count` / `show_cost` / `show_cache_rate`
    // toggles suppress those metrics from the live status line. The
    // headless `render_text_snapshot` intentionally exposes side-panel
    // state regardless of `show_subagent_panel`, so plan/activity rows
    // are NOT part of this contract — that toggle is a UI-only concern
    // verified separately by `subagent_panel_toggle_changes_side_visibility`.
    let mut header = HeaderState::new(ExecutionMode::Execute, PermissionMode::Auto, "mock");
    header.record_usage(80, 20, 20, 0, 0.05);
    let state = TuiState::new(header).with_config(TuiConfig {
        show_token_count: false,
        show_cost: false,
        show_cache_rate: false,
        show_subagent_panel: false,
        ..TuiConfig::default()
    });

    let snapshot = render_text_snapshot(&state);

    assert!(snapshot.contains("PERIDOT  mock"));
    assert!(snapshot.contains("metrics: execute · auto"));
    assert!(!snapshot.contains("tok"));
    assert!(!snapshot.contains("$"));
    assert!(!snapshot.contains("cache"));
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
    // The chat view hides System/Notice/TurnSeparator entries — use an
    // Assistant entry so the smoke test actually exercises the user-visible
    // rendering path.
    state.push_assistant("hello tui");

    terminal.draw(|frame| draw(frame, &state)).unwrap();

    let buffer = terminal.backend().buffer();
    let rendered = format!("{buffer:?}");
    assert!(rendered.contains("PERIDOT"));
    assert!(rendered.contains("hello tui"));
}

#[test]
fn borderless_transcript_keeps_status_panel_opt_in() {
    use ratatui::{Terminal, backend::TestBackend};

    let backend = TestBackend::new(120, 32);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.resize(120, 32);
    state.current_session_id = "session-628850-1779011508".to_string();
    state.side_panel.stats.steps = 12;
    state.side_panel.stats.elapsed_seconds = 8;
    state.push_assistant("borderless transcript body");

    terminal.draw(|frame| draw(frame, &state)).unwrap();

    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("session 1779011508"));
    assert!(rendered.contains("steps 12"));
    assert!(rendered.contains("8s"));
    assert!(rendered.contains("subagents 0"));
    assert!(rendered.contains("borderless transcript body"));
    assert!(
        !rendered.contains("Status"),
        "right status panel should be hidden by default"
    );
}

#[test]
fn status_panel_toggle_renders_opt_in_status_column() {
    use ratatui::{Terminal, backend::TestBackend};

    let backend = TestBackend::new(120, 32);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ))
    .with_config(TuiConfig {
        show_subagent_panel: true,
        ..TuiConfig::default()
    });
    state.resize(120, 32);
    state.current_session_id = "session-628850-1779011508".to_string();
    state.side_panel.stats.steps = 12;
    state.push_assistant("transcript with panel");

    terminal.draw(|frame| draw(frame, &state)).unwrap();

    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("Status"));
    assert!(rendered.contains("Session"));
    assert!(rendered.contains("id: 1779011508"));
    assert!(rendered.contains("transcript with panel"));
}

#[test]
fn status_bar_uses_mood_glyph_for_running_agent() {
    use ratatui::{Terminal, backend::TestBackend};

    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.resize(100, 28);
    state.agent_run_status = AgentRunStatus::Running;
    state.push_assistant("running transcript");

    terminal.draw(|frame| draw(frame, &state)).unwrap();

    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("\u{25D1}"));
    assert!(rendered.contains("processing"));
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
fn input_history_dedupes_and_keeps_recent_entries_bounded() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    for index in 0..55 {
        state.record_input_history(&format!("task {index}"));
    }
    state.record_input_history("task 10");

    assert_eq!(state.input_history.len(), 50);
    assert_eq!(
        state.input_history.first().map(String::as_str),
        Some("task 5")
    );
    assert_eq!(
        state.input_history.last().map(String::as_str),
        Some("task 10")
    );
    assert_eq!(
        state
            .input_history
            .iter()
            .filter(|entry| entry.as_str() == "task 10")
            .count(),
        1
    );

    state.previous_input_history();
    assert_eq!(state.input, "task 10");
}

#[test]
fn shift_enter_inserts_newline_without_submitting() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    for character in "line1".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    assert_eq!(
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)
        ),
        TuiEventOutcome::Continue
    );
    for character in "line2".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    assert_eq!(state.input, "line1\nline2");
    assert_eq!(
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
        ),
        TuiEventOutcome::Submit("line1\nline2".to_string())
    );
}

#[test]
fn ctrl_p_opens_menu_and_ctrl_bracket_toggles_side_panel() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    assert!(state.menu.is_none());
    handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
    );
    assert!(state.menu.is_some(), "Ctrl+P should open the menu");
    state.menu = None;

    let initial_panel = state.config.show_subagent_panel;
    handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Char(']'), KeyModifiers::CONTROL),
    );
    assert_eq!(state.config.show_subagent_panel, !initial_panel);
    handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Char(']'), KeyModifiers::CONTROL),
    );
    assert_eq!(state.config.show_subagent_panel, initial_panel);
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
    apply_slash_command(&mut state, SlashCommand::GoalMode);
    assert_eq!(state.header.mode, ExecutionMode::Goal);
    assert_eq!(state.goal_status, None);
    assert!(state.transcript.last().unwrap().text.contains("mode: goal"));

    state.header.mode = ExecutionMode::Execute;
    state.transcript.clear();
    state.lifecycle_events.clear();
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
    assert!(
        state
            .transcript
            .last()
            .unwrap()
            .text
            .contains("goal: paused")
    );
    assert!(render_text_snapshot(&state).contains("goal paused"));

    apply_slash_command(&mut state, SlashCommand::Fast(Some(true)));
    assert_eq!(state.service_tier.as_deref(), Some("fast"));
    apply_slash_command(&mut state, SlashCommand::Fast(Some(false)));
    assert_eq!(state.service_tier, None);
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
    assert!(
        state
            .transcript
            .last()
            .unwrap()
            .text
            .contains("/model <name>")
    );

    apply_slash_command(&mut state, SlashCommand::PlanShow);
    assert!(
        state
            .transcript
            .iter()
            .any(|line| line.text.contains("[ ] 1. write tests"))
    );

    apply_slash_command(&mut state, SlashCommand::ContextTop);
    assert_eq!(
        state.drain_pending_session_commands(),
        vec![SessionCommandEvent::ContextTop]
    );

    apply_slash_command(&mut state, SlashCommand::Model("next-model".to_string()));
    assert_eq!(state.header.model, "next-model");

    state.input = "/x".to_string();
    assert_eq!(
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
        ),
        TuiEventOutcome::Continue
    );
    assert!(state.transcript.last().unwrap().text.contains("/help"));

    apply_slash_command(&mut state, SlashCommand::Clear);
    // `/clear` now performs a deep reset: it wipes the transcript +
    // counters and pushes a single "session opened" notice plus a
    // pending `ClearAndRestart` command for the host. The transcript
    // therefore holds exactly one entry (the post-clear notice), and
    // the pending session command list carries `ClearAndRestart`.
    assert_eq!(state.transcript.len(), 1);
    assert!(
        state
            .transcript
            .last()
            .unwrap()
            .text
            .contains("transcript + context wiped")
    );
    assert!(
        state
            .pending_session_commands
            .iter()
            .any(|cmd| matches!(cmd, SessionCommandEvent::ClearAndRestart))
    );
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
        duration_ms: 0,
    });

    let snapshot = render_text_snapshot(&state);
    assert!(snapshot.contains("agent done"));
    assert!(snapshot.contains("task: fix tests"));
    let assistant_text = state
        .transcript
        .iter()
        .find(|entry| entry.kind == TranscriptKind::Assistant)
        .expect("assistant entry");
    assert_eq!(assistant_text.text, "thinking");
    assert!(
        snapshot.contains("thinking: checking the failing test path"),
        "thinking text should follow tui.show_thinking in non-debug view"
    );
    assert!(snapshot.contains("tool verify_test: ok: passed"));
    assert!(snapshot.contains("run: stopped=Done turns=1"));

    state.config.show_thinking = false;
    let hidden_snapshot = render_text_snapshot(&state);
    assert!(
        !hidden_snapshot.contains("thinking: checking the failing test path"),
        "thinking text should be hidden when tui.show_thinking is disabled"
    );

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
    assert_eq!(visible[0].text, "ask: How can I help you?");

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
fn turn_started_pushes_separator_after_first_transcript_entry() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.apply_runtime_event(TuiRuntimeEvent::TurnStarted { turn_index: 0 });
    assert!(
        state.transcript.is_empty(),
        "first turn should not emit a separator on an empty transcript"
    );
    state.push_transcript("task: first");
    state.apply_runtime_event(TuiRuntimeEvent::TurnStarted { turn_index: 1 });
    let separator = state
        .transcript
        .iter()
        .find(|entry| entry.kind == TranscriptKind::TurnSeparator);
    assert!(
        separator.is_some(),
        "subsequent turns should push a TurnSeparator entry"
    );
    assert!(separator.unwrap().text.contains("turn 2"));
    assert_eq!(state.current_turn, 1);
}

#[test]
fn thinking_log_persists_regardless_of_debug_view() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    assert!(state.thinking_log.is_empty());
    state.apply_runtime_event(TuiRuntimeEvent::Thinking {
        text: "first thought".to_string(),
    });
    state.apply_runtime_event(TuiRuntimeEvent::Thinking {
        text: "second thought".to_string(),
    });
    assert_eq!(state.thinking_log.len(), 2);

    let snapshot = render_text_snapshot(&state);
    assert!(snapshot.contains("first thought"));

    state.config.show_thinking = false;
    let snapshot = render_text_snapshot(&state);
    assert!(!snapshot.contains("first thought"));

    state.debug_view = true;
    let snapshot = render_text_snapshot(&state);
    assert!(snapshot.contains("first thought"));
    assert!(snapshot.contains("second thought"));
}

#[test]
fn interrupted_status_renders_with_dedicated_label() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.agent_run_status = AgentRunStatus::Interrupted;
    let snapshot = render_text_snapshot(&state);
    assert!(snapshot.contains("status: interrupted"));
    assert!(snapshot.contains("agent interrupted"));
}

#[test]
fn header_update_available_appears_in_buffer() {
    use ratatui::{Terminal, backend::TestBackend};

    let backend = TestBackend::new(120, 32);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.resize(120, 32);
    state.header.update_available = Some("v0.2.0".to_string());
    terminal.draw(|frame| draw(frame, &state)).unwrap();
    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("update v0.2.0"));
}

#[test]
fn scroll_offset_anchors_view_when_content_arrives_above_tail() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    // Seed a long history then scroll the view back five rows.
    for i in 0..20 {
        state.push_transcript_entry(TranscriptKind::System, format!("line {i}"));
    }
    state.scroll_up(5);
    assert_eq!(state.scroll_offset, 5);
    assert!(state.is_scrolled_back());

    // New agent output must NOT shift the user's view forward — `scroll_offset`
    // grows by the new entry's row count so the same rows stay visible.
    state.push_transcript_entry(TranscriptKind::Assistant, "new agent reply"); // 1 row
    state.push_transcript_entry(TranscriptKind::Assistant, "line a\nline b"); // 2 rows
    assert_eq!(state.scroll_offset, 8);

    // Scroll-down clamps at the tail.
    state.scroll_down(100);
    assert_eq!(state.scroll_offset, 0);
    assert!(!state.is_scrolled_back());
}

#[test]
fn ctrl_t_opens_session_picker_and_switches_by_prefix() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    let mut alpha = SessionDirectoryItem::new("alpha", "Alpha work");
    alpha.last_event_at_unix = 10;
    let mut beta = SessionDirectoryItem::new("beta", "Beta review");
    beta.last_event_at_unix = 30;
    beta.pending_attention = true;
    let mut build = SessionDirectoryItem::new("build", "Build checks");
    build.last_event_at_unix = 20;
    state.sessions = vec![alpha, beta, build];
    state.current_session_id = "alpha".to_string();

    handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL),
    );
    assert!(state.session_picker.is_some());

    handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE),
    );
    assert_eq!(
        state
            .session_picker
            .as_ref()
            .map(|picker| picker.query.as_str()),
        Some("b")
    );

    handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );

    assert_eq!(state.current_session_id, "beta");
    assert!(state.session_picker.is_none());
    assert!(
        !state
            .sessions
            .iter()
            .find(|item| item.id == "beta")
            .expect("beta session exists")
            .pending_attention
    );
}

#[test]
fn ctrl_w_still_cycles_sessions() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.sessions = vec![
        SessionDirectoryItem::new("a", "Alpha"),
        SessionDirectoryItem::new("b", "Beta"),
    ];
    state.current_session_id = "a".to_string();

    handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
    );

    assert_eq!(state.current_session_id, "b");
    assert!(state.session_picker.is_none());
}

#[test]
fn submit_input_resets_scroll_to_tail() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    for i in 0..10 {
        state.push_transcript_entry(TranscriptKind::System, format!("line {i}"));
    }
    state.scroll_up(4);
    assert!(state.is_scrolled_back());

    state.input = "hello".to_string();
    state.input_cursor = state.input.chars().count();
    let outcome = submit_input(&mut state);

    assert!(matches!(outcome, TuiEventOutcome::Submit(_)));
    assert_eq!(state.scroll_offset, 0, "submit must snap back to the tail");
}

#[test]
fn page_up_and_down_keys_drive_scroll_offset() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    for i in 0..40 {
        state.push_transcript_entry(TranscriptKind::System, format!("line {i}"));
    }

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let page_up = KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE);
    let page_down = KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE);

    handle_key_event(&mut state, page_up);
    let one_page = state.scroll_offset;
    assert!(one_page > 0);
    handle_key_event(&mut state, page_up);
    assert_eq!(state.scroll_offset, one_page * 2);
    handle_key_event(&mut state, page_down);
    assert_eq!(state.scroll_offset, one_page);
}

#[test]
fn tui_state_serde_round_trip_preserves_new_defaults() {
    let state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    let json = serde_json::to_string(&state).expect("serialize");
    let restored: TuiState = serde_json::from_str(&json).expect("round-trip");
    assert_eq!(restored.scroll_offset, 0);
    assert!(restored.slash_picker.is_none());
    assert!(restored.thinking_log.is_empty());
    assert_eq!(restored.last_session_save_unix, 0);
    assert_eq!(restored.current_turn, 0);
    assert_eq!(restored.header.update_available, None);
}

#[test]
fn fixture_scenarios_render_through_ratatui_backend() {
    use ratatui::{Terminal, backend::TestBackend};

    for scenario in [
        TestScenario::Welcome,
        TestScenario::Running,
        TestScenario::Approval,
        TestScenario::AskUser,
        TestScenario::Menu,
        TestScenario::Finished,
        TestScenario::MultiSessionTabBar,
        TestScenario::KoreanLocale,
    ] {
        let backend = TestBackend::new(140, 36);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = fixture_state(scenario);
        state.resize(140, 36);
        terminal
            .draw(|frame| draw(frame, &state))
            .unwrap_or_else(|_| panic!("draw failed for {scenario:?}"));
        let rendered = format!("{:?}", terminal.backend().buffer());
        assert!(
            rendered.contains("PERIDOT"),
            "header missing for {scenario:?}"
        );
    }
}

#[test]
fn subagent_monitor_indents_children_by_depth() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Goal,
        PermissionMode::Auto,
        "mock",
    ));
    state.subagents.push(SubagentMonitorItem {
        kind: "teammate".to_string(),
        task: "parent task".to_string(),
        status: "running".to_string(),
        summary: None,
        id: "p".to_string(),
        parent_id: None,
        depth: 0,
        started_at_unix: 0,
        tokens: 0,
    });
    state.subagents.push(SubagentMonitorItem {
        kind: "fork".to_string(),
        task: "child task".to_string(),
        status: "running".to_string(),
        summary: None,
        id: "c".to_string(),
        parent_id: Some("p".to_string()),
        depth: 1,
        started_at_unix: 0,
        tokens: 1200,
    });
    let rendered = render_subagent_monitor(&state.subagents);
    assert!(rendered.contains("teammate parent task [running]"));
    assert!(rendered.contains("└─ fork child task [running]"));
    assert!(rendered.contains("(1200 tok)"));
}

#[test]
fn esc_during_busy_run_returns_interrupt_outcome() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.apply_runtime_event(TuiRuntimeEvent::RunStarted {
        task: "long task".to_string(),
    });
    assert_eq!(state.agent_run_status, AgentRunStatus::Running);

    let outcome = handle_key_event(&mut state, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(outcome, TuiEventOutcome::Interrupt);
    assert!(state.menu.is_none(), "Esc must not open menu while busy");
}

#[test]
fn esc_with_nonempty_input_clears_buffer() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.input = "draft message".to_string();
    state.input_cursor = state.input.chars().count();
    let outcome = handle_key_event(&mut state, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(outcome, TuiEventOutcome::Continue);
    assert!(state.input.is_empty());
    assert!(state.menu.is_none());
}

#[test]
fn interrupted_status_survives_finished_event() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.apply_runtime_event(TuiRuntimeEvent::Interrupted {
        stage: "turn_start".to_string(),
    });
    assert_eq!(state.agent_run_status, AgentRunStatus::Interrupted);

    state.apply_runtime_event(TuiRuntimeEvent::Finished {
        stop_reason: "Interrupted".to_string(),
        turns: 1,
        success: false,
        duration_ms: 0,
    });
    assert_eq!(
        state.agent_run_status,
        AgentRunStatus::Interrupted,
        "Finished should not downgrade an interrupted run to Failed"
    );
    let snapshot = render_text_snapshot(&state);
    assert!(snapshot.contains("status: interrupted"));
    assert!(snapshot.contains("run: stopped=Interrupted turns=1"));
}

#[test]
fn plan_updated_event_replaces_plan_and_shows_banner() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.push_transcript("first task");
    state.apply_runtime_event(TuiRuntimeEvent::PlanUpdated {
        steps: vec![
            PlanStepUpdate {
                label: "Audit lib.rs".to_string(),
                done: true,
            },
            PlanStepUpdate {
                label: "Patch loop.rs".to_string(),
                done: false,
            },
            PlanStepUpdate {
                label: "Run tests".to_string(),
                done: false,
            },
        ],
        current: Some(1),
    });
    assert_eq!(state.side_panel.plan.len(), 3);
    assert!(state.side_panel.plan[0].done);
    let snapshot = render_text_snapshot(&state);
    assert!(snapshot.contains("banner: Plan (1/3) > Patch loop.rs"));
}

#[test]
fn cross_crate_events_update_side_panel_state() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.apply_runtime_event(TuiRuntimeEvent::BudgetUpdated {
        cost_used: 0.05,
        cost_limit: Some(1.0),
        turns_used: 2,
        turns_limit: Some(10),
    });
    assert!((state.side_panel.budget.cost_used - 0.05).abs() < f64::EPSILON);
    assert_eq!(state.side_panel.budget.turns_used, 2);

    state.apply_runtime_event(TuiRuntimeEvent::ContextUtilizationChanged {
        tokens_used: 5_000,
        threshold: 10_000,
        context_tokens: 4_000,
        message_tokens: 4_500,
        system_tokens: 200,
        tool_schema_tokens: 250,
        overhead_tokens: 50,
    });
    assert!((state.side_panel.context_pct - 0.5).abs() < 1e-6);

    state.apply_runtime_event(TuiRuntimeEvent::McpStatusChanged {
        servers: vec![McpServerSummary {
            name: "fs".to_string(),
            tool_count: 4,
            connected: true,
        }],
    });
    assert_eq!(state.side_panel.mcp_status.len(), 1);

    state.apply_runtime_event(TuiRuntimeEvent::AgentsMdLoaded {
        rule_count: 12,
        paths: vec!["AGENTS.md".to_string()],
    });
    assert_eq!(state.side_panel.agents_md.rule_count, 12);

    state.apply_runtime_event(TuiRuntimeEvent::HookFired {
        name: "pre-git-commit".to_string(),
        category: "lifecycle".to_string(),
        outcome: "allow".to_string(),
    });
    assert!(
        state
            .activities
            .iter()
            .any(|activity| activity.label.contains("pre-git-commit"))
    );

    state.apply_runtime_event(TuiRuntimeEvent::Interrupted {
        stage: "tool_call".to_string(),
    });
    assert_eq!(state.agent_run_status, AgentRunStatus::Interrupted);
    assert!(
        state
            .transcript
            .iter()
            .any(|entry| entry.text.contains("interrupted during tool_call"))
    );
}

#[test]
fn locale_switches_status_bar_strings() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    assert_eq!(state.config.language, Locale::En);
    let snapshot = render_text_snapshot(&state);
    assert!(snapshot.contains("status: idle"));

    state.config.language = Locale::Ko;
    let snapshot = render_text_snapshot(&state);
    assert!(snapshot.contains("status: 대기 중"));

    state.config.language = Locale::En;
    state.apply_runtime_event(TuiRuntimeEvent::ToolStarted {
        name: "shell_exec".to_string(),
        parameters: serde_json::json!({}),
    });
    let snapshot = render_text_snapshot(&state);
    assert!(snapshot.contains("status: tool running: shell_exec"));
}

#[test]
fn approval_panel_offers_scopes_and_returns_scope() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.apply_runtime_event(TuiRuntimeEvent::ApprovalRequested {
        tool_name: "file_patch".to_string(),
        reason: "writes outside workspace".to_string(),
        parameters: serde_json::json!({
            "path": "src/lib.rs",
            "old_text": "fn old() {}\n",
            "new_text": "fn old() { 1 }\n"
        }),
    });

    let panel = state.approval.as_ref().expect("approval panel");
    assert_eq!(panel.choices().len(), 5);
    let panel_text = render_approval_panel(panel);
    assert!(panel_text.contains("Approve once"));
    assert!(panel_text.contains("Approve for session"));
    assert!(panel_text.contains("Approve command"));
    assert!(panel_text.contains("Approve path"));
    assert!(panel_text.contains("Deny"));
    assert!(panel_text.contains("Params:"));
    // file_patch parameters now flow through the per-hunk staging path:
    // the panel renders a "Hunks:" header + per-hunk staged-state lines
    // instead of the single combined Diff: preview.
    assert!(panel_text.contains("Hunks:"));
    assert!(panel_text.contains("hunk 1"));
    assert_eq!(panel.hunks.len(), 1);
    assert!(panel.all_hunks_accepted());

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let outcome = handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
    match outcome {
        TuiEventOutcome::Approval {
            decision, scope, ..
        } => {
            assert_eq!(decision, ApprovalDecision::Approve);
            assert_eq!(scope, ApprovalScope::Command);
        }
        other => panic!("unexpected outcome: {other:?}"),
    }
}

#[test]
fn approval_panel_tab_toggles_focused_hunk_and_arrow_keys_move_focus() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.apply_runtime_event(TuiRuntimeEvent::ApprovalRequested {
        tool_name: "file_patch".to_string(),
        reason: "modifies src/lib.rs".to_string(),
        parameters: serde_json::json!({
            "path": "src/lib.rs",
            "old_text": "a\nb\nc\nd\ne\n",
            "new_text": "a\nB\nc\nd\nE\n"
        }),
    });

    // Two hunks expected; focus starts at hunk 0, both accepted by default.
    {
        let panel = state.approval.as_ref().expect("approval panel");
        assert_eq!(panel.hunks.len(), 2);
        assert_eq!(panel.focused_hunk, Some(0));
        assert!(panel.all_hunks_accepted());
    }

    // Right arrow moves focus to hunk 1.
    handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
    );
    assert_eq!(state.approval.as_ref().unwrap().focused_hunk, Some(1));

    // Tab toggles hunk 1 off.
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert!(!state.approval.as_ref().unwrap().hunk_accepted[1]);
    assert!(!state.approval.as_ref().unwrap().all_hunks_accepted());

    // Enter on "Approve once" (default selected_index = 0) returns the
    // synthesised partial parameters.
    let outcome = handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
    match outcome {
        TuiEventOutcome::Approval {
            decision,
            synthesised_parameters,
            ..
        } => {
            assert_eq!(decision, ApprovalDecision::Approve);
            let params = synthesised_parameters
                .expect("partial parameters should be returned when a hunk is rejected");
            let new_text = params.get("new_text").and_then(|v| v.as_str()).unwrap();
            assert!(
                new_text.contains("\nB\n"),
                "accepted hunk applied: {new_text}"
            );
            assert!(
                new_text.contains("\ne"),
                "rejected hunk preserved: {new_text}"
            );
            assert!(
                !new_text.contains("E\n"),
                "rejected hunk dropped: {new_text}"
            );
        }
        other => panic!("unexpected outcome: {other:?}"),
    }
}

#[test]
fn approval_panel_returns_no_synthesised_parameters_when_all_hunks_accepted() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.apply_runtime_event(TuiRuntimeEvent::ApprovalRequested {
        tool_name: "file_patch".to_string(),
        reason: "modifies src/lib.rs".to_string(),
        parameters: serde_json::json!({
            "path": "src/lib.rs",
            "old_text": "a\nb\n",
            "new_text": "a\nB\n"
        }),
    });

    let outcome = handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
    );
    match outcome {
        TuiEventOutcome::Approval {
            decision,
            synthesised_parameters,
            ..
        } => {
            assert_eq!(decision, ApprovalDecision::Approve);
            assert!(
                synthesised_parameters.is_none(),
                "all-accepted case should reuse the original parameters: {synthesised_parameters:?}"
            );
        }
        other => panic!("unexpected outcome: {other:?}"),
    }
}

#[test]
fn approval_panel_synthesises_partial_patch_when_hunk_rejected() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.apply_runtime_event(TuiRuntimeEvent::ApprovalRequested {
        tool_name: "file_patch".to_string(),
        reason: "modifies src/lib.rs".to_string(),
        parameters: serde_json::json!({
            "path": "src/lib.rs",
            "old_text": "a\nb\nc\nd\ne\n",
            "new_text": "a\nB\nc\nd\nE\n"
        }),
    });

    let panel = state
        .approval
        .as_mut()
        .expect("approval panel populated from event");
    assert_eq!(panel.hunks.len(), 2, "two non-adjacent hunks expected");
    assert!(panel.all_hunks_accepted());

    // Reject the second hunk.
    panel.move_hunk_focus(1);
    panel.toggle_focused_hunk();

    assert!(panel.any_hunk_accepted());
    assert!(!panel.all_hunks_accepted());

    let partial = panel
        .synthesised_new_text()
        .expect("partial new_text synthesised");
    // First hunk applied → "B"; second hunk rejected → trailing "e" preserved.
    assert!(
        partial.contains("\nB\n"),
        "partial should keep accepted hunk: {partial}"
    );
    assert!(
        partial.contains("\ne"),
        "partial should keep rejected line: {partial}"
    );
    assert!(
        !partial.contains("\nE\n"),
        "partial should drop rejected hunk: {partial}"
    );
}

#[test]
fn tab_autocompletes_slash_command_prefix() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    for character in "/go".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(state.input, "/goal ");

    state.clear_input();
    for character in "/fork".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(state.input, "/fork ");

    state.clear_input();
    for character in "/compa".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(state.input, "/compact");
}

#[test]
fn tab_autocompletes_dynamic_skill_slash() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.set_skill_suggestions(vec![SkillSlashSuggestion {
        name: "auto-fix-parser".to_string(),
        description: "repair parser tests".to_string(),
        ..Default::default()
    }]);

    for character in "/auto-f".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

    assert_eq!(state.input, "/auto-fix-parser");
}

#[test]
fn tab_autocompletes_skill_name_arguments() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.set_skill_suggestions(vec![SkillSlashSuggestion {
        name: "auto-fix-parser".to_string(),
        description: "repair parser tests".to_string(),
        ..Default::default()
    }]);

    for character in "/skills show auto".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

    assert_eq!(state.input, "/skills show auto-fix-parser");
}

#[test]
fn tab_autocompletes_skills_search_subcommand_with_query_slot() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    for character in "/skills se".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(state.input, "/skills search ");
}

#[test]
fn tab_autocompletes_skills_management_subcommand_with_name_slot() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    for character in "/skills sh".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(state.input, "/skills show ");
}

#[test]
fn tab_autocompletes_archived_skill_restore_argument() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.set_skill_suggestions(vec![
        SkillSlashSuggestion {
            name: "auto-fix-parser".to_string(),
            description: "repair parser tests".to_string(),
            ..Default::default()
        },
        SkillSlashSuggestion {
            name: "old-parser".to_string(),
            archived: true,
            ..Default::default()
        },
    ]);

    for character in "/skills restore old".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

    assert_eq!(state.input, "/skills restore old-parser");
}

#[test]
fn tab_autocompletes_session_target_arguments() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.sessions = vec![
        crate::session_directory::SessionDirectoryItem::new("s-1", "parser cleanup"),
        crate::session_directory::SessionDirectoryItem::new("s-2", "release prep"),
    ];

    for character in "/session switch release".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(state.input, "/session switch s-2");

    state.clear_input();
    for character in "/session rename parser".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(state.input, "/session rename s-1 ");
}

#[test]
fn tab_autocompletes_session_subcommands_before_required_arguments() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    for character in "/session sw".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(state.input, "/session switch ");

    state.clear_input();
    for character in "/session ren".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(state.input, "/session rename ");
}

#[test]
fn tab_autocompletes_mcp_add_transport_argument() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    for character in "/mcp add local h".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(state.input, "/mcp add local http ");
}

#[test]
fn tab_autocompletes_mcp_server_name_arguments() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.side_panel.mcp_status = vec![
        McpServerSummary {
            name: "filesystem".to_string(),
            tool_count: 4,
            connected: true,
        },
        McpServerSummary {
            name: "github".to_string(),
            tool_count: 2,
            connected: false,
        },
    ];

    for character in "/mcp test git".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(state.input, "/mcp test github");
}

#[test]
fn tab_autocompletes_model_name_arguments() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "claude-sonnet-4-6",
    ));
    state.set_model_suggestions(vec![
        "claude-sonnet-4-6".to_string(),
        "gpt-5.1-codex".to_string(),
    ]);

    for character in "/model g".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(state.input, "/model gpt-5.1-codex");

    state.clear_input();
    for character in "/subagent model r".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(state.input, "/subagent model reset");
}

#[test]
fn tab_autocompletes_branch_restore_arguments() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.set_branch_suggestions(vec![
        "parser-snapshot".to_string(),
        "release-branch".to_string(),
    ]);

    for character in "/branch restore rel".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(state.input, "/branch restore release-branch");
}

#[test]
fn tab_autocompletes_branch_subcommand_with_argument_slot() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    for character in "/branch tu".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(state.input, "/branch turn ");
}

#[test]
fn tab_autocompletes_codemap_subcommand_with_argument_slot() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    for character in "/codemap loc".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(state.input, "/codemap locate ");
}

#[test]
fn tab_autocompletes_goal_and_notes_subcommands() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    for character in "/goal p".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(state.input, "/goal pause");

    state.clear_input();
    for character in "/notes l".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(state.input, "/notes last ");
}

#[test]
fn tab_autocompletes_export_artifact_arguments() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    for character in "/export a".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(state.input, "/export attachments ");

    for character in "n".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(state.input, "/export attachments notes ");
}

#[test]
fn tab_autocompletes_think_alias_arguments() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    for character in "/think ha".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(state.input, "/think hard");

    state.clear_input();
    for character in "/think st".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(state.input, "/think stop");
}

#[test]
fn tab_autocompletes_fast_and_autofix_alias_arguments() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    for character in "/fast st".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(state.input, "/fast standard");

    state.clear_input();
    for character in "/autofix f".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(state.input, "/autofix false");
}

#[test]
fn skills_slash_queues_skill_inventory_load() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    apply_slash_command(&mut state, SlashCommand::SkillList);

    assert_eq!(
        state.drain_pending_session_commands(),
        vec![SessionCommandEvent::SkillList]
    );
    assert!(
        state
            .transcript
            .last()
            .unwrap()
            .text
            .contains("skills: loading active skill inventory")
    );
}

#[test]
fn skills_pin_slash_queues_skill_pin_update() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    apply_slash_command(
        &mut state,
        SlashCommand::SkillShow("auto-fix-parser".to_string()),
    );
    apply_slash_command(&mut state, SlashCommand::SkillSearch("parser".to_string()));
    apply_slash_command(
        &mut state,
        SlashCommand::Skill {
            name: "auto-fix-parser".to_string(),
            args: "--dry".to_string(),
        },
    );
    apply_slash_command(
        &mut state,
        SlashCommand::SkillPin("auto-fix-parser".to_string()),
    );
    apply_slash_command(
        &mut state,
        SlashCommand::SkillUnpin("auto-fix-parser".to_string()),
    );
    apply_slash_command(
        &mut state,
        SlashCommand::SkillArchive("auto-fix-parser".to_string()),
    );
    apply_slash_command(
        &mut state,
        SlashCommand::SkillRestore("auto-fix-parser".to_string()),
    );
    apply_slash_command(
        &mut state,
        SlashCommand::SkillArchived("parser".to_string()),
    );

    assert_eq!(
        state.drain_pending_session_commands(),
        vec![
            SessionCommandEvent::SkillShow("auto-fix-parser".to_string()),
            SessionCommandEvent::SkillSearch("parser".to_string()),
            SessionCommandEvent::Skill {
                name: "auto-fix-parser".to_string(),
                args: "--dry".to_string(),
            },
            SessionCommandEvent::SkillPin("auto-fix-parser".to_string()),
            SessionCommandEvent::SkillUnpin("auto-fix-parser".to_string()),
            SessionCommandEvent::SkillArchive("auto-fix-parser".to_string()),
            SessionCommandEvent::SkillRestore("auto-fix-parser".to_string()),
            SessionCommandEvent::SkillArchived("parser".to_string()),
        ]
    );
}

#[test]
fn slash_picker_selects_finite_argument_options() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    for character in "/rea".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(state.input, "/reasoning ");

    handle_key_event(&mut state, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
    assert_eq!(state.input, "/reasoning low");
    assert!(state.slash_picker.is_none());

    handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
    assert_eq!(state.input, "");
    assert_eq!(state.reasoning_effort, peridot_common::ReasoningEffort::Low);
}

#[test]
fn optional_finite_slash_can_submit_or_open_options() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    for character in "/fast".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
    assert_eq!(state.service_tier.as_deref(), Some("fast"));

    for character in "/fast".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(state.input, "/fast ");
    handle_key_event(&mut state, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
    assert_eq!(state.input, "/fast off");
}

#[test]
fn arrow_keys_select_slash_command_completion() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
    );
    assert_eq!(
        state.slash_picker.as_ref().map(|picker| picker.selected),
        Some(0)
    );

    handle_key_event(&mut state, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );

    assert_eq!(state.input, "/execute");

    let outcome = handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );

    assert_eq!(outcome, TuiEventOutcome::Continue);
    assert_eq!(state.header.mode, ExecutionMode::Execute);
    assert!(state.input.is_empty());
}

#[test]
fn lang_slash_command_changes_locale() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    assert_eq!(state.config.language, Locale::En);
    for character in "/lang ko".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
    assert_eq!(state.config.language, Locale::Ko);

    for character in "/lang en".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
    assert_eq!(state.config.language, Locale::En);
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
            .any(|entry| entry.kind == TranscriptKind::Notice && entry.text.contains("queued"))
    );

    state.apply_runtime_event(TuiRuntimeEvent::Finished {
        stop_reason: "Done".to_string(),
        turns: 1,
        success: true,
        duration_ms: 0,
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
fn tool_preview_lines_render_without_inheriting_parent_icon() {
    use ratatui::{Terminal, backend::TestBackend};

    let backend = TestBackend::new(120, 32);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.resize(120, 32);
    state.apply_runtime_event(TuiRuntimeEvent::ToolStarted {
        name: "shell_exec".to_string(),
        parameters: serde_json::json!({"command": "ls"}),
    });
    state.apply_runtime_event(TuiRuntimeEvent::ToolFinished {
        name: "shell_exec".to_string(),
        success: true,
        summary: "ok".to_string(),
        output: serde_json::json!({"status": 0, "stdout": "one\n"}),
    });
    terminal.draw(|frame| draw(frame, &state)).unwrap();
    let rendered = format!("{:?}", terminal.backend().buffer());
    // Header lines keep their icon, indented preview lines must NOT.
    assert!(rendered.contains("\u{2714} shell_exec  ok"));
    assert!(!rendered.contains("\u{2714}   status"));
    assert!(!rendered.contains("\u{2714}   stdout"));
    assert!(!rendered.contains("\u{276F}   command"));
}

#[test]
fn debug_raw_entry_is_truncated_with_ellipsis() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.debug_view = true;
    let long_payload = "x".repeat(500);
    state.apply_runtime_event(TuiRuntimeEvent::AssistantDelta {
        delta: long_payload,
    });
    state.apply_runtime_event(TuiRuntimeEvent::AssistantFinished);

    let raw = state
        .transcript
        .iter()
        .find(|entry| entry.text.starts_with("assistant raw:"))
        .expect("debug raw entry");
    assert!(raw.text.ends_with("..."));
    assert!(raw.text.chars().count() < 200);
    assert!(!raw.text.contains(&"x".repeat(400)));
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
    assert!(snapshot.contains("status: tool running: shell_exec"));

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
        parameters: serde_json::json!({"command": "apt-get install"}),
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
        duration_ms: 0,
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

#[test]
fn session_new_slash_queues_pending_command_with_task() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    apply_slash_command(
        &mut state,
        SlashCommand::SessionNew(Some("rewrite README".to_string())),
    );

    let pending = state.drain_pending_session_commands();
    assert_eq!(
        pending,
        vec![SessionCommandEvent::SessionNew(Some(
            "rewrite README".to_string()
        ))]
    );
    assert!(
        state
            .transcript
            .last()
            .unwrap()
            .text
            .contains("opening new session with task 'rewrite README'")
    );
}

#[test]
fn rewind_slash_restores_prompt_and_queues_context_rollback() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.push_transcript_entry(TranscriptKind::User, "implement rewind");
    state.push_transcript_entry(TranscriptKind::Assistant, "done");

    apply_slash_command(&mut state, SlashCommand::Rewind);

    assert_eq!(state.input, "implement rewind");
    assert_eq!(
        state.drain_pending_session_commands(),
        vec![SessionCommandEvent::RewindContext]
    );
    assert!(
        state
            .transcript
            .last()
            .unwrap()
            .text
            .contains("queued context rollback")
    );
}

#[test]
fn codemap_slash_queues_pending_command() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    apply_slash_command(&mut state, SlashCommand::CodeMap);

    assert_eq!(
        state.drain_pending_session_commands(),
        vec![SessionCommandEvent::CodeMap]
    );
    assert!(
        state
            .transcript
            .last()
            .unwrap()
            .text
            .contains("codemap: loading workspace")
    );

    apply_slash_command(&mut state, SlashCommand::CodeMapStatus);

    assert_eq!(
        state.drain_pending_session_commands(),
        vec![SessionCommandEvent::CodeMapStatus]
    );
    assert!(
        state
            .transcript
            .last()
            .unwrap()
            .text
            .contains("codemap: checking workspace code map status")
    );

    apply_slash_command(&mut state, SlashCommand::CodeMapRefresh);

    assert_eq!(
        state.drain_pending_session_commands(),
        vec![SessionCommandEvent::CodeMapRefresh]
    );
    assert!(
        state
            .transcript
            .last()
            .unwrap()
            .text
            .contains("codemap: refreshing workspace")
    );

    apply_slash_command(&mut state, SlashCommand::CodeMapFind("Runner".to_string()));

    assert_eq!(
        state.drain_pending_session_commands(),
        vec![SessionCommandEvent::CodeMapFind("Runner".to_string())]
    );
    assert!(
        state
            .transcript
            .last()
            .unwrap()
            .text
            .contains("codemap: searching workspace code map")
    );

    apply_slash_command(
        &mut state,
        SlashCommand::CodeMapLocate("Runner".to_string()),
    );

    assert_eq!(
        state.drain_pending_session_commands(),
        vec![SessionCommandEvent::CodeMapLocate("Runner".to_string())]
    );
    assert!(
        state
            .transcript
            .last()
            .unwrap()
            .text
            .contains("codemap: locating symbol")
    );

    apply_slash_command(
        &mut state,
        SlashCommand::CodeMapOutline("src/lib.rs".to_string()),
    );

    assert_eq!(
        state.drain_pending_session_commands(),
        vec![SessionCommandEvent::CodeMapOutline(
            "src/lib.rs".to_string()
        )]
    );
    assert!(
        state
            .transcript
            .last()
            .unwrap()
            .text
            .contains("codemap: outlining file")
    );

    apply_slash_command(&mut state, SlashCommand::CodeMapRefs("Runner".to_string()));

    assert_eq!(
        state.drain_pending_session_commands(),
        vec![SessionCommandEvent::CodeMapRefs("Runner".to_string())]
    );
    assert!(
        state
            .transcript
            .last()
            .unwrap()
            .text
            .contains("codemap: finding references")
    );
}

#[test]
fn attach_slash_queues_pending_command() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    apply_slash_command(&mut state, SlashCommand::Attach("src/lib.rs".to_string()));

    assert_eq!(
        state.drain_pending_session_commands(),
        vec![SessionCommandEvent::Attach("src/lib.rs".to_string())]
    );
    assert!(
        state
            .transcript
            .last()
            .unwrap()
            .text
            .contains("attach: loading src/lib.rs")
    );

    apply_slash_command(&mut state, SlashCommand::Attachments);

    assert_eq!(
        state.drain_pending_session_commands(),
        vec![SessionCommandEvent::Attachments]
    );
    assert!(
        state
            .transcript
            .last()
            .unwrap()
            .text
            .contains("attachments: loading session attachment inventory")
    );

    apply_slash_command(&mut state, SlashCommand::Detach("src/lib.rs".to_string()));

    assert_eq!(
        state.drain_pending_session_commands(),
        vec![SessionCommandEvent::Detach("src/lib.rs".to_string())]
    );
    assert!(
        state
            .transcript
            .last()
            .unwrap()
            .text
            .contains("detach: removing src/lib.rs")
    );

    apply_slash_command(
        &mut state,
        SlashCommand::Export(vec![
            peridot_core::ExportArtifact::Attachments,
            peridot_core::ExportArtifact::Notes,
        ]),
    );

    assert_eq!(
        state.drain_pending_session_commands(),
        vec![SessionCommandEvent::Export(vec![
            peridot_core::ExportArtifact::Attachments,
            peridot_core::ExportArtifact::Notes,
        ])]
    );
    assert!(
        state
            .transcript
            .last()
            .unwrap()
            .text
            .contains("export: writing session artifacts")
    );
}

#[test]
fn session_switch_and_close_slashes_queue_router_intents() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.current_session_id = "s1".to_string();
    state.sessions.push(SessionDirectoryItem::new("s1", "main"));

    apply_slash_command(&mut state, SlashCommand::SessionSwitch("s2".to_string()));
    apply_slash_command(&mut state, SlashCommand::SessionClose("s1".to_string()));
    apply_slash_command(&mut state, SlashCommand::SessionDelete("s2".to_string()));
    apply_slash_command(&mut state, SlashCommand::SessionCount);
    apply_slash_command(
        &mut state,
        SlashCommand::SessionRename {
            target: "s1".to_string(),
            title: "main work".to_string(),
        },
    );

    let pending = state.drain_pending_session_commands();
    assert_eq!(
        pending,
        vec![
            SessionCommandEvent::SessionSwitch("s2".to_string()),
            SessionCommandEvent::SessionClose("s1".to_string()),
            SessionCommandEvent::SessionDelete("s2".to_string()),
            SessionCommandEvent::SessionCount,
            SessionCommandEvent::SessionRename {
                target: "s1".to_string(),
                title: "main work".to_string(),
            },
        ]
    );
}

#[test]
fn fork_teammate_worktree_slashes_queue_pending_commands() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    apply_slash_command(&mut state, SlashCommand::Fork("audit lib.rs".to_string()));
    apply_slash_command(
        &mut state,
        SlashCommand::Teammate("compile checks".to_string()),
    );
    apply_slash_command(
        &mut state,
        SlashCommand::Worktree {
            branch: "wt/audit".to_string(),
            task: "audit branch".to_string(),
        },
    );

    let pending = state.drain_pending_session_commands();
    assert_eq!(
        pending,
        vec![
            SessionCommandEvent::Fork("audit lib.rs".to_string()),
            SessionCommandEvent::Teammate("compile checks".to_string()),
            SessionCommandEvent::Worktree {
                branch: "wt/audit".to_string(),
                task: "audit branch".to_string(),
            },
        ]
    );
}

#[test]
fn record_background_event_updates_directory_stats_and_attention() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state
        .sessions
        .push(SessionDirectoryItem::new("bg", "background"));

    state.record_background_event(
        "bg",
        &TuiRuntimeEvent::RunStarted {
            task: "audit".to_string(),
        },
    );
    state.record_background_event(
        "bg",
        &TuiRuntimeEvent::UsageUpdated {
            total_tokens: 1_400,
            cache_hit_rate: 0.5,
            cost_usd: 0.04,
        },
    );
    state.record_background_event(
        "bg",
        &TuiRuntimeEvent::ApprovalRequested {
            tool_name: "shell_exec".to_string(),
            reason: "destructive shell command".to_string(),
            parameters: serde_json::json!({"command": "rm -rf /tmp/cache"}),
        },
    );

    let item = state.sessions.iter().find(|s| s.id == "bg").unwrap();
    assert_eq!(item.status, AgentRunStatus::Running);
    assert_eq!(item.tokens, 1_400);
    assert!((item.cost_usd - 0.04).abs() < f64::EPSILON);
    assert!(item.pending_attention);
    assert!(item.last_event_at_unix > 0);
}

#[test]
fn record_background_event_promotes_child_into_subagent_monitor() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.current_session_id = "parent".to_string();
    state
        .sessions
        .push(SessionDirectoryItem::new("parent", "main"));
    let child = SessionDirectoryItem::new("fork-1", "audit lib.rs").with_parent("parent", "fork");
    state.sessions.push(child);

    state.record_background_event(
        "fork-1",
        &TuiRuntimeEvent::RunStarted {
            task: "audit lib.rs".to_string(),
        },
    );
    state.record_background_event(
        "fork-1",
        &TuiRuntimeEvent::UsageUpdated {
            total_tokens: 800,
            cache_hit_rate: 0.0,
            cost_usd: 0.01,
        },
    );

    let monitor = state
        .subagents
        .iter()
        .find(|item| item.id == "fork-1")
        .expect("subagent monitor entry should be created for the child");
    assert_eq!(monitor.kind, "fork");
    assert_eq!(monitor.parent_id.as_deref(), Some("parent"));
    assert_eq!(monitor.task, "audit lib.rs");
    assert_eq!(monitor.tokens, 800);
}

#[test]
fn record_background_event_skips_subagent_monitor_when_parent_not_foreground() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.current_session_id = "other".to_string();
    state
        .sessions
        .push(SessionDirectoryItem::new("other", "other"));
    let child = SessionDirectoryItem::new("fork-2", "compile checks").with_parent("parent", "fork");
    state.sessions.push(child);

    state.record_background_event(
        "fork-2",
        &TuiRuntimeEvent::RunStarted {
            task: "compile checks".to_string(),
        },
    );

    assert!(
        state.subagents.iter().all(|item| item.id != "fork-2"),
        "subagent monitor must only follow the foreground parent"
    );
}

#[test]
fn committee_slash_toggles_mode_and_status_metrics() {
    use peridot_common::CommitteeMode;

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    assert_eq!(state.committee_mode, CommitteeMode::Off);
    let snapshot_off = render_text_snapshot(&state);
    assert!(!snapshot_off.contains("committee "));

    apply_slash_command(&mut state, SlashCommand::Committee(CommitteeMode::Full));
    assert_eq!(state.committee_mode, CommitteeMode::Full);
    let snapshot_full = render_text_snapshot(&state);
    assert!(snapshot_full.contains("committee full"));
    assert!(
        state
            .transcript
            .iter()
            .any(|line| line.text.contains("committee: off -> full"))
    );
}

#[test]
fn parse_slash_committee_recognises_all_modes() {
    use peridot_common::CommitteeMode;
    assert_eq!(
        peridot_core::parse_slash_command("/committee off"),
        Some(SlashCommand::Committee(CommitteeMode::Off))
    );
    assert_eq!(
        peridot_core::parse_slash_command("/committee planner"),
        Some(SlashCommand::Committee(CommitteeMode::Planner))
    );
    assert_eq!(
        peridot_core::parse_slash_command("/committee full"),
        Some(SlashCommand::Committee(CommitteeMode::Full))
    );
    assert_eq!(peridot_core::parse_slash_command("/committee bogus"), None);
}

#[test]
fn committee_events_drain_into_pending_journal_queue() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.committee_mode = peridot_common::CommitteeMode::Full;

    state.apply_runtime_event(TuiRuntimeEvent::PlannerPlanReady {
        plan_text: "1. read 2. edit".to_string(),
    });
    state.apply_runtime_event(TuiRuntimeEvent::ReviewerVerdict {
        turn_index: 1,
        verdict: "request_changes".to_string(),
        comments: "indent off".to_string(),
    });
    state.apply_runtime_event(TuiRuntimeEvent::CommitteeRoleUsage {
        role: "reviewer".to_string(),
        cost_usd: 0.0042,
        tokens: 120,
    });

    let drained = state.drain_pending_committee_events();
    assert_eq!(drained.len(), 3);
    assert_eq!(drained[0]["kind"].as_str(), Some("planner_plan_ready"));
    assert_eq!(drained[1]["kind"].as_str(), Some("reviewer_verdict"));
    assert_eq!(drained[1]["verdict"].as_str(), Some("request_changes"));
    assert_eq!(drained[2]["kind"].as_str(), Some("role_usage"));
    assert!(state.pending_committee_events.is_empty());
}

#[test]
fn transcript_entry_missing_timestamp_defaults_to_zero() {
    let entry: TranscriptEntry =
        serde_json::from_str(r#"{"kind":"system","text":"old row"}"#).unwrap();

    assert_eq!(entry.ts, 0);
    assert_eq!(entry.text, "old row");
}

#[test]
fn committee_role_usage_event_accumulates_into_per_role_totals() {
    use peridot_common::CommitteeMode;

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.committee_mode = CommitteeMode::Full;

    state.apply_runtime_event(TuiRuntimeEvent::CommitteeRoleUsage {
        role: "planner".to_string(),
        cost_usd: 0.0123,
        tokens: 450,
    });
    state.apply_runtime_event(TuiRuntimeEvent::CommitteeRoleUsage {
        role: "reviewer".to_string(),
        cost_usd: 0.0042,
        tokens: 120,
    });
    state.apply_runtime_event(TuiRuntimeEvent::CommitteeRoleUsage {
        role: "reviewer".to_string(),
        cost_usd: 0.0058,
        tokens: 180,
    });
    state.apply_runtime_event(TuiRuntimeEvent::CommitteeRoleUsage {
        role: "unknown".to_string(),
        cost_usd: 9.99,
        tokens: 999,
    });

    assert!((state.committee_planner_cost - 0.0123).abs() < 1e-9);
    assert_eq!(state.committee_planner_tokens, 450);
    assert!((state.committee_reviewer_cost - 0.0100).abs() < 1e-9);
    assert_eq!(state.committee_reviewer_tokens, 300);

    apply_slash_command(&mut state, SlashCommand::Cost);
    let line = state.transcript.last().unwrap().text.clone();
    assert!(line.contains("committee cost: planner $0.0123"));
    assert!(line.contains("reviewer $0.0100"));
}

#[test]
fn reviewer_verdict_event_renders_with_kind_per_outcome() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.apply_runtime_event(TuiRuntimeEvent::ReviewerVerdict {
        turn_index: 0,
        verdict: "approve".to_string(),
        comments: String::new(),
    });
    state.apply_runtime_event(TuiRuntimeEvent::ReviewerVerdict {
        turn_index: 1,
        verdict: "request_changes".to_string(),
        comments: "indent is off".to_string(),
    });
    state.apply_runtime_event(TuiRuntimeEvent::ReviewerVerdict {
        turn_index: 2,
        verdict: "block".to_string(),
        comments: "writes outside workspace".to_string(),
    });

    let kinds: Vec<_> = state.transcript.iter().map(|e| e.kind).collect();
    assert!(kinds.contains(&TranscriptKind::System));
    assert!(kinds.contains(&TranscriptKind::Notice));
    assert!(kinds.contains(&TranscriptKind::Error));

    let snapshot = render_text_snapshot(&state);
    assert!(snapshot.contains("approve"));
    assert!(snapshot.contains("indent is off"));
    assert!(snapshot.contains("writes outside workspace"));
}

#[test]
fn planner_plan_ready_event_lands_in_transcript() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.apply_runtime_event(TuiRuntimeEvent::PlannerPlanReady {
        plan_text: "1. read AGENTS.md\n2. update lib.rs".to_string(),
    });
    let last = state.transcript.last().unwrap();
    assert!(last.text.contains("committee planner ready"));
    assert!(last.text.contains("update lib.rs"));
}

#[test]
fn ask_user_freeform_accepts_shift_enter_and_ctrl_j_newline() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.open_ask_user(AskUserRequest::FreeForm {
        question: "summary?".to_string(),
        hint: None,
        default: None,
    });
    for character in "line1".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT),
    );
    for character in "line2".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
    handle_key_event(
        &mut state,
        KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
    );
    for character in "line3".chars() {
        handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }

    let panel = state.ask_user.as_ref().expect("ask_user panel");
    assert_eq!(panel.freeform, "line1\nline2\nline3");
}

#[test]
fn note_slash_queues_pending_note_and_records_transcript() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    apply_slash_command(
        &mut state,
        SlashCommand::Note("kickoff observed for the migration".to_string()),
    );

    let drained = state.drain_pending_notes();
    assert_eq!(
        drained,
        vec!["kickoff observed for the migration".to_string()]
    );
    assert!(
        state
            .transcript
            .iter()
            .any(|line| line.text.contains("note: kickoff observed"))
    );
}

#[test]
fn notes_slash_queues_note_inventory_load() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    apply_slash_command(&mut state, SlashCommand::Notes(Some(2)));

    assert_eq!(
        state.drain_pending_session_commands(),
        vec![SessionCommandEvent::Notes(Some(2))]
    );
    assert!(
        state
            .transcript
            .iter()
            .any(|line| line.text.contains("notes: loading"))
    );
}

#[test]
fn info_slash_command_summarises_session_metadata() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "claude-sonnet-4-6",
    ));
    state.current_session_id = "sess-abc".to_string();
    state.header.workspace_label = Some("peridot-agent".to_string());
    state.header.provider = Some("openai-api".to_string());
    state.current_turn = 5;

    apply_slash_command(&mut state, SlashCommand::Info);

    let line = state.transcript.last().unwrap().text.clone();
    assert!(line.contains("session sess-abc"));
    assert!(line.contains("workspace peridot-agent"));
    assert!(line.contains("model claude-sonnet-4-6"));
    assert!(line.contains("provider openai-api"));
    assert!(line.contains("turn 5"));
}

#[test]
fn status_metrics_show_active_subagent_count() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.subagents.push(SubagentMonitorItem {
        kind: "fork".to_string(),
        task: "audit lib".to_string(),
        status: "running".to_string(),
        summary: None,
        id: "fork-1".to_string(),
        parent_id: Some("parent".to_string()),
        depth: 1,
        started_at_unix: 0,
        tokens: 0,
    });
    state.subagents.push(SubagentMonitorItem {
        kind: "fork".to_string(),
        task: "verify".to_string(),
        status: "done".to_string(),
        summary: Some("ok".to_string()),
        id: "fork-2".to_string(),
        parent_id: Some("parent".to_string()),
        depth: 1,
        started_at_unix: 0,
        tokens: 0,
    });

    let snapshot = render_text_snapshot(&state);
    assert!(snapshot.contains("subagents 1"));
}

#[test]
fn status_metrics_show_turn_count_after_first_turn() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.current_turn = 3;

    let snapshot = render_text_snapshot(&state);
    assert!(snapshot.contains("turn 3"));
}

#[test]
fn status_metrics_show_aggregate_usage_for_multi_session() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.current_session_id = "s1".to_string();
    state.header.cost_usd = 0.10;
    state.header.total_tokens = 2_000;
    state
        .sessions
        .push(crate::SessionDirectoryItem::new("s1", "main"));
    let mut bg = crate::SessionDirectoryItem::new("s2", "background");
    bg.cost_usd = 0.04;
    bg.tokens = 500;
    state.sessions.push(bg);

    let snapshot = render_text_snapshot(&state);

    assert!(snapshot.contains("2000 tok"));
    assert!(snapshot.contains("$0.1000"));
    assert!(snapshot.contains("all 2500 tok / $0.1400"));
}

#[test]
fn status_metrics_drop_low_priority_parts_when_narrow() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.header.workspace_label = Some("peridot-agent".to_string());
    state.header.provider = Some("openrouter-api".to_string());
    state.header.cost_usd = 0.10;
    state.header.total_tokens = 2_000;
    state.header.cache_hit_rate = 0.87;
    state.current_turn = 3;
    state.current_session_id = "s1".to_string();
    state.agent_run_status = AgentRunStatus::Running;
    state.side_panel.stats.elapsed_seconds = 8;
    state
        .sessions
        .push(crate::SessionDirectoryItem::new("s1", "main"));
    let mut bg = crate::SessionDirectoryItem::new("s2", "background");
    bg.cost_usd = 0.04;
    bg.tokens = 500;
    state.sessions.push(bg);
    state.subagents.push(SubagentMonitorItem {
        kind: "fork".to_string(),
        task: "audit lib".to_string(),
        status: "running".to_string(),
        summary: None,
        id: "fork-1".to_string(),
        parent_id: Some("parent".to_string()),
        depth: 1,
        started_at_unix: 0,
        tokens: 0,
    });

    let full = render_status_metrics(&state);
    assert!(full.contains("provider openrouter-api"));
    assert!(full.contains("cache 87%"));
    assert!(full.contains("subagents 1"));
    assert!(full.contains("all 2500 tok / $0.1400"));

    let compact = render_status_metrics_for_width(&state, 42);
    assert!(compact.contains("execute · auto"));
    assert!(compact.contains("agent running"));
    assert!(compact.contains("8s"));
    assert!(!compact.contains("provider openrouter-api"));
    assert!(!compact.contains("cache 87%"));
    assert!(!compact.contains("subagents 1"));
    assert!(unicode_width::UnicodeWidthStr::width(compact.as_str()) <= 42);
}

#[test]
fn status_metrics_omit_redundant_aggregate_usage() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.current_session_id = "s1".to_string();
    state.header.cost_usd = 0.10;
    state.header.total_tokens = 2_000;
    state
        .sessions
        .push(crate::SessionDirectoryItem::new("s1", "main"));
    state
        .sessions
        .push(crate::SessionDirectoryItem::new("s2", "idle"));

    let snapshot = render_text_snapshot(&state);

    assert!(!snapshot.contains("all 2000 tok"));
    assert!(!snapshot.contains("all $0.1000"));
}

#[test]
fn workspace_label_appears_in_status_metrics_when_set() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.header.workspace_label = Some("peridot-agent".to_string());

    let snapshot = render_text_snapshot(&state);
    assert!(snapshot.contains("workspace peridot-agent"));
}

#[test]
fn provider_slash_command_updates_header_and_status_metrics() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));

    apply_slash_command(&mut state, SlashCommand::Provider("openai-api".to_string()));

    assert_eq!(state.header.provider.as_deref(), Some("openai-api"));
    let snapshot = render_text_snapshot(&state);
    assert!(snapshot.contains("provider openai-api"));
    assert!(
        state
            .transcript
            .iter()
            .any(|line| line.text.contains("provider: openai-api"))
    );
}

#[test]
fn parse_slash_command_accepts_provider() {
    assert_eq!(
        peridot_core::parse_slash_command("/provider claude-api"),
        Some(SlashCommand::Provider("claude-api".to_string())),
    );
    // After the auto-skill slash registration (see SlashCommand::Skill),
    // bare `/provider` no longer returns None — the parser falls
    // through to the kebab-case skill-lookup gate. The dispatcher
    // surfaces "skill not found: provider" instead, which still tells
    // the operator the form was wrong but goes through the skill path
    // rather than the legacy "invalid slash" path.
    assert!(matches!(
        peridot_core::parse_slash_command("/provider"),
        Some(SlashCommand::Skill { ref name, .. }) if name == "provider"
    ));
}

#[test]
fn swap_foreground_state_round_trips_transcripts_between_sessions() {
    use std::collections::HashMap;

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.current_session_id = "A".to_string();
    state.sessions.push(SessionDirectoryItem::new("A", "alpha"));
    state.sessions.push(SessionDirectoryItem::new("B", "beta"));
    state.push_transcript("hello from A");

    state.current_session_id = "B".to_string();
    let mut other_states: HashMap<String, TuiState> = HashMap::new();
    swap_foreground_state(&mut state, &mut other_states, "A");

    assert_eq!(state.current_session_id, "B");
    assert!(
        state.transcript.is_empty(),
        "freshly-swapped foreground starts with a clean transcript"
    );
    assert_eq!(state.sessions.len(), 2);
    assert!(other_states.contains_key("A"));
    assert_eq!(other_states["A"].transcript.len(), 1);
    assert_eq!(other_states["A"].transcript[0].text, "hello from A");

    state.push_transcript("hello from B");
    state.current_session_id = "A".to_string();
    swap_foreground_state(&mut state, &mut other_states, "B");

    assert_eq!(state.current_session_id, "A");
    assert_eq!(state.transcript.len(), 1);
    assert_eq!(state.transcript[0].text, "hello from A");
    assert!(other_states.contains_key("B"));
    assert_eq!(other_states["B"].transcript.len(), 1);
    assert_eq!(other_states["B"].transcript[0].text, "hello from B");
}

#[test]
fn swap_foreground_state_keeps_input_history_per_session() {
    use std::collections::HashMap;

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.current_session_id = "A".to_string();
    state.sessions.push(SessionDirectoryItem::new("A", "alpha"));
    state.sessions.push(SessionDirectoryItem::new("B", "beta"));
    state.record_input_history("ask from A");
    state.record_input_history("second from A");

    state.current_session_id = "B".to_string();
    let mut other_states: HashMap<String, TuiState> = HashMap::new();
    swap_foreground_state(&mut state, &mut other_states, "A");

    assert_eq!(state.current_session_id, "B");
    assert!(state.input_history.is_empty());
    state.record_input_history("ask from B");

    state.current_session_id = "A".to_string();
    swap_foreground_state(&mut state, &mut other_states, "B");

    assert_eq!(
        state.input_history,
        vec!["ask from A".to_string(), "second from A".to_string()]
    );
    state.previous_input_history();
    assert_eq!(state.input, "second from A");

    state.current_session_id = "B".to_string();
    swap_foreground_state(&mut state, &mut other_states, "A");

    assert_eq!(state.input_history, vec!["ask from B".to_string()]);
    state.previous_input_history();
    assert_eq!(state.input, "ask from B");
}

#[test]
fn swap_foreground_state_noops_when_target_matches_previous() {
    use std::collections::HashMap;

    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.current_session_id = "A".to_string();
    state.push_transcript("only line");
    let mut other_states: HashMap<String, TuiState> = HashMap::new();

    swap_foreground_state(&mut state, &mut other_states, "A");

    assert_eq!(state.current_session_id, "A");
    assert_eq!(state.transcript.len(), 1);
    assert!(other_states.is_empty());
}

#[test]
fn pending_attention_count_skips_foreground_session() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.current_session_id = "foreground".to_string();
    let mut fg = SessionDirectoryItem::new("foreground", "main");
    fg.pending_attention = true;
    state.sessions.push(fg);
    let mut bg = SessionDirectoryItem::new("bg", "background");
    bg.pending_attention = true;
    state.sessions.push(bg);

    assert_eq!(state.pending_attention_count(), 1);
}

#[test]
fn status_snapshot_surfaces_pending_attention_count_in_locale() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.current_session_id = "foreground".to_string();
    state
        .sessions
        .push(SessionDirectoryItem::new("foreground", "main"));
    let mut bg = SessionDirectoryItem::new("bg", "background");
    bg.pending_attention = true;
    state.sessions.push(bg);

    let snapshot = render_text_snapshot(&state);
    assert!(snapshot.contains("attention: 1 sessions need attention"));

    state.config.language = Locale::Ko;
    let snapshot_ko = render_text_snapshot(&state);
    assert!(snapshot_ko.contains("attention: 1개 세션이 응답 대기 중"));
}

#[test]
fn record_background_event_marks_finished_status_per_stop_reason() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.sessions.push(SessionDirectoryItem::new("a", "a"));
    state.sessions.push(SessionDirectoryItem::new("b", "b"));
    state.sessions.push(SessionDirectoryItem::new("c", "c"));

    state.record_background_event(
        "a",
        &TuiRuntimeEvent::Finished {
            stop_reason: "Done".to_string(),
            turns: 1,
            success: true,
            duration_ms: 0,
        },
    );
    state.record_background_event(
        "b",
        &TuiRuntimeEvent::Finished {
            stop_reason: "Interrupted".to_string(),
            turns: 1,
            success: false,
            duration_ms: 0,
        },
    );
    state.record_background_event(
        "c",
        &TuiRuntimeEvent::Failed {
            message: "boom".to_string(),
        },
    );

    let by_id = |id: &str| {
        state
            .sessions
            .iter()
            .find(|s| s.id == id)
            .unwrap()
            .status
            .clone()
    };
    assert_eq!(by_id("a"), AgentRunStatus::Succeeded);
    assert_eq!(by_id("b"), AgentRunStatus::Interrupted);
    assert_eq!(by_id("c"), AgentRunStatus::Failed);
}

#[test]
fn aggregate_cost_with_no_sessions_uses_header() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.header.cost_usd = 0.05;
    state.header.total_tokens = 1000;
    assert!((state.aggregate_cost_usd() - 0.05).abs() < 1e-9);
    assert_eq!(state.aggregate_tokens(), 1000);
}

#[test]
fn aggregate_cost_sums_multiple_sessions() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.current_session_id = "s1".to_string();
    state.header.cost_usd = 0.10;
    state.header.total_tokens = 2000;
    state
        .sessions
        .push(crate::SessionDirectoryItem::new("s1", "main"));
    state.sessions.last_mut().unwrap().cost_usd = 0.08;
    state.sessions.last_mut().unwrap().tokens = 1800;
    let mut bg = crate::SessionDirectoryItem::new("s2", "background");
    bg.cost_usd = 0.04;
    bg.tokens = 500;
    state.sessions.push(bg);
    // Foreground uses max(header, item): max(0.10, 0.08) = 0.10
    // Background: 0.04
    assert!((state.aggregate_cost_usd() - 0.14).abs() < 1e-9);
    // Foreground tokens: max(2000, 1800) = 2000; background: 500
    assert_eq!(state.aggregate_tokens(), 2500);
}

#[test]
fn cost_command_shows_aggregate_for_multi_session() {
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.current_session_id = "s1".to_string();
    state.header.cost_usd = 0.10;
    state.header.total_tokens = 2000;
    state
        .sessions
        .push(crate::SessionDirectoryItem::new("s1", "main"));
    let mut bg = crate::SessionDirectoryItem::new("s2", "bg");
    bg.cost_usd = 0.05;
    bg.tokens = 700;
    state.sessions.push(bg);
    apply_slash_command(&mut state, SlashCommand::Cost);
    let text: String = state
        .transcript
        .iter()
        .map(|e| e.text.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("aggregate"));
    assert!(text.contains("2 sessions"));
}
