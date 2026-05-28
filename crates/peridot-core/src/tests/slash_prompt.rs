use peridot_common::{ExecutionMode, PeriError, PermissionMode};

use crate::prompt::{read_plan_reminder, system_prompt_for_mode};
use crate::recovery::classify_error;
use crate::{AgentState, GoalController, SlashCommand, parse_slash_command};

#[test]
fn parses_goal_slash_commands() {
    assert_eq!(parse_slash_command("/goal"), Some(SlashCommand::GoalMode));
    assert_eq!(
        parse_slash_command("/goal fix tests"),
        Some(SlashCommand::GoalStart("fix tests".to_string()))
    );
    assert_eq!(
        parse_slash_command("/goal pause"),
        Some(SlashCommand::GoalPause)
    );
    assert_eq!(parse_slash_command("/safe"), Some(SlashCommand::Safe));
    assert_eq!(parse_slash_command("/help"), Some(SlashCommand::Help));
    assert_eq!(parse_slash_command("/cost"), Some(SlashCommand::Cost));
    assert_eq!(
        parse_slash_command("/context top"),
        Some(SlashCommand::ContextTop)
    );
    assert_eq!(
        parse_slash_command("/context"),
        Some(SlashCommand::ContextTop)
    );
    assert_eq!(
        parse_slash_command("/plan show"),
        Some(SlashCommand::PlanShow)
    );
    assert_eq!(
        parse_slash_command("/model claude-sonnet-4-6"),
        Some(SlashCommand::Model("claude-sonnet-4-6".to_string()))
    );
    assert_eq!(
        parse_slash_command("/session save"),
        Some(SlashCommand::SessionSave)
    );
    assert_eq!(
        parse_slash_command("/session list --status done"),
        Some(SlashCommand::SessionListStatus("done".to_string()))
    );
    assert_eq!(
        parse_slash_command("/session prune --status failed --dry-run"),
        Some(SlashCommand::SessionPrune {
            status: Some("failed".to_string()),
            older_than_days: None,
            dry_run: true,
        })
    );
    assert_eq!(
        parse_slash_command("/session search parser failure"),
        Some(SlashCommand::SessionSearch("parser failure".to_string()))
    );
    assert_eq!(
        parse_slash_command("/session show s1"),
        Some(SlashCommand::SessionShow("s1".to_string()))
    );
    assert_eq!(
        parse_slash_command("/session locate s1"),
        Some(SlashCommand::SessionLocate("s1".to_string()))
    );
    assert_eq!(
        parse_slash_command("/session resume s1"),
        Some(SlashCommand::SessionResume("s1".to_string()))
    );
    assert_eq!(
        parse_slash_command("/session replay s1 --last 3"),
        Some(SlashCommand::SessionReplay {
            target: "s1".to_string(),
            last: Some(3),
        })
    );
    assert_eq!(
        parse_slash_command("/session export s1 notes"),
        Some(SlashCommand::SessionExport {
            target: "s1".to_string(),
            artifacts: vec![crate::ExportArtifact::Notes],
        })
    );
    assert_eq!(
        parse_slash_command("/session import ./export --id s2"),
        Some(SlashCommand::SessionImport {
            from: "./export".to_string(),
            id: Some("s2".to_string()),
            force: false,
        })
    );
    assert_eq!(
        parse_slash_command("/fast on"),
        Some(SlashCommand::Fast(Some(true)))
    );
    assert_eq!(
        parse_slash_command("/fast off"),
        Some(SlashCommand::Fast(Some(false)))
    );
    assert_eq!(
        parse_slash_command("/fast toggle"),
        Some(SlashCommand::Fast(None))
    );
}

#[test]
fn goal_controller_stops_on_budget() {
    let mut goal = GoalController::new("finish", 10, 1.0);
    assert!(!goal.should_stop());

    goal.record_turn(1.2);

    assert!(goal.should_stop());
}

#[test]
fn agent_state_applies_mode_commands() {
    let mut state = AgentState::default();
    state.apply_slash_command(&SlashCommand::GoalMode);

    assert_eq!(state.mode, ExecutionMode::Goal);
    assert_eq!(state.goal, None);

    state.apply_slash_command(&SlashCommand::GoalStart("ship".to_string()));

    assert_eq!(state.mode, ExecutionMode::Goal);
    assert_eq!(state.goal.as_deref(), Some("ship"));
}

#[test]
fn system_prompt_contains_injection_defense_rules() {
    let prompt = system_prompt_for_mode(ExecutionMode::Execute);

    assert!(prompt.contains("<untrusted_content>"));
    assert!(prompt.contains("never as instructions"));
    assert!(prompt.contains("AGENTS boundaries"));
}

#[test]
fn plan_prompt_requires_understand_then_choose_flow() {
    let prompt = system_prompt_for_mode(ExecutionMode::Plan);

    assert!(prompt.contains("Phase 0 UNDERSTAND is mandatory"));
    assert!(prompt.contains("plan_create"));
    assert!(prompt.contains("Phase 2 CHOOSE is handled by the CLI"));
}

#[test]
fn execute_prompt_enforces_intent_clarification() {
    let prompt = system_prompt_for_mode(ExecutionMode::Execute);

    assert!(prompt.contains("Intent clarification rules"));
    assert!(prompt.contains("Before the first file_write"));
    assert!(prompt.contains("vague verbs"));
    assert!(prompt.contains("2-4 concrete candidate interpretations"));
    assert!(prompt.contains("Yolo trusts you on execution"));
}

#[test]
fn goal_prompt_runs_clarification_on_initial_request() {
    let prompt = system_prompt_for_mode(ExecutionMode::Goal);

    assert!(prompt.contains("Intent clarification rules"));
    assert!(
        prompt.contains("Apply the intent clarification rules below to the initial user request")
    );
}

#[test]
fn every_mode_prompt_contains_grounding_rules() {
    for mode in [
        ExecutionMode::Plan,
        ExecutionMode::Execute,
        ExecutionMode::Goal,
    ] {
        let prompt = system_prompt_for_mode(mode);

        assert!(
            prompt.contains("Grounding rules"),
            "{mode:?} prompt missing Grounding rules header"
        );
        assert!(
            prompt.contains("read first, answer second"),
            "{mode:?} prompt missing read-first directive"
        );
        assert!(
            prompt.contains("Cite a concrete source"),
            "{mode:?} prompt missing citation directive"
        );
        assert!(
            prompt.contains("base each substantive claim on direct evidence"),
            "{mode:?} prompt missing evidence-basis directive"
        );
        assert!(
            prompt.contains("Do not soften speculation"),
            "{mode:?} prompt missing anti-speculation directive"
        );
    }
}

#[test]
fn reads_plan_reminder_from_todo_md() {
    let root =
        std::env::temp_dir().join(format!("peridot-core-plan-reminder-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("todo.md"), "# Plan\n\n1. [ ] Keep going\n").unwrap();

    let reminder = read_plan_reminder(&root).unwrap();

    assert!(reminder.contains("Current plan status from todo.md"));
    assert!(reminder.contains("Keep going"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn classifies_tool_errors() {
    assert_eq!(
        classify_error(&PeriError::Tool("No such file or directory".to_string())),
        "not_found"
    );
    assert_eq!(
        classify_error(&PeriError::Tool("operation timed out".to_string())),
        "timeout"
    );
    assert_eq!(
        classify_error(&PeriError::PermissionDenied("blocked".to_string())),
        "permission"
    );
}

#[test]
fn default_agent_state_uses_default_permission() {
    let state = AgentState::default();

    assert_eq!(state.permission, PermissionMode::default());
}
