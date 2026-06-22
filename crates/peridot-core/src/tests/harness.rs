use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use peridot_common::{
    ExecutionMode, HookConfig, HookFailureMode, HooksConfig, PeriError, PermissionMode,
    SecurityConfig, ToolCall,
};
use peridot_context::{ContextEntry, ContextManager, ContextSource};
use peridot_llm::ToolInvocation;
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
                service_tier: None,
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
                service_tier: None,
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

#[tokio::test]
async fn plan_tools_emit_plan_updated_event() {
    let root =
        std::env::temp_dir().join(format!("peridot-core-plan-updated-{}", std::process::id()));
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
            "action": "plan_create",
            "parameters": {
                "objective": "ship todo UI",
                "steps": ["wire event", "render panel"]
            }
        })
        .to_string(),
    ]);
    let mut events = Vec::new();

    let _summary = agent
        .run_until_done_with_events(
            &provider,
            AgentRunRequest {
                task: "create a plan".to_string(),
                model: "mock".to_string(),
                goal_checker_model: None,
                max_turns: 1,
                max_tokens: 512,
                reasoning_effort: peridot_common::ReasoningEffort::Off,
                service_tier: None,
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

    let plan_event = events
        .iter()
        .find_map(|event| match event {
            AgentRunEvent::PlanUpdated { steps, current } => Some((steps, current)),
            _ => None,
        })
        .expect("plan_create should emit PlanUpdated");
    assert_eq!(plan_event.0.len(), 2);
    assert_eq!(plan_event.0[0].label, "wire event");
    assert!(!plan_event.0[0].done);
    assert_eq!(*plan_event.1, Some(0));

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
    let root = std::env::temp_dir().join(format!("peridot-core-text-only-{}", std::process::id()));
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
                service_tier: None,
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
    let root = std::env::temp_dir().join(format!("peridot-core-done-dedup-{}", std::process::id()));
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
                service_tier: None,
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
        !events.iter().any(
            |event| matches!(event, AgentRunEvent::ToolStarted { name, .. } if name == "agent_done")
        ),
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
async fn parallel_tool_calls_record_only_executed_call_in_context() {
    let root = std::env::temp_dir().join(format!(
        "peridot-core-single-tool-history-{}",
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
    let provider = StaticProvider::new_custom_completion(
        String::new(),
        vec![
            ToolInvocation {
                id: "call_file_list".to_string(),
                name: "file_list".to_string(),
                arguments: json!({"path": "."}),
            },
            ToolInvocation {
                id: "call_ignored".to_string(),
                name: "agent_done".to_string(),
                arguments: json!({"summary": "ignored"}),
            },
        ],
    );

    let outcome = agent
        .run_turn(
            &provider,
            AgentTurnRequest {
                user_input: Some("hi".to_string()),
                model: "mock".to_string(),
                max_tokens: 512,
                reasoning_effort: peridot_common::ReasoningEffort::Off,
                service_tier: None,
                project_root: root.clone(),
                denied_paths: Vec::new(),
                hooks: HooksConfig::default(),
                security: SecurityConfig::default(),
            },
        )
        .await
        .unwrap();

    assert_eq!(outcome.tool_name, "file_list");
    let assistant_entry = agent
        .context()
        .entries()
        .iter()
        .find(|entry| entry.source == ContextSource::Assistant)
        .expect("assistant tool-call entry");
    assert_eq!(assistant_entry.tool_calls.len(), 1);
    assert_eq!(assistant_entry.tool_calls[0].id, "call_file_list");
    let tool_entry = agent
        .context()
        .entries()
        .iter()
        .find(|entry| entry.source == ContextSource::Tool)
        .expect("tool result entry");
    assert_eq!(tool_entry.tool_call_id.as_deref(), Some("call_file_list"));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn parallel_read_only_tool_calls_record_all_outputs_in_context() {
    let root = std::env::temp_dir().join(format!(
        "peridot-core-multi-tool-history-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), "alpha\n").unwrap();
    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry).unwrap();
    let mut agent = HarnessAgent::new(
        AgentState::new(ExecutionMode::Execute, PermissionMode::Auto),
        ContextManager::new(),
        registry,
    );
    let provider = StaticProvider::new_custom_completion(
        String::new(),
        vec![
            ToolInvocation {
                id: "call_file_list".to_string(),
                name: "file_list".to_string(),
                arguments: json!({"path": "."}),
            },
            ToolInvocation {
                id: "call_file_read".to_string(),
                name: "file_read".to_string(),
                arguments: json!({"path": "a.txt"}),
            },
        ],
    );

    let outcome = agent
        .run_turn(
            &provider,
            AgentTurnRequest {
                user_input: Some("inspect files".to_string()),
                model: "mock".to_string(),
                max_tokens: 512,
                reasoning_effort: peridot_common::ReasoningEffort::Off,
                service_tier: None,
                project_root: root.clone(),
                denied_paths: Vec::new(),
                hooks: HooksConfig::default(),
                security: SecurityConfig::default(),
            },
        )
        .await
        .unwrap();

    assert_eq!(outcome.tool_name, "multi_tool");
    assert!(outcome.tool_result.success);
    let assistant_entry = agent
        .context()
        .entries()
        .iter()
        .find(|entry| entry.source == ContextSource::Assistant)
        .expect("assistant tool-call entry");
    assert_eq!(assistant_entry.tool_calls.len(), 2);
    let tool_call_ids = agent
        .context()
        .entries()
        .iter()
        .filter(|entry| entry.source == ContextSource::Tool)
        .filter_map(|entry| entry.tool_call_id.as_deref())
        .collect::<Vec<_>>();
    assert_eq!(tool_call_ids, vec!["call_file_list", "call_file_read"]);
    std::fs::remove_dir_all(root).unwrap();
}

/// When a tool fails midway through a read-only batch, the assistant turn was
/// already appended with ALL tool_calls. The harness must answer EVERY
/// tool_call id — the failing one plus every not-yet-executed one — before
/// bubbling the error, or Responses-style providers (OpenAI Codex) reject the
/// next request with `400 No tool output found for function call <id>` and the
/// recovery loop retries on a permanently-broken conversation.
#[tokio::test]
async fn mid_batch_tool_failure_answers_every_tool_call_id() {
    let root = std::env::temp_dir().join(format!(
        "peridot-core-multi-tool-failure-{}",
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
    // First read-only call reads a missing file → execution returns `Err`,
    // hitting the batch error arm. The remaining read-only calls never run,
    // but their tool_call ids must still be answered.
    let provider = StaticProvider::new_custom_completion(
        String::new(),
        vec![
            ToolInvocation {
                id: "call_missing_read".to_string(),
                name: "file_read".to_string(),
                arguments: json!({"path": "does-not-exist.txt"}),
            },
            ToolInvocation {
                id: "call_skipped_list".to_string(),
                name: "file_list".to_string(),
                arguments: json!({"path": "."}),
            },
            ToolInvocation {
                id: "call_skipped_read".to_string(),
                name: "file_read".to_string(),
                arguments: json!({"path": "does-not-exist-2.txt"}),
            },
        ],
    );

    let result = agent
        .run_turn(
            &provider,
            AgentTurnRequest {
                user_input: Some("inspect files".to_string()),
                model: "mock".to_string(),
                max_tokens: 512,
                reasoning_effort: peridot_common::ReasoningEffort::Off,
                service_tier: None,
                project_root: root.clone(),
                denied_paths: Vec::new(),
                hooks: HooksConfig::default(),
                security: SecurityConfig::default(),
            },
        )
        .await;

    // The turn surfaces the underlying error.
    assert!(result.is_err(), "mid-batch failure should bubble an error");

    // Every tool_call id in the assistant turn must have a matching
    // tool_result entry in the context — none left dangling.
    let assistant_entry = agent
        .context()
        .entries()
        .iter()
        .find(|entry| entry.source == ContextSource::Assistant)
        .expect("assistant tool-call entry");
    let mut call_ids = assistant_entry
        .tool_calls
        .iter()
        .map(|call| call.id.clone())
        .collect::<Vec<_>>();
    call_ids.sort();

    let mut answered_ids = agent
        .context()
        .entries()
        .iter()
        .filter(|entry| entry.source == ContextSource::Tool)
        .filter_map(|entry| entry.tool_call_id.clone())
        .collect::<Vec<_>>();
    answered_ids.sort();

    assert_eq!(
        call_ids, answered_ids,
        "every assistant tool_call id must be paired with a tool_result"
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
                service_tier: None,
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
async fn safe_shell_exec_requires_approval_for_read_only_command() {
    let root = std::env::temp_dir().join(format!(
        "peridot-core-safe-shell-approval-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry).unwrap();
    let mut agent = HarnessAgent::new(
        AgentState::new(ExecutionMode::Execute, PermissionMode::Safe),
        ContextManager::new(),
        registry,
    );
    let provider = StaticProvider::new(vec![
        json!({
            "action": "shell_exec",
            "parameters": {"command": "printf ok"}
        })
        .to_string(),
    ]);
    let mut events = Vec::new();

    let summary = agent
        .run_until_done_with_events(
            &provider,
            AgentRunRequest {
                task: "inspect with shell".to_string(),
                model: "mock".to_string(),
                goal_checker_model: None,
                max_turns: 1,
                max_tokens: 512,
                reasoning_effort: peridot_common::ReasoningEffort::Off,
                service_tier: None,
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
            AgentRunEvent::ApprovalRequested { tool_name, reason, parameters, risk_class }
                if tool_name == "shell_exec"
                    && reason == "shell_exec requires explicit user approval"
                    && parameters.get("command").and_then(serde_json::Value::as_str)
                        == Some("printf ok")
                    && risk_class.as_deref() == Some("destructive")
        )
    }));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn approved_exact_tool_call_skips_safe_confirmation_gate() {
    let root = std::env::temp_dir().join(format!(
        "peridot-core-approved-tool-call-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry).unwrap();
    let agent = HarnessAgent::new(
        AgentState::new(ExecutionMode::Execute, PermissionMode::Safe),
        ContextManager::new(),
        registry,
    );
    let parameters = json!({"command": "printf ok"});
    let mut security = SecurityConfig::default();
    security.approved_tool_calls.push(format!(
        "shell_exec:{}",
        serde_json::to_string(&parameters).unwrap()
    ));

    let (result, _) = agent
        .execute_tool_call_with_runtime(
            ToolCall {
                name: "shell_exec".to_string(),
                parameters,
            },
            root.clone(),
            Vec::new(),
            HooksConfig::default(),
            security,
        )
        .await
        .unwrap();

    assert!(result.success);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn hard_blocked_permission_denied_pairs_tool_output_without_approval_event() {
    let root = std::env::temp_dir().join(format!(
        "peridot-core-hard-blocked-tool-output-{}",
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
            "parameters": {"command": "rm -rf /"}
        })
        .to_string(),
    ]);
    let mut events = Vec::new();

    let summary = agent
        .run_until_done_with_events(
            &provider,
            AgentRunRequest {
                task: "delete everything".to_string(),
                model: "mock".to_string(),
                goal_checker_model: None,
                max_turns: 1,
                max_tokens: 512,
                reasoning_effort: peridot_common::ReasoningEffort::Off,
                service_tier: None,
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

    assert_ne!(summary.stopped_reason, StopReason::ApprovalRequired);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentRunEvent::ApprovalRequested { .. })),
        "hard-blocked shell commands must not enter the approval-resume path"
    );
    let tool_entry = agent
        .context()
        .entries()
        .iter()
        .find(|entry| entry.source == ContextSource::Tool)
        .expect("hard-blocked tool failure should still be paired with a tool output");
    assert_eq!(tool_entry.tool_call_id.as_deref(), Some("call_0"));
    assert!(
        tool_entry
            .content
            .contains("hard-blocked shell command pattern")
    );
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
                service_tier: None,
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
                service_tier: None,
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
        llm_compaction_threshold_tokens: 1,
        ..peridot_context::ContextLimits::default()
    });
    // Compaction now preserves the last COMPACTION_KEEP_TAIL=6 entries,
    // so seed more than that or there's nothing to fold.
    for index in 0..12 {
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
        json!({
            "key_facts": ["prior context compacted"],
            "current_plan": "finish",
            "recent_decisions": []
        })
        .to_string(),
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
                service_tier: None,
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
    let root =
        std::env::temp_dir().join(format!("peridot-core-auto-verify-{}", std::process::id()));
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
                service_tier: None,
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
    let root = std::env::temp_dir().join(format!("peridot-core-auto-grade-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry).unwrap();
    let mut agent = HarnessAgent::new(
        AgentState::new(ExecutionMode::Execute, PermissionMode::Auto),
        ContextManager::new(),
        registry,
    );
    agent.set_auto_grade_on_done(true);
    // Inject a non-empty diff so the empty-diff fast path (added to
    // unblock chat / Q&A turns from looping forever) does NOT trigger
    // here. We want the grader path proper to run so we can verify
    // the rejection-then-pass loop.
    let diff_call_count = Arc::new(AtomicUsize::new(0));
    let diff_call_count_for_provider = Arc::clone(&diff_call_count);
    agent.set_grader_diff_provider(move |_| {
        if diff_call_count_for_provider.fetch_add(1, Ordering::SeqCst) == 0 {
            String::new()
        } else {
            "+ pretend change\n".to_string()
        }
    });
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
                service_tier: None,
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

#[tokio::test]
async fn auto_grade_skips_dirty_baseline_when_run_makes_no_new_diff() {
    let root = std::env::temp_dir().join(format!(
        "peridot-core-auto-grade-dirty-baseline-{}",
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
    agent.set_grader_diff_provider(|_| "+ pre-existing unrelated change\n".to_string());
    let provider = super::support::StaticProvider::new(vec![
        json!({"action": "agent_done", "parameters": {"summary": "done"}}).to_string(),
    ]);

    let summary = agent
        .run_until_done_with_events(
            &provider,
            AgentRunRequest {
                task: "run a no-code approval check".to_string(),
                model: "mock".to_string(),
                goal_checker_model: None,
                max_turns: 4,
                max_tokens: 512,
                reasoning_effort: peridot_common::ReasoningEffort::Off,
                service_tier: None,
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
    assert_eq!(summary.turns.len(), 1);
    assert!(agent.context().entries().iter().any(|entry| {
        entry
            .content
            .contains("[auto-grade] Skipped: worktree diff is unchanged")
    }));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn pending_resume_sidecar_replays_pending_tool_call_on_next_run() {
    // Approval-required → sidecar write → next session loads + replays
    // the pending tool call without asking the model.
    let root = std::env::temp_dir().join(format!(
        "peridot-core-pending-resume-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let sidecar = root.join("pending_resume.bin");
    // Write a fake pending resume payload manually (skipping the
    // approval-denied path). The harness should consume it on
    // run_until_done().
    let pending = ToolCall {
        name: "file_write".to_string(),
        parameters: json!({
            "path": "resumed.txt",
            "content": "from sidecar\n"
        }),
    };
    std::fs::write(&sidecar, serde_json::to_vec(&pending).unwrap()).unwrap();

    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry).unwrap();
    let mut agent = HarnessAgent::new(
        AgentState::new(ExecutionMode::Execute, PermissionMode::Auto),
        ContextManager::new(),
        registry,
    );
    agent.set_pending_resume_path(sidecar.clone());
    // Provider only needs to drive the loop to agent_done after the
    // pending-resume runs first.
    let provider = StaticProvider::new(vec![
        json!({"action": "agent_done", "parameters": {"summary": "resumed"}}).to_string(),
    ]);

    let summary = agent
        .run_until_done(
            &provider,
            AgentRunRequest {
                task: "resume sidecar".to_string(),
                model: "mock".to_string(),
                goal_checker_model: None,
                max_turns: 2,
                max_tokens: 256,
                reasoning_effort: peridot_common::ReasoningEffort::Off,
                service_tier: None,
                budget_usd: 1.0,
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
    // The sidecar must have been replayed: file exists with the
    // expected content.
    let written = std::fs::read_to_string(root.join("resumed.txt")).expect("resumed.txt created");
    assert_eq!(written, "from sidecar\n");
    // The sidecar itself should be consumed (deleted).
    assert!(
        !sidecar.exists(),
        "pending_resume.bin must be removed after consumption"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn agents_md_hot_reload_injects_updated_content_into_context() {
    // Edit AGENTS.md mid-run and verify the next refresh_agents_md tick
    // injects the new content as a PlanReminder.
    let root = std::env::temp_dir().join(format!(
        "peridot-core-agents-hot-reload-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let agents_path = root.join("AGENTS.md");
    std::fs::write(&agents_path, "## rule1\nbe terse\n").unwrap();

    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry).unwrap();
    let mut agent = HarnessAgent::new(
        AgentState::new(ExecutionMode::Execute, PermissionMode::Auto),
        ContextManager::new(),
        registry,
    );
    agent.set_agents_md_path(agents_path.clone());

    let provider = StaticProvider::new(vec![
        json!({"action": "agent_done", "parameters": {"summary": "first"}}).to_string(),
    ]);
    let _summary1 = agent
        .run_until_done(
            &provider,
            AgentRunRequest {
                task: "first turn".to_string(),
                model: "mock".to_string(),
                goal_checker_model: None,
                max_turns: 1,
                max_tokens: 64,
                reasoning_effort: peridot_common::ReasoningEffort::Off,
                service_tier: None,
                budget_usd: 1.0,
                budget_warning_pct: 50,
                project_root: root.clone(),
                denied_paths: Vec::new(),
                hooks: HooksConfig::default(),
                security: SecurityConfig::default(),
            },
        )
        .await
        .unwrap();
    // Confirm the initial AGENTS.md content reached context.
    assert!(
        agent
            .context()
            .entries()
            .iter()
            .any(|e| e.content.contains("be terse")),
        "expected initial AGENTS.md content in context"
    );

    // Now edit the file (newer mtime, different len).
    std::thread::sleep(std::time::Duration::from_millis(1_100));
    std::fs::write(&agents_path, "## rule1\nbe terse AND polite\nALWAYS\n").unwrap();

    let provider2 = StaticProvider::new(vec![
        json!({"action": "agent_done", "parameters": {"summary": "second"}}).to_string(),
    ]);
    let _summary2 = agent
        .run_until_done(
            &provider2,
            AgentRunRequest {
                task: "second turn".to_string(),
                model: "mock".to_string(),
                goal_checker_model: None,
                max_turns: 1,
                max_tokens: 64,
                reasoning_effort: peridot_common::ReasoningEffort::Off,
                service_tier: None,
                budget_usd: 1.0,
                budget_warning_pct: 50,
                project_root: root.clone(),
                denied_paths: Vec::new(),
                hooks: HooksConfig::default(),
                security: SecurityConfig::default(),
            },
        )
        .await
        .unwrap();

    assert!(
        agent
            .context()
            .entries()
            .iter()
            .any(|e| e.content.contains("ALWAYS")),
        "expected reloaded AGENTS.md (containing ALWAYS) to land in context"
    );
    std::fs::remove_dir_all(root).unwrap();
}
