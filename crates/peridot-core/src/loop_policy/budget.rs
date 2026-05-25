//! Budget-warning policy.
//!
//! Watches aggregated run usage and emits the one-shot budget-warning
//! hook + plan reminder when usage crosses `budget_warning_pct` of the
//! configured `budget_usd`. The matching budget *cap* (which actually
//! stops the run) lives separately because it runs at a different
//! point in the loop and needs to produce a final summary; that policy
//! will be extracted alongside the auto-grade gate in a later PR.

use peridot_common::{ExecutionMode, PeriResult};
use peridot_context::{ContextEntry, ContextSource};

use crate::loop_policy::{Decision, LoopPolicy, PolicyCx};
use crate::recovery::{
    budget_warning_message, run_budget_warning_hook, should_emit_budget_warning,
};
use crate::requests::AgentTurnOutcome;

/// Emits a one-shot warning when run usage crosses the budget threshold.
///
/// Threshold is configured by `request.budget_warning_pct` (an integer
/// percentage of `request.budget_usd`). The warning fires at most once
/// per run; subsequent turns past the threshold are silent so the
/// transcript doesn't get spammed.
#[derive(Default)]
pub struct BudgetWarningPolicy {
    /// True after the warning fired this run. The check is one-shot.
    warning_sent: bool,
}

impl BudgetWarningPolicy {
    /// Convenience constructor with the default (un-fired) state.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl LoopPolicy for BudgetWarningPolicy {
    fn name(&self) -> &'static str {
        "budget_warning"
    }

    async fn post_turn(
        &mut self,
        cx: &mut PolicyCx<'_>,
        _outcome: &AgentTurnOutcome,
    ) -> PeriResult<Decision> {
        if !should_emit_budget_warning(
            cx.request.budget_usd,
            cx.request.budget_warning_pct,
            cx.usage.estimated_cost_usd,
            self.warning_sent,
        ) {
            return Ok(Decision::Continue);
        }
        run_budget_warning_hook(
            cx.project_root,
            cx.hooks,
            cx.usage.estimated_cost_usd,
            cx.request.budget_usd,
        )?;
        self.warning_sent = true;
        // In Goal mode, the model needs to know it's approaching the cap
        // so it can wrap up. In other modes the warning is informational
        // only — the model wouldn't act on it anyway.
        if cx.state.mode == ExecutionMode::Goal {
            cx.context.append(ContextEntry::trusted(
                ContextSource::PlanReminder,
                budget_warning_message(cx.usage.estimated_cost_usd, cx.request.budget_usd),
            ));
        }
        Ok(Decision::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_policy::dispatch_post_turn;
    use crate::requests::{AgentRunEvent, AgentRunRequest, AgentTurnOutcome};
    use crate::state::AgentState;
    use peridot_common::{HooksConfig, PermissionMode, SecurityConfig};
    use peridot_context::ContextManager;
    use peridot_llm::Usage;

    fn make_request(budget_usd: f64) -> AgentRunRequest {
        AgentRunRequest {
            task: String::new(),
            model: "mock".to_string(),
            goal_checker_model: None,
            max_turns: 1,
            max_tokens: 256,
            reasoning_effort: peridot_common::ReasoningEffort::Off,
            service_tier: None,
            budget_usd,
            budget_warning_pct: 80,
            project_root: std::path::PathBuf::from("."),
            denied_paths: Vec::new(),
            hooks: HooksConfig::default(),
            security: SecurityConfig::default(),
        }
    }

    fn make_outcome() -> AgentTurnOutcome {
        AgentTurnOutcome {
            tool_name: "noop".to_string(),
            tool_result: peridot_common::ToolResult::success("ok", serde_json::Value::Null),
            usage: Usage::default(),
            done: false,
        }
    }

    #[tokio::test]
    async fn fires_once_when_threshold_crossed_in_goal_mode() {
        let mut state = AgentState::new(ExecutionMode::Goal, PermissionMode::Safe);
        let mut context = ContextManager::new();
        let request = make_request(1.0);
        let mut usage = Usage {
            estimated_cost_usd: 0.85, // 85% of 1.0 — over the 80% threshold
            ..Default::default()
        };
        let hooks = HooksConfig::default();
        let security = SecurityConfig::default();
        let outcome = make_outcome();
        let mut events_buf = Vec::<AgentRunEvent>::new();
        let mut events = |e: AgentRunEvent| events_buf.push(e);
        let project_root = std::path::PathBuf::from(".");
        let mut policies: Vec<Box<dyn LoopPolicy>> = vec![Box::new(BudgetWarningPolicy::new())];

        let initial_entries = context.entries().len();
        {
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
                provider: &crate::loop_policy::test_support::NoopProvider,
                tool_dispatcher: &crate::loop_policy::test_support::NoopToolDispatcher,
                subagent_runner: None,
                pending_resume_path: None,
            };
            let d = dispatch_post_turn(&mut policies, &mut cx, &outcome)
                .await
                .unwrap();
            assert!(matches!(d, Decision::Continue));
        }
        // Goal mode → plan-reminder entry must have been appended.
        assert_eq!(
            context.entries().len(),
            initial_entries + 1,
            "expected one new plan-reminder entry after threshold crossed"
        );

        // Second call past the threshold should NOT emit the warning again.
        let entries_after_first = context.entries().len();
        {
            let mut cx = PolicyCx {
                state: &mut state,
                context: &mut context,
                events: &mut events,
                usage: &mut usage,
                request: &request,
                turn_index: 1,
                project_root: &project_root,
                hooks: &hooks,
                security: &security,
                provider: &crate::loop_policy::test_support::NoopProvider,
                tool_dispatcher: &crate::loop_policy::test_support::NoopToolDispatcher,
                subagent_runner: None,
                pending_resume_path: None,
            };
            let d = dispatch_post_turn(&mut policies, &mut cx, &outcome)
                .await
                .unwrap();
            assert!(matches!(d, Decision::Continue));
        }
        assert_eq!(
            context.entries().len(),
            entries_after_first,
            "warning must fire at most once per run"
        );
    }

    #[tokio::test]
    async fn does_not_fire_below_threshold() {
        let mut state = AgentState::new(ExecutionMode::Goal, PermissionMode::Safe);
        let mut context = ContextManager::new();
        let request = make_request(1.0);
        let mut usage = Usage {
            estimated_cost_usd: 0.5, // 50% — below 80%
            ..Default::default()
        };
        let hooks = HooksConfig::default();
        let security = SecurityConfig::default();
        let outcome = make_outcome();
        let mut events_buf = Vec::<AgentRunEvent>::new();
        let mut events = |e: AgentRunEvent| events_buf.push(e);
        let project_root = std::path::PathBuf::from(".");
        let mut policies: Vec<Box<dyn LoopPolicy>> = vec![Box::new(BudgetWarningPolicy::new())];

        let initial_entries = context.entries().len();
        {
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
                provider: &crate::loop_policy::test_support::NoopProvider,
                tool_dispatcher: &crate::loop_policy::test_support::NoopToolDispatcher,
                subagent_runner: None,
                pending_resume_path: None,
            };
            dispatch_post_turn(&mut policies, &mut cx, &outcome)
                .await
                .unwrap();
        }
        assert_eq!(context.entries().len(), initial_entries);
    }
}
