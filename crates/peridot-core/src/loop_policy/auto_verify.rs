//! Auto-verify-after-mutation policy.
//!
//! When `auto_verify_after_mutation` is enabled on the harness and a
//! mutating tool (file_write / file_patch) just succeeded, this policy
//! runs `verify_build` to surface a broken compile while the diff is
//! still fresh in the user's mind. The result is appended to context
//! as a `[auto-verify]` PlanReminder; PreflightPolicy's
//! context-aware check then accepts the marker as satisfying the
//! "verify before done" gate.
//!
//! Replaces the inline `HarnessAgent::run_auto_verify_after_mutation`
//! helper. Failures in the verify infrastructure (e.g. no build
//! command configured) are surfaced as a quiet note in context and
//! never abort the run.

use peridot_common::PeriResult;
use peridot_context::{ContextEntry, ContextSource};

use crate::agent_helpers::is_mutating_tool_name;
use crate::loop_policy::{Decision, LoopPolicy, PolicyCx};
use crate::requests::AgentTurnOutcome;

/// Runs `verify_build` after every successful mutating-tool outcome.
/// Gated by `cx.request` — specifically, the `auto_verify_after_mutation`
/// flag the harness reflects into the request's behavioural knobs.
///
/// The policy currently reads the flag through a field on the policy
/// itself rather than `cx.request` (which doesn't yet carry the flag).
/// The harness sets it at construction.
pub struct AutoVerifyAfterMutationPolicy {
    enabled: bool,
}

impl AutoVerifyAfterMutationPolicy {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }
}

