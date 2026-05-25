//! Auto-grade policy.
//!
//! On `agent_done`, asks an LLM grader whether the worktree diff is
//! actually shippable. If the grader rejects the result, this policy
//! folds the recommendations back into context as a plan reminder and
//! returns [`Decision::SkipTurn`] so the model gets another turn to
//! address them. If the grader passes — or the run is non-coding (empty
//! diff) or hasn't changed anything (diff == initial snapshot) — the
//! policy returns [`Decision::Continue`] and the loop falls through to
//! the normal Done path.
//!
//! Grader infrastructure failure (provider hiccup, unparseable response)
//! degrades silently to "agent_done stands" with a note in context —
//! blocking the operator on a flaky grader is worse than letting one
//! ungated run through.

use peridot_common::{AgentPhase, PeriResult};
use peridot_context::{ContextEntry, ContextSource};

use crate::agent::transition_phase;
use crate::agent_helpers::recent_verify_summary;
use crate::loop_policy::{Decision, LoopPolicy, PolicyCx};
use crate::requests::{AgentRunEvent, AgentTurnOutcome};
use crate::usage::accumulate_usage;

/// Diff provider used to obtain the current worktree diff at on-done time.
/// Boxed so the policy stays object-safe inside a `Vec<Box<dyn LoopPolicy>>`.
pub type DiffProvider = std::sync::Arc<dyn Fn() -> String + Send + Sync>;

/// LLM-grader gate on `agent_done`.
///
/// Stateful: holds the run-start diff snapshot so the "no work was done"
/// fast path can compare against it. The driver injects both the diff
/// provider closure and the initial diff via the constructor.
pub struct AutoGradePolicy {
    diff_provider: DiffProvider,
    initial_diff: Option<String>,
    enabled: bool,
}

impl AutoGradePolicy {
    /// `enabled` reflects `HarnessAgent::auto_grade_on_done`. When
    /// false the policy is a no-op (the driver still constructs it so
    /// `policies` is a single fixed list).
    pub fn new(enabled: bool, diff_provider: DiffProvider, initial_diff: Option<String>) -> Self {
        Self {
            diff_provider,
            initial_diff,
            enabled,
        }
    }
}

