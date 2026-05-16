use std::os::unix::fs::PermissionsExt;

use peridot_common::{
    ExecutionMode, HookConfig, HookFailureMode, HooksConfig, PeriError, PermissionMode,
    SecurityConfig, ToolCall,
};
use peridot_context::{ContextEntry, ContextManager, ContextSource};
use peridot_tools::{ToolRegistry, register_builtin_tools};
use serde_json::json;

use crate::{
    AgentRunEvent, AgentRunRequest, AgentState, AgentTurnRequest, HarnessAgent, StopReason,
};

use super::support::{StaticProvider, StreamingOnlyProvider};

#[tokio::test]
async fn run_until_done_executes_tools_and_stops() {
    let root = std::env::temp_dir().join(format!("peridot-core-loop-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry).unwrap();
    let mut agent = HarnessAgent::new(
        AgentState::new(ExecutionMode::Execute, PermissionMode::Auto),
        ContextManager::new(),
        registry,
    );
    let provider = StaticProvider::new(vec![
        json!({
            "action": "file_write",
            "parameters": {"path": "loop.txt", "content": "ok\n"}
        })
        .to_string(),
        json!({
            "action": "agent_done",
            "parameters": {"summary": "finished"}
        })
        .to_string(),
    ]);

    let summary = agent
        .run_until_done(
            &provider,
            AgentRunRequest {
                task: "write loop.txt".to_string(),
                model: "mock".to_string(),
                goal_checker_model: None,
                max_turns: 4,
                max_tokens: 512,
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
    assert_eq!(summary.turns.len(), 2);
    assert_eq!(
        std::fs::read_to_string(root.join("loop.txt")).unwrap(),
        "ok\n"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn run_until_done_emits_ui_events() {
    let root =
        std::env::temp_dir().join(format!("peridot-core-loop-events-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry).unwrap();
    let mut agent = HarnessAgent::new(
        AgentState::new(ExecutionMode::Execute, PermissionMode::Auto),
        ContextManager::new(),
        registry,
    );
    let provider = StaticProvider::new(vec![
        json!({
            "thinking": "ready to finish",
            "action": "agent_done",
            "parameters": {"summary": "finished"}
        })
        .to_string(),
    ]);
    let mut events = Vec::new();

    let summary = agent
        .run_until_done_with_events(
            &provider,
            AgentRunRequest {
                task: "finish".to_string(),
                model: "mock".to_string(),
                goal_checker_model: None,
                max_turns: 2,
                max_tokens: 512,
                budget_usd: 5.0,
                budget_warning_pct: 50,
                project_root: root.clone(),
                denied_paths: Vec::new(),
                hooks: HooksConfig::default(),
                security: SecurityConfig::default(),
            },
            |event| events.push(event),
        )
        .await
        .unwrap();

    assert_eq!(summary.stopped_reason, StopReason::Done);
    assert!(matches!(events[0], AgentRunEvent::RunStarted { .. }));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentRunEvent::AssistantDelta { .. }))
    );
    assert!(events.iter().any(|event| {
        matches!(
            event,
            AgentRunEvent::Thinking { text } if text == "ready to finish"
        )
    }));
    assert!(events.iter().any(|event| {
        matches!(
            event,
            AgentRunEvent::ToolFinished { name, result }
                if name == "agent_done" && result.success
        )
    }));
    assert!(matches!(
        events.last().unwrap(),
        AgentRunEvent::Finished { .. }
    ));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn permission_denied_tool_emits_approval_event() {
    let root = std::env::temp_dir().join(format!(
        "peridot-core-approval-event-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry).unwrap();
    let mut agent = HarnessAgent::new(
        AgentState::new(ExecutionMode::Execute, PermissionMode::Auto),
        ContextManager::new(),
        registry,
    );
    let provider = StaticProvider::new(vec![
        json!({
            "action": "shell_exec",
            "parameters": {"command": "npm install left-pad"}
        })
        .to_string(),
    ]);
    let mut events = Vec::new();

    let summary = agent
        .run_until_done_with_events(
            &provider,
            AgentRunRequest {
                task: "install dependency".to_string(),
                model: "mock".to_string(),
                goal_checker_model: None,
                max_turns: 1,
                max_tokens: 512,
                budget_usd: 5.0,
                budget_warning_pct: 50,
                project_root: root.clone(),
                denied_paths: Vec::new(),
                hooks: HooksConfig::default(),
                security: SecurityConfig::default(),
            },
            |event| events.push(event),
        )
        .await
        .unwrap();

    assert_eq!(summary.stopped_reason, StopReason::ApprovalRequired);
    assert!(events.iter().any(|event| {
        matches!(
            event,
            AgentRunEvent::ApprovalRequested { tool_name, reason, .. }
                if tool_name == "shell_exec" && reason.contains("dependency installation")
        )
    }));
    assert!(matches!(
        events.last().unwrap(),
        AgentRunEvent::Finished { summary }
            if summary.stopped_reason == StopReason::ApprovalRequired
    ));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn run_turn_injects_plan_reminder() {
    let root = std::env::temp_dir().join(format!(
        "peridot-core-turn-plan-reminder-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("todo.md"), "# Plan\n\n1. [ ] Keep context\n").unwrap();
    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry).unwrap();
    let mut agent = HarnessAgent::new(
        AgentState::new(ExecutionMode::Execute, PermissionMode::Auto),
        ContextManager::new(),
        registry,
    );
    let provider = StaticProvider::new(vec![
        json!({"action":"agent_done","parameters":{"summary":"done"}}).to_string(),
    ]);

    agent
        .run_turn(
            &provider,
            AgentTurnRequest {
                user_input: Some("finish".to_string()),
                model: "mock".to_string(),
                max_tokens: 512,
                project_root: root.clone(),
                denied_paths: Vec::new(),
                hooks: HooksConfig::default(),
                security: SecurityConfig::default(),
            },
        )
        .await
        .unwrap();

    assert!(agent.context().entries().iter().any(|entry| {
        entry.source == ContextSource::PlanReminder && entry.content.contains("Keep context")
    }));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn run_turn_uses_provider_stream_chunks() {
    let root =
        std::env::temp_dir().join(format!("peridot-core-stream-turn-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry).unwrap();
    let mut agent = HarnessAgent::new(
        AgentState::new(ExecutionMode::Execute, PermissionMode::Auto),
        ContextManager::new(),
        registry,
    );

    let outcome = agent
        .run_turn(
            &StreamingOnlyProvider,
            AgentTurnRequest {
                user_input: Some("finish".to_string()),
                model: "mock".to_string(),
                max_tokens: 512,
                project_root: root.clone(),
                denied_paths: Vec::new(),
                hooks: HooksConfig::default(),
                security: SecurityConfig::default(),
            },
        )
        .await
        .unwrap();

    assert_eq!(outcome.tool_name, "agent_done");
    assert!(outcome.done);
    assert_eq!(outcome.usage.output_tokens, 3);
    assert!(agent.context().entries().iter().any(|entry| {
        entry.source == ContextSource::Assistant && entry.content.contains("streamed")
    }));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn plan_mode_blocks_file_write() {
    let root = std::env::temp_dir().join(format!("peridot-core-plan-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry).unwrap();
    let agent = HarnessAgent::new(
        AgentState::new(ExecutionMode::Plan, PermissionMode::Auto),
        ContextManager::new(),
        registry,
    );

    let result = agent
        .execute_tool_call(
            ToolCall::new("file_write", json!({"path":"blocked.txt","content":"nope"})),
            &root,
        )
        .await;

    assert!(matches!(result, Err(PeriError::PermissionDenied(_))));
    assert!(!root.join("blocked.txt").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn plan_mode_blocks_subagent_delegation() {
    let root =
        std::env::temp_dir().join(format!("peridot-core-plan-delegate-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry).unwrap();
    let agent = HarnessAgent::new(
        AgentState::new(ExecutionMode::Plan, PermissionMode::Auto),
        ContextManager::new(),
        registry,
    );

    let result = agent
        .execute_tool_call(
            ToolCall::new(
                "agent_delegate",
                json!({"prompt":"write tests", "kind":"fork"}),
            ),
            &root,
        )
        .await;

    assert!(matches!(result, Err(PeriError::PermissionDenied(_))));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn tool_hooks_wrap_execution() {
    let root = std::env::temp_dir().join(format!("peridot-core-hooks-{}", std::process::id()));
    let hooks_dir = root.join(".peridot/hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    let script = hooks_dir.join("mark.sh");
    std::fs::write(&script, "#!/bin/sh\necho $1 >> hook.log\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry).unwrap();
    let agent = HarnessAgent::new(
        AgentState::new(ExecutionMode::Execute, PermissionMode::Auto),
        ContextManager::new(),
        registry,
    );

    agent
        .execute_tool_call_with_runtime(
            ToolCall::new("file_write", json!({"path":"hooked.txt","content":"ok"})),
            &root,
            Vec::new(),
            HooksConfig {
                tool: vec![HookConfig {
                    event: "pre:file_write".to_string(),
                    run: ".peridot/hooks/mark.sh {path}".to_string(),
                    description: None,
                    on_failure: HookFailureMode::Block,
                    only_paths: Vec::new(),
                }],
                ..HooksConfig::default()
            },
            SecurityConfig::default(),
        )
        .await
        .unwrap();

    assert_eq!(
        std::fs::read_to_string(root.join("hook.log")).unwrap(),
        "hooked.txt\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("hooked.txt")).unwrap(),
        "ok"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn run_turn_emits_context_compacted_hook() {
    let root =
        std::env::temp_dir().join(format!("peridot-core-compact-hook-{}", std::process::id()));
    let hooks_dir = root.join(".peridot/hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    let script = hooks_dir.join("compact.sh");
    std::fs::write(&script, "#!/bin/sh\necho \"$1:$2\" >> compact.log\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry).unwrap();
    let mut context = ContextManager::with_limits(peridot_context::ContextLimits {
        compaction_threshold_tokens: 1,
        ..peridot_context::ContextLimits::default()
    });
    for index in 0..5 {
        context.append(ContextEntry::trusted(
            ContextSource::User,
            format!("large prior message {index}"),
        ));
    }
    let mut agent = HarnessAgent::new(
        AgentState::new(ExecutionMode::Execute, PermissionMode::Auto),
        context,
        registry,
    );
    let provider = StaticProvider::new(vec![
        json!({"action":"agent_done","parameters":{"summary":"done"}}).to_string(),
    ]);

    agent
        .run_turn(
            &provider,
            AgentTurnRequest {
                user_input: Some("finish".to_string()),
                model: "mock".to_string(),
                max_tokens: 512,
                project_root: root.clone(),
                denied_paths: Vec::new(),
                hooks: HooksConfig {
                    event: vec![HookConfig {
                        event: "context_compacted".to_string(),
                        run: ".peridot/hooks/compact.sh {current} {limit}".to_string(),
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

    let log = std::fs::read_to_string(root.join("compact.log")).unwrap();
    assert!(log.contains(":1"));
    std::fs::remove_dir_all(root).unwrap();
}