#[async_trait::async_trait]
impl LoopPolicy for AutoVerifyAfterMutationPolicy {
    fn name(&self) -> &'static str {
        "auto_verify_after_mutation"
    }

    async fn post_turn(
        &mut self,
        cx: &mut PolicyCx<'_>,
        outcome: &AgentTurnOutcome,
    ) -> PeriResult<Decision> {
        if !(self.enabled
            && outcome.tool_result.success
            && is_mutating_tool_name(&outcome.tool_name))
        {
            return Ok(Decision::Continue);
        }
        let exec = cx
            .tool_dispatcher
            .execute(
                peridot_common::ToolCall {
                    name: "verify_build".to_string(),
                    parameters: serde_json::json!({}),
                },
                cx.state.mode,
                cx.state.phase,
                cx.state.permission,
                cx.project_root.to_path_buf(),
                cx.request.denied_paths.clone(),
                cx.hooks.clone(),
                cx.security.clone(),
            )
            .await;
        match exec {
            Ok((result, _file_diff)) => {
                let note = if result.success {
                    format!("[auto-verify] verify_build passed: {}", result.summary)
                } else {
                    format!(
                        "[auto-verify] verify_build FAILED: {}\nFix this before declaring agent_done.",
                        result.summary
                    )
                };
                cx.context
                    .append(ContextEntry::trusted(ContextSource::PlanReminder, note));
            }
            Err(err) => {
                cx.context.append(ContextEntry::trusted(
                    ContextSource::PlanReminder,
                    format!("[auto-verify] verify_build could not run: {err}"),
                ));
            }
        }
        Ok(Decision::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_policy::test_support::NoopProvider;
    use crate::loop_policy::{LoopPolicy, PolicyCx, ToolDispatcher, dispatch_post_turn};
    use crate::requests::AgentRunRequest;
    use crate::state::AgentState;
    use peridot_common::{
        AgentPhase, ExecutionMode, HooksConfig, PermissionMode, SecurityConfig, ToolCall,
    };
    use peridot_context::{ContextManager, ContextSource};
    use peridot_llm::Usage;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// Mock dispatcher that returns a fixed verify_build result and
    /// records every call. Used to assert what AutoVerify dispatches
    /// without spinning up a real ToolRegistry.
    struct RecordingDispatcher {
        calls: Mutex<Vec<ToolCall>>,
        result: peridot_common::ToolResult,
    }

    #[async_trait::async_trait]
    impl ToolDispatcher for RecordingDispatcher {
        async fn execute(
            &self,
            call: ToolCall,
            _mode: ExecutionMode,
            _phase: AgentPhase,
            _permission: PermissionMode,
            _project_root: PathBuf,
            _denied_paths: Vec<PathBuf>,
            _hooks: HooksConfig,
            _security: SecurityConfig,
        ) -> peridot_common::PeriResult<(
            peridot_common::ToolResult,
            Option<crate::requests::FileDiffPayload>,
        )> {
            self.calls.lock().unwrap().push(call);
            Ok((self.result.clone(), None))
        }
    }

    fn make_request() -> AgentRunRequest {
        AgentRunRequest {
            task: String::new(),
            model: "mock".to_string(),
            goal_checker_model: None,
            max_turns: 1,
            max_tokens: 256,
            reasoning_effort: peridot_common::ReasoningEffort::Off,
            service_tier: None,
            budget_usd: 0.0,
            budget_warning_pct: 80,
            project_root: std::path::PathBuf::from("."),
            denied_paths: Vec::new(),
            hooks: HooksConfig::default(),
            security: SecurityConfig::default(),
        }
    }

    fn make_outcome(tool_name: &str, success: bool) -> AgentTurnOutcome {
        AgentTurnOutcome {
            tool_name: tool_name.to_string(),
            tool_result: if success {
                peridot_common::ToolResult::success(tool_name, serde_json::Value::Null)
            } else {
                peridot_common::ToolResult::failure(tool_name)
            },
            usage: Usage::default(),
            done: false,
        }
    }

    async fn run_post_turn(
        policy: AutoVerifyAfterMutationPolicy,
        outcome: &AgentTurnOutcome,
        dispatcher: &RecordingDispatcher,
        context: &mut ContextManager,
    ) -> Decision {
        let mut state = AgentState::new(ExecutionMode::Execute, PermissionMode::Auto);
        let request = make_request();
        let mut usage = Usage::default();
        let hooks = HooksConfig::default();
        let security = SecurityConfig::default();
        let mut events = |_e: crate::requests::AgentRunEvent| {};
        let project_root = std::path::PathBuf::from(".");
        let mut policies: Vec<Box<dyn LoopPolicy>> = vec![Box::new(policy)];
        let mut cx = PolicyCx {
            state: &mut state,
            context,
            events: &mut events,
            usage: &mut usage,
            request: &request,
            turn_index: 0,
            project_root: &project_root,
            hooks: &hooks,
            security: &security,
            provider: &NoopProvider,
            tool_dispatcher: dispatcher,
            subagent_runner: None,
            pending_resume_path: None,
        };
        dispatch_post_turn(&mut policies, &mut cx, outcome)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn fires_after_successful_file_patch_and_appends_passed_marker() {
        let dispatcher = RecordingDispatcher {
            calls: Mutex::new(Vec::new()),
            result: peridot_common::ToolResult::success("build OK", serde_json::Value::Null),
        };
        let mut context = ContextManager::new();
        let outcome = make_outcome("file_patch", true);
        let decision = run_post_turn(
            AutoVerifyAfterMutationPolicy::new(true),
            &outcome,
            &dispatcher,
            &mut context,
        )
        .await;
        assert!(matches!(decision, Decision::Continue));
        let calls = dispatcher.calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "expected exactly one verify_build dispatch");
        assert_eq!(calls[0].name, "verify_build");
        // PreflightPolicy reads the `[auto-verify] verify_build passed`
        // marker — ensure it lands.
        let marker = context.entries().iter().any(|e| {
            e.source == ContextSource::PlanReminder
                && e.content.contains("[auto-verify]")
                && e.content.contains("passed")
        });
        assert!(marker, "expected the passed-marker reminder in context");
    }

    #[tokio::test]
    async fn appends_failed_marker_when_verify_build_returns_failure() {
        let dispatcher = RecordingDispatcher {
            calls: Mutex::new(Vec::new()),
            result: peridot_common::ToolResult::failure("compile error in src/lib.rs"),
        };
        let mut context = ContextManager::new();
        let outcome = make_outcome("file_write", true);
        let decision = run_post_turn(
            AutoVerifyAfterMutationPolicy::new(true),
            &outcome,
            &dispatcher,
            &mut context,
        )
        .await;
        assert!(matches!(decision, Decision::Continue));
        // FAILED marker must NOT contain "passed" — PreflightPolicy
        // matches the substring "passed" specifically.
        let failed = context.entries().iter().any(|e| {
            e.source == ContextSource::PlanReminder
                && e.content.contains("[auto-verify]")
                && e.content.contains("FAILED")
        });
        assert!(failed, "expected the FAILED marker reminder in context");
        assert!(
            !context
                .entries()
                .iter()
                .any(|e| e.content.contains("passed")),
            "FAILED outcome must not produce a 'passed' string in context"
        );
    }

    #[tokio::test]
    async fn skips_non_mutating_tool() {
        let dispatcher = RecordingDispatcher {
            calls: Mutex::new(Vec::new()),
            result: peridot_common::ToolResult::success("never run", serde_json::Value::Null),
        };
        let mut context = ContextManager::new();
        let outcome = make_outcome("file_read", true);
        run_post_turn(
            AutoVerifyAfterMutationPolicy::new(true),
            &outcome,
            &dispatcher,
            &mut context,
        )
        .await;
        assert!(
            dispatcher.calls.lock().unwrap().is_empty(),
            "AutoVerify must only fire on mutating tools (file_write/file_patch)"
        );
    }

    #[tokio::test]
    async fn skips_when_disabled() {
        let dispatcher = RecordingDispatcher {
            calls: Mutex::new(Vec::new()),
            result: peridot_common::ToolResult::success("never run", serde_json::Value::Null),
        };
        let mut context = ContextManager::new();
        let outcome = make_outcome("file_patch", true);
        run_post_turn(
            AutoVerifyAfterMutationPolicy::new(false), // disabled
            &outcome,
            &dispatcher,
            &mut context,
        )
        .await;
        assert!(dispatcher.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn skips_when_mutating_tool_failed() {
        // A failed mutation didn't actually mutate anything — no point
        // verifying. AutoVerify must gate on `tool_result.success`.
        let dispatcher = RecordingDispatcher {
            calls: Mutex::new(Vec::new()),
            result: peridot_common::ToolResult::success("never run", serde_json::Value::Null),
        };
        let mut context = ContextManager::new();
        let outcome = make_outcome("file_patch", false);
        run_post_turn(
            AutoVerifyAfterMutationPolicy::new(true),
            &outcome,
            &dispatcher,
            &mut context,
        )
        .await;
        assert!(dispatcher.calls.lock().unwrap().is_empty());
    }
}
