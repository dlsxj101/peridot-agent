//! Codebase-survey prefetch policy.
//!
//! In Goal mode with a long, vague task, spawning an internal "codebase
//! survey" sub-agent before the main turn gives the model structured
//! orientation context to work from. This policy replaces the inline
//! `HarnessAgent::try_prefetch_codebase_survey` helper.
//!
//! Gated by:
//!   - `should_prefetch_codebase_survey(mode, task)` — heuristic on
//!     task length and execution mode.
//!   - `cx.subagent_runner.is_some()` — runner must be configured.
//!   - First turn only (`turn_index == 0`).
//!
//! When any gate fails, the policy is a no-op.

use peridot_agents::{ModelTier, SubAgentKind, SubAgentTask};
use peridot_common::PeriResult;
use peridot_context::{ContextEntry, ContextSource};

use crate::agent::{
    build_subagent_review, codebase_survey_prompt, should_prefetch_codebase_survey,
};
use crate::loop_policy::{Decision, LoopPolicy, PolicyCx};
use crate::requests::AgentRunEvent;

/// Once-per-run codebase-survey kickoff. Fires at `turn_index == 0`
/// when the harness mode and task heuristics agree it's worthwhile.
#[derive(Default)]
pub struct CodebaseSurveyPrefetchPolicy {
    /// Latched after the first invocation. Even if the gate
    /// re-evaluates differently on later turns, the policy won't
    /// re-fire — survey is a one-shot prefetch by design.
    fired: bool,
}

impl CodebaseSurveyPrefetchPolicy {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl LoopPolicy for CodebaseSurveyPrefetchPolicy {
    fn name(&self) -> &'static str {
        "codebase_survey_prefetch"
    }

