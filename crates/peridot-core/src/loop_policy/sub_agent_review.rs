//! Sub-agent review policy.
//!
//! After every successful `agent_delegate` tool call the harness needs to
//! tell the parent model "go look at the diff this sub-agent actually
//! produced — do not rubber-stamp its summary." Otherwise it's trivial
//! for a sub-agent to claim success without producing meaningful changes,
//! and the parent will happily call `agent_done` on top of an empty diff.
//!
//! [`SubAgentReviewPolicy`] runs as a `post_turn` observer. If the outcome
//! is a successful `agent_delegate`, it formats the sub-agent's payload
//! into a `[sub-agent review]` directive and appends it to the parent's
//! context. No state, no fan-out — every other tool is a passthrough.

use peridot_common::PeriResult;
use peridot_context::{ContextEntry, ContextSource};

use crate::agent::build_subagent_review;
use crate::loop_policy::{Decision, LoopPolicy, PolicyCx};
use crate::requests::AgentTurnOutcome;

/// Appends a `[sub-agent review]` plan reminder after every successful
/// `agent_delegate` so the parent must inspect the workspace diff before
/// declaring done.
#[derive(Default)]
pub struct SubAgentReviewPolicy;

impl SubAgentReviewPolicy {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl LoopPolicy for SubAgentReviewPolicy {
    fn name(&self) -> &'static str {
        "sub_agent_review"
    }

    async fn post_turn(
        &mut self,
        cx: &mut PolicyCx<'_>,
        outcome: &AgentTurnOutcome,
    ) -> PeriResult<Decision> {
        if outcome.tool_name != "agent_delegate" || !outcome.tool_result.success {
            return Ok(Decision::Continue);
        }
        let review = build_subagent_review(&outcome.tool_result.output);
        if review.is_empty() {
            return Ok(Decision::Continue);
        }
        // PR 13: evidence-ref protocol. A sub-agent that returned actual
        // evidence references gets folded in as a *trusted* plan reminder
        // — the parent should re-read those refs but can act on them.
        // A sub-agent that returned no evidence gets downgraded: we
        // record the review as a `SubAgentSummary` source so the
        // context layer knows it's an untrusted claim, and we suffix
        // an explicit "no evidence refs" notice so the model can't
        // accidentally treat the prose as ground truth.
        let evidence_present = outcome
            .tool_result
            .output
            .get("evidence_refs")
            .and_then(|v| v.as_array())
            .map(|arr| !arr.is_empty())
            .unwrap_or(false);
        if evidence_present {
            cx.context
                .append(ContextEntry::trusted(ContextSource::PlanReminder, review));
        } else {
            let downgraded = format!(
                "{review}\n\n[sub-agent review] NOTE: this sub-agent returned no evidence_refs. \
                 Do not trust its summary — re-read the relevant files yourself before acting."
            );
            cx.context.append(ContextEntry::trusted(
                ContextSource::SubAgentSummary,
                downgraded,
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
    use peridot_common::{ExecutionMode, HooksConfig, PermissionMode, SecurityConfig, ToolResult};
    use peridot_context::ContextManager;
    use peridot_llm::Usage;

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

    fn run(outcome: AgentTurnOutcome, context: &mut ContextManager) -> usize {
        let mut state = AgentState::new(ExecutionMode::Plan, PermissionMode::Safe);
        let request = make_request();
        let mut usage = Usage::default();
        let hooks = HooksConfig::default();
        let security = SecurityConfig::default();
        let mut events_buf = Vec::<AgentRunEvent>::new();
        let mut events = |e: AgentRunEvent| events_buf.push(e);
        let project_root = std::path::PathBuf::from(".");
        let mut policies: Vec<Box<dyn LoopPolicy>> = vec![Box::new(SubAgentReviewPolicy::new())];
        let initial = context.entries().len();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
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
                provider: &crate::loop_policy::test_support::NoopProvider,
                tool_dispatcher: &crate::loop_policy::test_support::NoopToolDispatcher,
                subagent_runner: None,
                pending_resume_path: None,
            };
            dispatch_post_turn(&mut policies, &mut cx, &outcome)
                .await
                .unwrap();
        });
        context.entries().len() - initial
    }

    #[test]
    fn appends_review_for_successful_delegate() {
        let mut context = ContextManager::new();
        let outcome = AgentTurnOutcome {
            tool_name: "agent_delegate".to_string(),
            tool_result: ToolResult::success(
                "delegated",
                serde_json::json!({
                    "summary": "subagent did the thing",
                    "workspace": "/tmp/subagent",
                    "diff": "+++ src/lib.rs\n+fn new_fn() {}\n",
                }),
            ),
            usage: Usage::default(),
            done: false,
        };
        let added = run(outcome, &mut context);
        assert_eq!(added, 1, "expected one plan-reminder entry to be appended");
    }

    #[test]
    fn downgrades_review_when_no_evidence_refs() {
        let mut context = ContextManager::new();
        let outcome = AgentTurnOutcome {
            tool_name: "agent_delegate".to_string(),
            tool_result: ToolResult::success(
                "delegated",
                serde_json::json!({
                    "summary": "subagent claims success",
                    "workspace": "/tmp/subagent",
                    "diff": "+++ src/lib.rs\n+fn foo() {}\n",
                    // evidence_refs absent → downgrade to SubAgentSummary
                }),
            ),
            usage: Usage::default(),
            done: false,
        };
        run(outcome, &mut context);
        let last = context
            .entries()
            .last()
            .expect("policy should append entry");
        assert_eq!(
            last.source,
            peridot_context::ContextSource::SubAgentSummary,
            "no evidence_refs should downgrade source to SubAgentSummary",
        );
        assert!(
            last.content.contains("no evidence_refs"),
            "downgrade notice must be appended",
        );
    }

    #[test]
    fn trusts_review_when_evidence_refs_present() {
        let mut context = ContextManager::new();
        let outcome = AgentTurnOutcome {
            tool_name: "agent_delegate".to_string(),
            tool_result: ToolResult::success(
                "delegated",
                serde_json::json!({
                    "summary": "subagent inspected files",
                    "workspace": "/tmp/subagent",
                    "diff": "+++ src/lib.rs\n+fn foo() {}\n",
                    "evidence_refs": [
                        {"kind": "file", "id": "src/lib.rs", "summary": "L1-10"}
                    ],
                }),
            ),
            usage: Usage::default(),
            done: false,
        };
        run(outcome, &mut context);
        let last = context
            .entries()
            .last()
            .expect("policy should append entry");
        assert_eq!(
            last.source,
            peridot_context::ContextSource::PlanReminder,
            "presence of evidence_refs should keep source as PlanReminder",
        );
    }

    #[test]
    fn skips_non_delegate_tools() {
        let mut context = ContextManager::new();
        let outcome = AgentTurnOutcome {
            tool_name: "file_read".to_string(),
            tool_result: ToolResult::success("ok", serde_json::Value::Null),
            usage: Usage::default(),
            done: false,
        };
        let added = run(outcome, &mut context);
        assert_eq!(added, 0);
    }

    #[test]
    fn skips_failed_delegate() {
        let mut context = ContextManager::new();
        let outcome = AgentTurnOutcome {
            tool_name: "agent_delegate".to_string(),
            tool_result: ToolResult::failure("delegate failed"),
            usage: Usage::default(),
            done: false,
        };
        let added = run(outcome, &mut context);
        assert_eq!(added, 0);
    }
}
