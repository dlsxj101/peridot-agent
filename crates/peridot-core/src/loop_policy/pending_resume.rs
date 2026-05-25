//! Pending-tool resume policy.
//!
//! When the harness halted on `approval_required` and the operator
//! later relaxed the security posture, a sidecar file records the
//! pending tool call. This policy runs at `turn_index == 0` of the
//! resumed run, re-executes the recorded tool against the (presumably
//! more permissive) security config, and folds the result back into
//! context so the model picks up exactly where it stopped.
//!
//! Replaces the inline `HarnessAgent::try_resume_pending_tool` helper.
//! Idempotent across runs — `take_pending_resume` clears the sidecar
//! before returning the call.

use peridot_common::PeriResult;
use peridot_context::{ContextEntry, ContextSource};

use crate::agent::{
    append_pending_resume_observation, emit_plan_updated_after_tool, risk_class_label_for,
    take_pending_resume,
};
use crate::loop_policy::{Decision, LoopPolicy, PolicyCx};
use crate::requests::AgentRunEvent;

/// Resumes the pending-approval tool call recorded in the sidecar file
/// at the start of the run. After the first turn, the sidecar is empty
/// and this policy is a no-op.
#[derive(Default)]
pub struct PendingResumePolicy {
    /// Set after the first `pre_turn` invocation so subsequent turns
    /// skip the sidecar lookup entirely.
    fired: bool,
}

impl PendingResumePolicy {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl LoopPolicy for PendingResumePolicy {
    fn name(&self) -> &'static str {
        "pending_resume"
    }