#[async_trait::async_trait]
impl LoopPolicy for AutoGradePolicy {
    fn name(&self) -> &'static str {
        "auto_grade"
    }

    async fn on_done(
        &mut self,
        cx: &mut PolicyCx<'_>,
        _outcomes: &[AgentTurnOutcome],
    ) -> PeriResult<Decision> {
        if !self.enabled {
            return Ok(Decision::Continue);
        }

        let diff = (self.diff_provider)();

        // Empty-diff fast path: chat / explanation / "do you remember
        // the last conversation?" turns finish without touching the
        // worktree. Feeding the grader an empty diff makes it reject
        // every non-coding turn with "No change was provided to address
        // the request", looping forever. Skip the grader.
        if diff.trim().is_empty() {
            cx.context.append(ContextEntry::trusted(
                ContextSource::PlanReminder,
                "[auto-grade] Skipped: no worktree changes to grade (chat or explanation turn)."
                    .to_string(),
            ));
            return Ok(Decision::Continue);
        }

        // Unchanged-from-start fast path: if the run produced a diff
        // identical to the one we captured at run start, the model
        // didn't actually do anything this turn. Same rationale.
        if self.initial_diff.as_deref() == Some(diff.as_str()) {
            cx.context.append(ContextEntry::trusted(
                ContextSource::PlanReminder,
                "[auto-grade] Skipped: worktree diff is unchanged since run start.".to_string(),
            ));
            return Ok(Decision::Continue);
        }

        let verify_summary = recent_verify_summary(cx.context).unwrap_or_default();
        match crate::grader::grade_work(
            cx.provider,
            &cx.request.model,
            &cx.request.task,
            &diff,
            &verify_summary,
        )
        .await
        {
            Ok(verdict) => {
                accumulate_usage(cx.usage, verdict.usage);
                if verdict.passed {
                    cx.context.append(ContextEntry::trusted(
                        ContextSource::PlanReminder,
                        format!("[auto-grade] Grader passed: {}", verdict.summary),
                    ));
                    return Ok(Decision::Continue);
                }
                transition_phase(
                    cx.state,
                    AgentPhase::Recovering,
                    "auto_grader_rejected",
                    cx.events,
                );
                let mut directive = format!(
                    "[auto-grade] Grader rejected agent_done: {}",
                    verdict.summary
                );
                if !verdict.recommendations.is_empty() {
                    directive.push_str("\nRecommendations:\n");
                    for rec in &verdict.recommendations {
                        directive.push_str("- ");
                        directive.push_str(rec);
                        directive.push('\n');
                    }
                }
                directive.push_str(
                    "\nAddress the recommendations and call agent_done again only when the change actually ships.",
                );
                cx.context.append(ContextEntry::trusted(
                    ContextSource::PlanReminder,
                    directive,
                ));
                (cx.events)(AgentRunEvent::Recovery {
                    message: format!("auto-grade failed: {}", verdict.summary),
                });
                Ok(Decision::SkipTurn)
            }
            Err(err) => {
                // Grader infrastructure failed. Don't block; surface a
                // note so the operator notices on review.
                cx.context.append(ContextEntry::trusted(
                    ContextSource::PlanReminder,
                    format!("[auto-grade] Grader unavailable: {err}"),
                ));
                Ok(Decision::Continue)
            }
        }
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
    use peridot_common::{ExecutionMode, HooksConfig, PermissionMode, SecurityConfig, ToolResult};
    use peridot_context::ContextManager;
    use peridot_llm::Usage;

    fn make_request() -> AgentRunRequest {
        AgentRunRequest {
            task: "ship the change".to_string(),
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

    fn done_outcomes() -> Vec<AgentTurnOutcome> {
        vec![AgentTurnOutcome {
            tool_name: "agent_done".to_string(),
            tool_result: ToolResult::success("done", serde_json::Value::Null),
            usage: Usage::default(),
            done: true,
        }]
    }

    /// Build the on_done dispatch context, run AutoGradePolicy
    /// configured by `(enabled, initial_diff)`, with a closure that
    /// produces `current_diff` when called. Returns the policy's
    /// Decision and the resulting context entries.
    async fn run(
        provider: &StaticProvider,
        enabled: bool,
        initial_diff: Option<String>,
        current_diff: String,
    ) -> (Decision, ContextManager) {
        let mut state = AgentState::new(ExecutionMode::Execute, PermissionMode::Auto);
        let mut context = ContextManager::new();
        let request = make_request();
        let mut usage = Usage::default();
        let hooks = HooksConfig::default();
        let security = SecurityConfig::default();
        let mut events = |_e: crate::requests::AgentRunEvent| {};
        let project_root = std::path::PathBuf::from(".");
        let diff_for_closure = current_diff.clone();
        let diff_provider: DiffProvider = std::sync::Arc::new(move || diff_for_closure.clone());
        let mut policies: Vec<Box<dyn LoopPolicy>> = vec![Box::new(AutoGradePolicy::new(
            enabled,
            diff_provider,
            initial_diff,
        ))];
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
    async fn no_op_when_disabled() {
        // No diff providers should run, no LLM calls.
        let provider = StaticProvider::new(vec![]);
        let (decision, context) = run(&provider, false, None, "+ added line".to_string()).await;
        assert!(matches!(decision, Decision::Continue));
        assert!(
            context.entries().is_empty(),
            "disabled grader must not mutate context"
        );
    }

    #[tokio::test]
    async fn skips_grader_on_empty_diff() {
        let provider = StaticProvider::new(vec![]);
        let (decision, context) = run(&provider, true, None, String::new()).await;
        assert!(matches!(decision, Decision::Continue));
        assert!(
            context
                .entries()
                .iter()
                .any(|e| e.content.contains("no worktree changes"))
        );
    }

    #[tokio::test]
    async fn skips_grader_on_unchanged_diff() {
        let provider = StaticProvider::new(vec![]);
        let baseline = "+ pre-existing change\n".to_string();
        let (decision, context) = run(&provider, true, Some(baseline.clone()), baseline).await;
        assert!(matches!(decision, Decision::Continue));
        assert!(
            context
                .entries()
                .iter()
                .any(|e| e.content.contains("unchanged since run start"))
        );
    }

    #[tokio::test]
    async fn grader_passed_lets_done_proceed() {
        let provider = StaticProvider::new(vec![
            r#"{"passed": true, "summary": "ship-ready", "recommendations": []}"#.to_string(),
        ]);
        let (decision, context) =
            run(&provider, true, None, "+ new functionality\n".to_string()).await;
        assert!(matches!(decision, Decision::Continue));
        assert!(
            context
                .entries()
                .iter()
                .any(|e| e.content.contains("Grader passed"))
        );
    }

    #[tokio::test]
    async fn grader_rejected_returns_skip_turn_with_recommendations() {
        let provider = StaticProvider::new(vec![
            r#"{"passed": false, "summary": "needs more tests", "recommendations": ["add a regression test", "update CHANGELOG"]}"#.to_string(),
        ]);
        let (decision, context) =
            run(&provider, true, None, "+ new functionality\n".to_string()).await;
        assert!(
            matches!(decision, Decision::SkipTurn),
            "rejected grader must veto done with SkipTurn"
        );
        let directive = context
            .entries()
            .iter()
            .find(|e| e.content.contains("Grader rejected"))
            .expect("expected rejection directive in context");
        assert!(directive.content.contains("add a regression test"));
        assert!(directive.content.contains("update CHANGELOG"));
    }
}
