//! Goal-checker policy.
//!
//! In Goal mode, when the model declares `agent_done`, an independent LLM
//! pass asks whether the objective is actually satisfied. If not, this
//! policy appends a plan reminder and returns [`Decision::SkipTurn`] so
//! the loop continues with the new feedback instead of terminating.
//!
//! Only fires when `state.mode == ExecutionMode::Goal` AND
//! `request.goal_checker_model` is set. Otherwise it's a no-op.

use peridot_common::{AgentPhase, ExecutionMode, PeriResult};
use peridot_context::{ContextEntry, ContextSource};

use crate::agent::transition_phase;
use crate::goal::check_goal_satisfied;
use crate::loop_policy::{Decision, LoopPolicy, PolicyCx};
use crate::recovery::run_recovery_event_hook;
use crate::requests::AgentTurnOutcome;
use crate::usage::accumulate_usage;

/// Gates `agent_done` in Goal mode behind an independent LLM verdict.
#[derive(Default)]
pub struct GoalCheckerPolicy;

impl GoalCheckerPolicy {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl LoopPolicy for GoalCheckerPolicy {
    fn name(&self) -> &'static str {
        "goal_checker"
    }

    async fn on_done(
        &mut self,
        cx: &mut PolicyCx<'_>,
        outcomes: &[AgentTurnOutcome],
    ) -> PeriResult<Decision> {
        // Bail unless this is a Goal run with a configured checker model.
        let Some(checker_model) = cx.request.goal_checker_model.as_deref() else {
            return Ok(Decision::Continue);
        };
        if cx.state.mode != ExecutionMode::Goal {
            return Ok(Decision::Continue);
        }

        let verdict =
            check_goal_satisfied(cx.provider, checker_model, &cx.request.task, outcomes).await?;
        accumulate_usage(cx.usage, verdict.usage);
        cx.context.append(ContextEntry::trusted(
            ContextSource::PlanReminder,
            format!("Goal checker verdict: {}", verdict.reason),
        ));

        if verdict.satisfied {
            return Ok(Decision::Continue);
        }

        transition_phase(
            cx.state,
            AgentPhase::Recovering,
            "goal_checker_rejected",
            cx.events,
        );
        run_recovery_event_hook(cx.project_root, cx.hooks, "goal_checker", &verdict.reason)?;
        cx.context.append(ContextEntry::trusted(
            ContextSource::PlanReminder,
            "Goal checker says the objective is not satisfied yet. Continue with a concrete next action.".to_string(),
        ));
        Ok(Decision::SkipTurn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_policy::test_support::NoopToolDispatcher;
    use crate::loop_policy::{LoopPolicy, PolicyCx, dispatch_on_done};
    use crate::requests::AgentRunRequest;
    use crate::state::AgentState;
    use crate::tests::support::StaticProvider;
    use peridot_common::{HooksConfig, PermissionMode, SecurityConfig, ToolResult};
    use peridot_context::ContextManager;
    use peridot_llm::Usage;

    fn make_request(checker_model: Option<&str>) -> AgentRunRequest {
        AgentRunRequest {
            task: "ship the release".to_string(),
            model: "mock".to_string(),
            goal_checker_model: checker_model.map(String::from),
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

    fn done_outcomes() -> Vec<AgentTurnOutcome> {
        vec![AgentTurnOutcome {
            tool_name: "agent_done".to_string(),
            tool_result: ToolResult::success("done", serde_json::Value::Null),
            usage: Usage::default(),
            done: true,
        }]
    }

    async fn run_on_done(
        provider: &StaticProvider,
        mode: ExecutionMode,
        request: AgentRunRequest,
    ) -> (Decision, ContextManager) {
        let mut state = AgentState::new(mode, PermissionMode::Auto);
        if mode == ExecutionMode::Goal {
            state = state.with_goal("ship the release".to_string());
        }
        let mut context = ContextManager::new();
        let mut usage = Usage::default();
        let hooks = HooksConfig::default();
        let security = SecurityConfig::default();
        let mut events = |_e: crate::requests::AgentRunEvent| {};
        let project_root = std::path::PathBuf::from(".");
        let mut policies: Vec<Box<dyn LoopPolicy>> = vec![Box::new(GoalCheckerPolicy::new())];
        let outcomes = done_outcomes();
        let mut cx = PolicyCx {
            state: &mut state,
            context: &mut context,
            events: &mut events,
            usage: &mut usage,
            request: &request,
            turn_index: 0,
            project_root: &project_root,
            hooks: &hooks,
            security: &security,
            provider,
            tool_dispatcher: &NoopToolDispatcher,
            subagent_runner: None,
            pending_resume_path: None,
        };
        let decision = dispatch_on_done(&mut policies, &mut cx, &outcomes)
            .await
            .unwrap();
        (decision, context)
    }

    #[tokio::test]
    async fn no_op_outside_goal_mode() {
        // ExecutionMode::Execute → checker doesn't run even if a
        // checker_model is configured.
        let provider = StaticProvider::new(vec![]);
        let request = make_request(Some("goal-checker-model"));
        let (decision, context) = run_on_done(&provider, ExecutionMode::Execute, request).await;
        assert!(matches!(decision, Decision::Continue));
        // No context mutation since the policy bailed before calling LLM.
        assert!(context.entries().is_empty());
    }

    #[tokio::test]
    async fn no_op_when_no_checker_model_configured() {
        let provider = StaticProvider::new(vec![]);
        let request = make_request(None);
        let (decision, _context) = run_on_done(&provider, ExecutionMode::Goal, request).await;
        assert!(matches!(decision, Decision::Continue));
    }

    #[tokio::test]
    async fn satisfied_verdict_lets_done_proceed() {
        let provider = StaticProvider::new(vec![
            r#"{"satisfied": true, "reason": "all looks good"}"#.to_string(),
        ]);
        let request = make_request(Some("goal-checker-model"));
        let (decision, context) = run_on_done(&provider, ExecutionMode::Goal, request).await;
        assert!(matches!(decision, Decision::Continue));
        // The verdict reason is appended to context regardless.
        assert!(
            context
                .entries()
                .iter()
                .any(|e| e.content.contains("Goal checker verdict")),
            "expected the verdict text to be appended"
        );
    }

    #[tokio::test]
    async fn unsatisfied_verdict_returns_skip_turn() {
        let provider = StaticProvider::new(vec![
            r#"{"satisfied": false, "reason": "tests still failing"}"#.to_string(),
        ]);
        let request = make_request(Some("goal-checker-model"));
        let (decision, context) = run_on_done(&provider, ExecutionMode::Goal, request).await;
        assert!(
            matches!(decision, Decision::SkipTurn),
            "expected SkipTurn when verdict.satisfied is false, got {decision:?}"
        );
        // Both the verdict-reason and the "not satisfied yet" follow-up
        // reminder should be appended.
        let entries = context.entries();
        assert!(
            entries
                .iter()
                .any(|e| e.content.contains("tests still failing"))
        );
        assert!(
            entries
                .iter()
                .any(|e| e.content.contains("not satisfied yet"))
        );
    }
}
