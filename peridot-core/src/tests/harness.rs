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
                reasoning_effort: peridot_common::ReasoningEffort::Off,
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
            .any(|event| matches!(event, AgentRunEvent::AssistantStarted { .. }))
    );
    assert!(events.iter().any(|event| {
        matches!(
            event,
            AgentRunEvent::ToolStarted { name, .. } if name == "agent_done"
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

/// A plain-text reply with no tool calls (e.g. "hi -> 안녕하세요!") must finish
/// the turn through a synthesized `agent_done` WITHOUT surfacing
/// `ToolStarted` / `ToolFinished` events — the assistant text is already
/// rendered by `AssistantFinished`, so re-emitting it under a tool prefix
/// produces a duplicated green echo in the transcript. The audit log still
/// records the synthetic call internally; only the user-visible event stream
/// is silenced.
#[tokio::test]
async fn text_only_completion_does_not_emit_synthetic_tool_events() {
    let root = std::env::temp_dir().join(format!(
        "peridot-core-text-only-{}",
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
    let provider = StaticProvider::new(vec!["안녕하세요! 무엇을 도와드릴까요?".to_string()]);
    let mut events = Vec::new();

    let summary = agent
        .run_until_done_with_events(
            &provider,
            AgentRunRequest {
                task: "hi".to_string(),
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
            |event| events.push(event),
        )
        .await
        .unwrap();

    assert_eq!(summary.stopped_reason, StopReason::Done);
    assert!(matches!(events[0], AgentRunEvent::RunStarted { .. }));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentRunEvent::AssistantFinished { .. })),
        "expected AssistantFinished to render the chat reply"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentRunEvent::ToolStarted { .. })),
        "synthetic agent_done from plain text must not emit ToolStarted: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentRunEvent::ToolFinished { .. })),
        "synthetic agent_done from plain text must not emit ToolFinished: {events:?}"
    );
    assert!(matches!(
        events.last().unwrap(),
        AgentRunEvent::Finished { .. }
    ));
    std::fs::remove_dir_all(root).unwrap();
}

/// When the model both streams a reply AND explicitly calls `agent_done`, the
/// summary parameter usually duplicates the assistant text the user already
/// read. To keep the transcript clean we suppress the `ToolStarted` /
/// `ToolFinished` events in that case; the assistant text already covers the
/// answer. The tool still runs (audit + phase transition), so we only assert
/// the UI events stay quiet.
#[tokio::test]
async fn text_plus_agent_done_suppresses_redundant_tool_events() {
    let root = std::env::temp_dir().join(format!(
        "peridot-core-done-dedup-{}",
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
    // The StaticProvider helper splits the JSON `action` envelope into a tool
    // call, leaving `text` empty. Bypass it by stashing a custom provider that
    // returns both `text` and a tool_calls entry for `agent_done`.
    let provider = super::support::StaticProvider::new_text_with_tool_call(
        "여기 본문 답변이 있어.".to_string(),
        "agent_done".to_string(),
        json!({"summary": "여기 본문 답변이 있어."}),
    );
    let mut events = Vec::new();
    let summary = agent
        .run_until_done_with_events(
            &provider,
            AgentRunRequest {
                task: "explain".to_string(),
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
            |event| events.push(event),
        )
        .await
        .unwrap();
    assert_eq!(summary.stopped_reason, StopReason::Done);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentRunEvent::ToolStarted { name, .. } if name == "agent_done")),
        "text + agent_done should suppress ToolStarted: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentRunEvent::ToolFinished { name, .. } if name == "agent_done")),
        "text + agent_done should suppress ToolFinished: {events:?}"
    );
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
                reasoning_effort: peridot_common::ReasoningEffort::Off,
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
                reasoning_effort: peridot_common::ReasoningEffort::Off,
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
                reasoning_effort: peridot_common::ReasoningEffort::Off,
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
                reasoning_effort: peridot_common::ReasoningEffort::Off,
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

#[tokio::test]
async fn auto_verify_after_mutation_appends_plan_reminder() {
    // file_write succeeds → harness auto-runs verify_build → the
    // result lands as a PlanReminder so the next turn sees it.
    let root = std::env::temp_dir().join(format!(
        "peridot-core-auto-verify-{}",
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
    agent.set_auto_verify_after_mutation(true);
    let provider = StaticProvider::new(vec![
        json!({
            "action": "file_write",
            "parameters": {"path": "note.txt", "content": "hello"}
        })
        .to_string(),
        json!({"action": "agent_done", "parameters": {"summary": "done"}}).to_string(),
    ]);
    let summary = agent
        .run_until_done_with_events(
            &provider,
            AgentRunRequest {
                task: "write a note".to_string(),
                model: "mock".to_string(),
                goal_checker_model: None,
                max_turns: 3,
                max_tokens: 512,
                reasoning_effort: peridot_common::ReasoningEffort::Off,
                budget_usd: 5.0,
                budget_warning_pct: 50,
                project_root: root.clone(),
                denied_paths: Vec::new(),
                hooks: HooksConfig::default(),
                security: SecurityConfig::default(),
            },
            |_| {},
        )
        .await
        .unwrap();
    assert_eq!(summary.stopped_reason, StopReason::Done);
    let has_auto_verify_note = agent
        .context()
        .entries()
        .iter()
        .any(|entry| entry.content.contains("[auto-verify]"));
    assert!(
        has_auto_verify_note,
        "expected an [auto-verify] PlanReminder after the file_write mutation"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn auto_grade_failure_keeps_loop_running() {
    // Grader rejects the first agent_done → recommendations injected
    // into context, loop continues, second agent_done passes.
    let root = std::env::temp_dir().join(format!(
        "peridot-core-auto-grade-{}",
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
    agent.set_auto_grade_on_done(true);
    // Provider responses, in order:
    //   1. agent_done (first attempt — grader rejects)
    //   2. grader verdict: passed=false + recommendations
    //   3. agent_done (second attempt — grader passes)
    //   4. grader verdict: passed=true
    let provider = super::support::StaticProvider::new(vec![
        json!({"action": "agent_done", "parameters": {"summary": "first try"}}).to_string(),
        r#"{"passed": false, "summary": "tests missing", "recommendations": ["add a unit test"]}"#
            .to_string(),
        json!({"action": "agent_done", "parameters": {"summary": "second try"}}).to_string(),
        r#"{"passed": true, "summary": "looks good", "recommendations": []}"#.to_string(),
    ]);
    let summary = agent
        .run_until_done_with_events(
            &provider,
            AgentRunRequest {
                task: "finish a small task".to_string(),
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
            |_| {},
        )
        .await
        .unwrap();
    assert_eq!(summary.stopped_reason, StopReason::Done);
    assert!(
        summary.turns.len() >= 2,
        "expected loop to continue past first agent_done — got {} turns",
        summary.turns.len()
    );
    let has_rejection_note = agent
        .context()
        .entries()
        .iter()
        .any(|entry| entry.content.contains("Grader rejected"));
    assert!(
        has_rejection_note,
        "expected an [auto-grade] rejection PlanReminder"
    );
    std::fs::remove_dir_all(root).unwrap();
}
