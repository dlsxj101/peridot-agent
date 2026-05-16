use std::os::unix::fs::PermissionsExt;

use peridot_common::{
    AgentPhase, ExecutionMode, HookConfig, HookFailureMode, HooksConfig, PermissionMode,
    SecurityConfig,
};
use peridot_context::{ContextManager, ContextSource};
use peridot_tools::{ToolRegistry, register_builtin_tools};
use serde_json::json;

use crate::{AgentRunRequest, AgentState, HarnessAgent, StopReason};

use super::support::StaticProvider;

#[tokio::test]
async fn run_until_done_stops_on_budget() {
    let root = std::env::temp_dir().join(format!("peridot-core-budget-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry).unwrap();
    let mut agent = HarnessAgent::new(
        AgentState::new(ExecutionMode::Execute, PermissionMode::Auto),
        ContextManager::new(),
        registry,
    );
    let provider = StaticProvider::with_cost(
        vec![json!({"action":"plan_update","parameters":{"update":"one"}}).to_string()],
        0.25,
    );

    let summary = agent
        .run_until_done(
            &provider,
            AgentRunRequest {
                task: "spend budget".to_string(),
                model: "mock".to_string(),
                goal_checker_model: None,
                max_turns: 4,
                max_tokens: 512,
                reasoning_effort: peridot_common::ReasoningEffort::Off,
                budget_usd: 0.1,
                budget_warning_pct: 50,
                project_root: root.clone(),
                denied_paths: Vec::new(),
                hooks: HooksConfig::default(),
                security: SecurityConfig::default(),
            },
        )
        .await
        .unwrap();

    assert_eq!(summary.stopped_reason, StopReason::Budget);
    assert_eq!(summary.turns.len(), 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn run_until_done_emits_budget_warning_hook() {
    let root =
        std::env::temp_dir().join(format!("peridot-core-budget-hook-{}", std::process::id()));
    let hooks_dir = root.join(".peridot/hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    let script = hooks_dir.join("budget.sh");
    std::fs::write(&script, "#!/bin/sh\necho \"$1\" >> budget.log\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry).unwrap();
    let mut agent = HarnessAgent::new(
        AgentState::new(ExecutionMode::Execute, PermissionMode::Auto),
        ContextManager::new(),
        registry,
    );
    let provider = StaticProvider::with_cost(
        vec![json!({"action":"agent_done","parameters":{"summary":"done"}}).to_string()],
        0.06,
    );

    agent
        .run_until_done(
            &provider,
            AgentRunRequest {
                task: "spend half".to_string(),
                model: "mock".to_string(),
                goal_checker_model: None,
                max_turns: 1,
                max_tokens: 512,
                reasoning_effort: peridot_common::ReasoningEffort::Off,
                budget_usd: 0.1,
                budget_warning_pct: 50,
                project_root: root.clone(),
                denied_paths: Vec::new(),
                hooks: HooksConfig {
                    event: vec![HookConfig {
                        event: "budget_warning".to_string(),
                        run: ".peridot/hooks/budget.sh {percentage}".to_string(),
                        description: None,
                        on_failure: HookFailureMode::Block,
                        only_paths: Vec::new(),
                    }],
                    ..HooksConfig::default()
                },
                security: SecurityConfig::default(),
            },
        )
        .await
        .unwrap();

    let log = std::fs::read_to_string(root.join("budget.log")).unwrap();
    assert!(log.contains("60"));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn goal_budget_warning_is_injected_into_context() {
    let root = std::env::temp_dir().join(format!(
        "peridot-core-goal-budget-warning-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry).unwrap();
    let mut agent = HarnessAgent::new(
        AgentState::new(ExecutionMode::Goal, PermissionMode::Auto),
        ContextManager::new(),
        registry,
    );
    let provider = StaticProvider::with_cost(
        vec![
            json!({"action":"plan_update","parameters":{"update":"one more step"}}).to_string(),
            json!({"action":"agent_done","parameters":{"summary":"done"}}).to_string(),
        ],
        0.06,
    );

    let summary = agent
        .run_until_done(
            &provider,
            AgentRunRequest {
                task: "watch budget".to_string(),
                model: "mock".to_string(),
                goal_checker_model: None,
                max_turns: 2,
                max_tokens: 512,
                reasoning_effort: peridot_common::ReasoningEffort::Off,
                budget_usd: 1.0,
                budget_warning_pct: 5,
                project_root: root.clone(),
                denied_paths: Vec::new(),
                hooks: HooksConfig::default(),
                security: SecurityConfig::default(),
            },
        )
        .await
        .unwrap();

    assert_eq!(summary.stopped_reason, StopReason::Done);
    assert!(agent.context().entries().iter().any(|entry| {
        entry.source == ContextSource::PlanReminder && entry.content.contains("Budget warning")
    }));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn goal_budget_stop_injects_pause_directive() {
    let root = std::env::temp_dir().join(format!(
        "peridot-core-goal-budget-stop-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry).unwrap();
    let mut agent = HarnessAgent::new(
        AgentState::new(ExecutionMode::Goal, PermissionMode::Auto),
        ContextManager::new(),
        registry,
    );
    let provider = StaticProvider::with_cost(
        vec![json!({"action":"plan_update","parameters":{"update":"costly"}}).to_string()],
        0.2,
    );

    let summary = agent
        .run_until_done(
            &provider,
            AgentRunRequest {
                task: "hit budget".to_string(),
                model: "mock".to_string(),
                goal_checker_model: None,
                max_turns: 2,
                max_tokens: 512,
                reasoning_effort: peridot_common::ReasoningEffort::Off,
                budget_usd: 0.1,
                budget_warning_pct: 50,
                project_root: root.clone(),
                denied_paths: Vec::new(),
                hooks: HooksConfig::default(),
                security: SecurityConfig::default(),
            },
        )
        .await
        .unwrap();

    assert_eq!(summary.stopped_reason, StopReason::Budget);
    assert_eq!(agent.state().phase, AgentPhase::Recovering);
    assert!(agent.context().entries().iter().any(|entry| {
        entry.source == ContextSource::PlanReminder && entry.content.contains("Budget exceeded")
    }));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn repeated_actions_inject_recovery_directive() {
    let root = std::env::temp_dir().join(format!("peridot-core-stuck-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry).unwrap();
    let mut agent = HarnessAgent::new(
        AgentState::new(ExecutionMode::Goal, PermissionMode::Auto),
        ContextManager::new(),
        registry,
    );
    let repeated =
        json!({"action":"plan_update","parameters":{"update":"still trying"}}).to_string();
    let provider = StaticProvider::new(vec![
        repeated.clone(),
        repeated.clone(),
        repeated,
        json!({"action":"agent_done","parameters":{"summary":"recovered"}}).to_string(),
    ]);

    let summary = agent
        .run_until_done(
            &provider,
            AgentRunRequest {
                task: "avoid loops".to_string(),
                model: "mock".to_string(),
                goal_checker_model: None,
                max_turns: 4,
                max_tokens: 512,
                reasoning_effort: peridot_common::ReasoningEffort::Off,
                budget_usd: 5.0,
                budget_warning_pct: 50,
                project_root: root.clone(),
                denied_paths: Vec::new(),
                hooks: HooksConfig::default(),
                security: SecurityConfig::default(),
            },
        )
        .await
        .unwrap();

    assert_eq!(summary.stopped_reason, StopReason::Done);
    assert!(agent.context().entries().iter().any(|entry| {
        entry
            .content
            .contains("Recovery directive: the last action repeated 3 times")
    }));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn run_until_done_recovers_after_tool_error() {
    let root = std::env::temp_dir().join(format!("peridot-core-recover-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry).unwrap();
    let mut agent = HarnessAgent::new(
        AgentState::new(ExecutionMode::Goal, PermissionMode::Auto),
        ContextManager::new(),
        registry,
    );
    let provider = StaticProvider::new(vec![
        json!({"action":"file_read","parameters":{"path":"missing.txt"}}).to_string(),
        json!({"action":"agent_done","parameters":{"summary":"recovered"}}).to_string(),
    ]);

    let summary = agent
        .run_until_done(
            &provider,
            AgentRunRequest {
                task: "recover from missing file".to_string(),
                model: "mock".to_string(),
                goal_checker_model: None,
                max_turns: 2,
                max_tokens: 512,
                reasoning_effort: peridot_common::ReasoningEffort::Off,
                budget_usd: 5.0,
                budget_warning_pct: 50,
                project_root: root.clone(),
                denied_paths: Vec::new(),
                hooks: HooksConfig::default(),
                security: SecurityConfig::default(),
            },
        )
        .await
        .unwrap();

    assert_eq!(summary.stopped_reason, StopReason::Done);
    assert_eq!(summary.turns.len(), 1);
    assert!(agent.context().entries().iter().any(|entry| {
        entry
            .content
            .contains("previous turn failed with not_found")
    }));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn repeated_parse_failures_inject_format_reminder() {
    let root = std::env::temp_dir().join(format!(
        "peridot-core-parse-recovery-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry).unwrap();
    let mut agent = HarnessAgent::new(
        AgentState::new(ExecutionMode::Goal, PermissionMode::Auto),
        ContextManager::new(),
        registry,
    );
    // Three consecutive parse errors should inject the format reminder via the
    // recovery layer; the fourth response then succeeds with a real tool call so the
    // loop can stop. Mirrors the production trigger where the model emits something
    // the provider's response parser rejects.
    let provider = StaticProvider::with_initial_parse_errors(
        vec![json!({"action":"agent_done","parameters":{"summary":"recovered"}}).to_string()],
        3,
    );

    let summary = agent
        .run_until_done(
            &provider,
            AgentRunRequest {
                task: "recover parse".to_string(),
                model: "mock".to_string(),
                goal_checker_model: None,
                max_turns: 8,
                max_tokens: 512,
                reasoning_effort: peridot_common::ReasoningEffort::Off,
                budget_usd: 5.0,
                budget_warning_pct: 50,
                project_root: root.clone(),
                denied_paths: Vec::new(),
                hooks: HooksConfig::default(),
                security: SecurityConfig::default(),
            },
        )
        .await
        .unwrap();

    assert_eq!(summary.stopped_reason, StopReason::Done);
    assert!(agent.context().entries().iter().any(|entry| {
        entry.source == ContextSource::PlanReminder && entry.content.contains("Format reminder")
    }));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn run_until_done_emits_error_and_recovery_hooks() {
    let root = std::env::temp_dir().join(format!(
        "peridot-core-recovery-hooks-{}",
        std::process::id()
    ));
    let hooks_dir = root.join(".peridot/hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    let script = hooks_dir.join("event.sh");
    std::fs::write(&script, "#!/bin/sh\necho \"$1:$2\" >> events.log\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry).unwrap();
    let mut agent = HarnessAgent::new(
        AgentState::new(ExecutionMode::Goal, PermissionMode::Auto),
        ContextManager::new(),
        registry,
    );
    let provider = StaticProvider::new(vec![
        json!({"action":"file_read","parameters":{"path":"missing.txt"}}).to_string(),
        json!({"action":"agent_done","parameters":{"summary":"recovered"}}).to_string(),
    ]);

    agent
        .run_until_done(
            &provider,
            AgentRunRequest {
                task: "recover from missing file".to_string(),
                model: "mock".to_string(),
                goal_checker_model: None,
                max_turns: 2,
                max_tokens: 512,
                reasoning_effort: peridot_common::ReasoningEffort::Off,
                budget_usd: 5.0,
                budget_warning_pct: 50,
                project_root: root.clone(),
                denied_paths: Vec::new(),
                hooks: HooksConfig {
                    event: vec![
                        HookConfig {
                            event: "error".to_string(),
                            run: ".peridot/hooks/event.sh error {error_type}".to_string(),
                            description: None,
                            on_failure: HookFailureMode::Block,
                            only_paths: Vec::new(),
                        },
                        HookConfig {
                            event: "recovery_triggered".to_string(),
                            run: ".peridot/hooks/event.sh recovery {recovery_type}".to_string(),
                            description: None,
                            on_failure: HookFailureMode::Block,
                            only_paths: Vec::new(),
                        },
                    ],
                    ..HooksConfig::default()
                },
                security: SecurityConfig::default(),
            },
        )
        .await
        .unwrap();

    let log = std::fs::read_to_string(root.join("events.log")).unwrap();
    assert!(log.contains("error:not_found"));
    assert!(log.contains("recovery:error"));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn goal_checker_can_reject_premature_done() {
    let root = std::env::temp_dir().join(format!("peridot-core-goal-check-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry).unwrap();
    let mut agent = HarnessAgent::new(
        AgentState::new(ExecutionMode::Goal, PermissionMode::Auto),
        ContextManager::new(),
        registry,
    );
    let provider = StaticProvider::new(vec![
        json!({"action":"agent_done","parameters":{"summary":"done"}}).to_string(),
        json!({"satisfied":false,"reason":"tests not run"}).to_string(),
        json!({"action":"plan_update","parameters":{"update":"ran tests"}}).to_string(),
        json!({"action":"agent_done","parameters":{"summary":"verified"}}).to_string(),
        json!({"satisfied":true,"reason":"objective verified"}).to_string(),
    ]);

    let summary = agent
        .run_until_done(
            &provider,
            AgentRunRequest {
                task: "finish only when verified".to_string(),
                model: "mock-main".to_string(),
                goal_checker_model: Some("mock-checker".to_string()),
                max_turns: 5,
                max_tokens: 512,
                reasoning_effort: peridot_common::ReasoningEffort::Off,
                budget_usd: 5.0,
                budget_warning_pct: 50,
                project_root: root.clone(),
                denied_paths: Vec::new(),
                hooks: HooksConfig::default(),
                security: SecurityConfig::default(),
            },
        )
        .await
        .unwrap();

    assert_eq!(summary.stopped_reason, StopReason::Done);
    assert_eq!(summary.turns.len(), 3);
    assert!(agent.context().entries().iter().any(|entry| {
        entry
            .content
            .contains("Goal checker says the objective is not satisfied")
    }));
    std::fs::remove_dir_all(root).unwrap();
}