    async fn pre_turn(&mut self, cx: &mut PolicyCx<'_>) -> PeriResult<Decision> {
        if self.fired || cx.turn_index != 0 {
            return Ok(Decision::Continue);
        }
        self.fired = true;

        if !should_prefetch_codebase_survey(cx.state.mode, &cx.request.task) {
            return Ok(Decision::Continue);
        }
        let Some(runner) = cx.subagent_runner.as_ref().cloned() else {
            return Ok(Decision::Continue);
        };

        let prompt = codebase_survey_prompt(&cx.request.task, cx.context);
        (cx.events)(AgentRunEvent::ToolStarted {
            name: "agent_delegate".to_string(),
            parameters: serde_json::json!({
                "kind": "fork",
                "model_tier": "main",
                "prompt": "codebase survey prefetch",
            }),
            risk_class: None,
        });
        match runner
            .run(SubAgentTask {
                prompt,
                kind: SubAgentKind::Fork,
                model_tier: Some(ModelTier::Main),
            })
            .await
        {
            Ok(result) => {
                let output = serde_json::json!(result);
                let tool_result = peridot_common::ToolResult::success(
                    "codebase survey subagent finished",
                    output.clone(),
                );
                (cx.events)(AgentRunEvent::ToolFinished {
                    name: "agent_delegate".to_string(),
                    result: tool_result,
                });
                cx.context.append(ContextEntry::trusted(
                    ContextSource::PlanReminder,
                    format!(
                        "[codebase survey sub-agent]\n{}\n\nUse this as orientation only. Re-read exact files or evidence refs before making final claims.",
                        build_subagent_review(&output)
                    ),
                ));
            }
            Err(err) => {
                (cx.events)(AgentRunEvent::Recovery {
                    message: format!("codebase survey subagent failed: {err}"),
                });
                cx.context.append(ContextEntry::trusted(
                    ContextSource::PlanReminder,
                    format!(
                        "[codebase survey sub-agent] Failed before the main turn: {err}. Continue with direct file_search/file_read, and avoid broad full-repo reads."
                    ),
                ));
            }
        }
        Ok(Decision::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_policy::test_support::{NoopProvider, NoopToolDispatcher};
    use crate::loop_policy::{LoopPolicy, PolicyCx, dispatch_pre_turn};
    use crate::requests::AgentRunRequest;
    use crate::state::AgentState;
    use peridot_agents::{SubAgent, SubAgentKind, SubAgentResult, SubAgentTask};
    use peridot_common::{ExecutionMode, HooksConfig, PeriResult, PermissionMode, SecurityConfig};
    use peridot_context::ContextManager;
    use peridot_llm::Usage;
    use std::sync::{Arc, Mutex};

    /// Records sub-agent invocations so tests can assert the policy
    /// either fired or stayed quiet. The body is a constant success.
    struct RecordingRunner {
        calls: Mutex<Vec<SubAgentTask>>,
    }

    #[async_trait::async_trait]
    impl SubAgent for RecordingRunner {
        async fn run(&self, task: SubAgentTask) -> PeriResult<SubAgentResult> {
            self.calls.lock().unwrap().push(task);
            Ok(SubAgentResult {
                success: true,
                summary: "survey done".to_string(),
                kind: SubAgentKind::Fork,
                workspace: None,
                diff: String::new(),
                evidence_refs: Vec::new(),
            })
        }
    }

    fn long_goal_task() -> String {
        // `should_prefetch_codebase_survey` accepts tasks that hit a
        // "broad" keyword (codebase, architecture, etc.) AND meet a
        // length threshold. Include "codebase" so the keyword gate
        // matches in non-Plan mode.
        "Audit the codebase architecture, understand the codebase fully, and \
         refactor where appropriate so policies own their own state. "
            .repeat(2)
    }

    fn make_request(task: String) -> AgentRunRequest {
        AgentRunRequest {
            task,
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

    fn run_with_runner(
        runner: Option<Arc<dyn SubAgent>>,
        mode: ExecutionMode,
        task: String,
        turn_index: usize,
    ) -> (Decision, ContextManager) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut state = AgentState::new(mode, PermissionMode::Auto);
            let mut context = ContextManager::new();
            let request = make_request(task);
            let mut usage = Usage::default();
            let hooks = HooksConfig::default();
            let security = SecurityConfig::default();
            let mut events_buf = Vec::new();
            let mut events = |e: crate::requests::AgentRunEvent| events_buf.push(e);
            let project_root = std::path::PathBuf::from(".");
            let mut policies: Vec<Box<dyn LoopPolicy>> =
                vec![Box::new(CodebaseSurveyPrefetchPolicy::new())];
            let decision = {
                let mut cx = PolicyCx {
                    state: &mut state,
                    context: &mut context,
                    events: &mut events,
                    usage: &mut usage,
                    request: &request,
                    turn_index,
                    project_root: &project_root,
                    hooks: &hooks,
                    security: &security,
                    provider: &NoopProvider,
                    tool_dispatcher: &NoopToolDispatcher,
                    subagent_runner: runner,
                    pending_resume_path: None,
                };
                dispatch_pre_turn(&mut policies, &mut cx).await.unwrap()
            };
            (decision, context)
        })
    }

    #[test]
    fn fires_in_goal_mode_with_long_task_when_runner_present() {
        let runner = Arc::new(RecordingRunner {
            calls: Mutex::new(Vec::new()),
        });
        let (decision, context) = run_with_runner(
            Some(runner.clone() as Arc<dyn SubAgent>),
            ExecutionMode::Goal,
            long_goal_task(),
            0,
        );
        assert!(matches!(decision, Decision::Continue));
        assert_eq!(
            runner.calls.lock().unwrap().len(),
            1,
            "expected the survey policy to spawn the subagent exactly once"
        );
        // The success branch appends a `[codebase survey sub-agent]` reminder.
        let added = context
            .entries()
            .iter()
            .any(|e| e.content.contains("[codebase survey sub-agent]"));
        assert!(
            added,
            "expected the survey reminder to land in context after a successful subagent run"
        );
    }

    #[test]
    fn no_op_when_runner_is_missing() {
        let (decision, context) = run_with_runner(None, ExecutionMode::Goal, long_goal_task(), 0);
        assert!(matches!(decision, Decision::Continue));
        assert!(
            context.entries().is_empty(),
            "no runner → no context mutation"
        );
    }

    #[test]
    fn no_op_on_later_turns() {
        let runner = Arc::new(RecordingRunner {
            calls: Mutex::new(Vec::new()),
        });
        // turn_index > 0 must skip the prefetch even if everything else matches.
        let (decision, context) = run_with_runner(
            Some(runner.clone() as Arc<dyn SubAgent>),
            ExecutionMode::Goal,
            long_goal_task(),
            3,
        );
        assert!(matches!(decision, Decision::Continue));
        assert!(runner.calls.lock().unwrap().is_empty());
        assert!(context.entries().is_empty());
    }

    #[test]
    fn no_op_in_plan_mode() {
        // `should_prefetch_codebase_survey` rejects Plan mode outright
        // (planning is read-only orientation already). Execute mode
        // *does* accept the prefetch — it's gated on the broad-keyword
        // heuristic, not on Goal-vs-Execute.
        let runner = Arc::new(RecordingRunner {
            calls: Mutex::new(Vec::new()),
        });
        let (decision, context) = run_with_runner(
            Some(runner.clone() as Arc<dyn SubAgent>),
            ExecutionMode::Plan,
            long_goal_task(),
            0,
        );
        assert!(matches!(decision, Decision::Continue));
        assert!(runner.calls.lock().unwrap().is_empty());
        assert!(context.entries().is_empty());
    }
}