    async fn pre_turn(&mut self, cx: &mut PolicyCx<'_>) -> PeriResult<Decision> {
        if self.fired || cx.turn_index != 0 {
            return Ok(Decision::Continue);
        }
        self.fired = true;

        // Use the explicit sidecar path from PolicyCx when provided
        // (the harness installs it via `set_pending_resume_path`).
        // Otherwise fall through to the helper's default.
        let sidecar_owned;
        let sidecar_arg = if let Some(p) = cx.pending_resume_path {
            sidecar_owned = p.to_path_buf();
            Some(&sidecar_owned)
        } else {
            None
        };
        let Some(call) = take_pending_resume(sidecar_arg) else {
            return Ok(Decision::Continue);
        };

        let pending_name = call.name.clone();
        let pending_params = call.parameters.clone();
        // Get the risk_class label from a registry lookup. Without
        // direct access to the harness's tool registry from here we
        // emit None — the chip is purely informational.
        (cx.events)(AgentRunEvent::ToolStarted {
            name: pending_name.clone(),
            parameters: pending_params,
            risk_class: None,
        });

        let exec = cx
            .tool_dispatcher
            .execute(
                call,
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
            Ok((result, file_diff)) => {
                if let Some(diff) = file_diff {
                    (cx.events)(AgentRunEvent::FileDiff(diff));
                }
                (cx.events)(AgentRunEvent::ToolFinished {
                    name: pending_name.clone(),
                    result: result.clone(),
                });
                emit_plan_updated_after_tool(&pending_name, &result, cx.project_root, cx.events);
                append_pending_resume_observation(cx.context, &pending_name, &result)?;
            }
            Err(err) => {
                let failure_result = peridot_common::ToolResult::failure(format!(
                    "resume failed after approval: {err}"
                ));
                append_pending_resume_observation(cx.context, &pending_name, &failure_result)?;
                cx.context.append(ContextEntry::trusted(
                    ContextSource::PlanReminder,
                    format!(
                        "[resume] Tried to resume {pending_name} after approval but failed: {err}. Try a different approach."
                    ),
                ));
            }
        }
        // Suppress for the unused `risk_class_label_for` warning when
        // policy is built without the harness registry. The actual
        // label population is a follow-up that needs registry access
        // through the dispatcher trait.
        let _ = risk_class_label_for;

        Ok(Decision::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_policy::test_support::NoopProvider;
    use crate::loop_policy::{LoopPolicy, PolicyCx, ToolDispatcher, dispatch_pre_turn};
    use crate::requests::AgentRunRequest;
    use crate::state::AgentState;
    use peridot_common::{
        AgentPhase, ExecutionMode, HooksConfig, PeriResult, PermissionMode, SecurityConfig,
        ToolCall, ToolResult,
    };
    use peridot_context::ContextManager;
    use peridot_llm::Usage;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// Tool dispatcher that records every execute() call and returns a
    /// canned result. Lets PendingResume tests assert "was the recorded
    /// tool re-invoked?" without spinning up a real ToolRegistry.
    struct RecordingDispatcher {
        calls: Mutex<Vec<ToolCall>>,
        result: ToolResult,
    }

    impl RecordingDispatcher {
        fn ok_with(summary: &str) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                result: ToolResult::success(summary, serde_json::Value::Null),
            }
        }
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
        ) -> PeriResult<(ToolResult, Option<crate::requests::FileDiffPayload>)> {
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

    /// Writes a `pending_resume.json` sidecar at `path` containing the
    /// supplied tool call. Returns the path so the test can clean up.
    fn write_sidecar(dir: &std::path::Path, name: &str, params: serde_json::Value) -> PathBuf {
        let path = dir.join("pending_resume.json");
        let payload = serde_json::json!({
            "name": name,
            "parameters": params,
        });
        std::fs::write(&path, payload.to_string()).unwrap();
        path
    }

    fn tmpdir(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "peridot-pending-resume-{name}-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[tokio::test]
    async fn no_sidecar_means_no_op() {
        let root = tmpdir("none");
        let dispatcher = RecordingDispatcher::ok_with("ignored");
        let mut state = AgentState::new(ExecutionMode::Execute, PermissionMode::Auto);
        let mut context = ContextManager::new();
        let request = make_request();
        let mut usage = Usage::default();
        let hooks = HooksConfig::default();
        let security = SecurityConfig::default();
        let mut events = |_e: crate::requests::AgentRunEvent| {};
        let mut policies: Vec<Box<dyn LoopPolicy>> = vec![Box::new(PendingResumePolicy::new())];
        let sidecar_path = root.join("missing.json");
        let mut cx = PolicyCx {
            state: &mut state,
            context: &mut context,
            events: &mut events,
            usage: &mut usage,
            request: &request,
            turn_index: 0,
            project_root: &root,
            hooks: &hooks,
            security: &security,
            provider: &NoopProvider,
            tool_dispatcher: &dispatcher,
            subagent_runner: None,
            pending_resume_path: Some(&sidecar_path),
        };
        let decision = dispatch_pre_turn(&mut policies, &mut cx).await.unwrap();
        assert!(matches!(decision, Decision::Continue));
        assert!(
            dispatcher.calls.lock().unwrap().is_empty(),
            "no sidecar → dispatcher must never run"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn sidecar_present_replays_the_tool_on_first_turn() {
        let root = tmpdir("replay");
        let sidecar = write_sidecar(&root, "file_patch", serde_json::json!({"path": "x.txt"}));
        let dispatcher = RecordingDispatcher::ok_with("patched");
        let mut state = AgentState::new(ExecutionMode::Execute, PermissionMode::Auto);
        let mut context = ContextManager::new();
        let request = make_request();
        let mut usage = Usage::default();
        let hooks = HooksConfig::default();
        let security = SecurityConfig::default();
        let mut events = |_e: crate::requests::AgentRunEvent| {};
        let mut policies: Vec<Box<dyn LoopPolicy>> = vec![Box::new(PendingResumePolicy::new())];
        let mut cx = PolicyCx {
            state: &mut state,
            context: &mut context,
            events: &mut events,
            usage: &mut usage,
            request: &request,
            turn_index: 0,
            project_root: &root,
            hooks: &hooks,
            security: &security,
            provider: &NoopProvider,
            tool_dispatcher: &dispatcher,
            subagent_runner: None,
            pending_resume_path: Some(&sidecar),
        };
        let decision = dispatch_pre_turn(&mut policies, &mut cx).await.unwrap();
        assert!(matches!(decision, Decision::Continue));
        let calls = dispatcher.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "file_patch");
        // `take_pending_resume` deletes the sidecar on success so a
        // re-run in the same workspace wouldn't double-resume. Verify.
        assert!(
            !sidecar.exists(),
            "the helper should clear the sidecar after consuming it"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn does_not_fire_after_turn_zero() {
        let root = tmpdir("turn-gate");
        let sidecar = write_sidecar(&root, "file_patch", serde_json::json!({}));
        let dispatcher = RecordingDispatcher::ok_with("would-be");
        let mut state = AgentState::new(ExecutionMode::Execute, PermissionMode::Auto);
        let mut context = ContextManager::new();
        let request = make_request();
        let mut usage = Usage::default();
        let hooks = HooksConfig::default();
        let security = SecurityConfig::default();
        let mut events = |_e: crate::requests::AgentRunEvent| {};
        let mut policies: Vec<Box<dyn LoopPolicy>> = vec![Box::new(PendingResumePolicy::new())];
        let mut cx = PolicyCx {
            state: &mut state,
            context: &mut context,
            events: &mut events,
            usage: &mut usage,
            request: &request,
            turn_index: 5, // not the first turn
            project_root: &root,
            hooks: &hooks,
            security: &security,
            provider: &NoopProvider,
            tool_dispatcher: &dispatcher,
            subagent_runner: None,
            pending_resume_path: Some(&sidecar),
        };
        let decision = dispatch_pre_turn(&mut policies, &mut cx).await.unwrap();
        assert!(matches!(decision, Decision::Continue));
        assert!(dispatcher.calls.lock().unwrap().is_empty());
        // Sidecar should NOT have been consumed — the policy ignored
        // the file because we're past turn 0.
        assert!(
            sidecar.exists(),
            "turn_index gate must skip the sidecar read"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
