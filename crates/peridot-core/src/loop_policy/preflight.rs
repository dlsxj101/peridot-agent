//! Deterministic preflight checks gating `agent_done`.
//!
//! Today the only thing that can veto `agent_done` is the optional LLM
//! grader (and that's free to silently fall through on provider errors).
//! The preflight policy adds cheap, deterministic checks BEFORE the
//! grader so the most common "premature done" failure modes get caught
//! without an LLM round-trip.
//!
//! Each check returns [`Decision::SkipTurn`] (with a plan reminder
//! injected into context) so the loop keeps running and the model gets
//! a chance to address the issue. None of them stop the run outright —
//! they just block the done state until the conditions are met.

use peridot_common::PeriResult;
use peridot_context::{ContextEntry, ContextSource};

use crate::loop_policy::{Decision, LoopPolicy, PolicyCx};
use crate::requests::AgentTurnOutcome;

/// Configuration for which preflight checks are active.
///
/// Both flags default to OFF. Existing test fixtures (and many real-world
/// flows) call `agent_done` directly after a mutation without running
/// `verify_*`; enabling preflight unconditionally would mass-break those.
/// Once auto-verify is a policy itself (PR plan migration step 4) the
/// loop will be guaranteed to verify before done, and these defaults can
/// flip to true. `peridot run --no-preflight-foo` knobs can be wired later.
#[derive(Clone, Copy, Debug, Default)]
pub struct PreflightConfig {
    /// If true, require a successful `verify_*` tool call after the
    /// last mutating tool before accepting `agent_done`.
    pub require_verify_after_mutation: bool,
    /// If true, require no `pending`-state tools to remain.
    pub require_no_pending_tools: bool,
}

/// Runs deterministic checks each time the loop wants to accept
/// `agent_done`. See [`PreflightConfig`] for the individual rules.
pub struct PreflightPolicy {
    config: PreflightConfig,
}

impl PreflightPolicy {
    /// Construct with default checks enabled.
    pub fn new() -> Self {
        Self::with_config(PreflightConfig::default())
    }

    /// Construct with an explicit config.
    pub fn with_config(config: PreflightConfig) -> Self {
        Self { config }
    }

    /// Builder shortcut to flip the "verify after mutation" check on.
    /// Useful while the rest of the policy migration is still in
    /// progress and the default needs to stay off.
    pub fn with_verify_after_mutation(mut self) -> Self {
        self.config.require_verify_after_mutation = true;
        self
    }

    /// Builder shortcut for the no-pending-tools rule.
    pub fn with_no_pending_tools(mut self) -> Self {
        self.config.require_no_pending_tools = true;
        self
    }

    fn is_mutating_tool(name: &str) -> bool {
        matches!(
            name,
            "file_write" | "file_patch" | "shell" | "git_commit" | "agent_delegate"
        )
    }

    fn is_verify_tool(name: &str) -> bool {
        name.starts_with("verify_")
    }

    /// Returns Some(reason) if the rule applies and the done state
    /// should be blocked, else None.
    ///
    /// `auto_verify_marker` is true when any context entry contains a
    /// successful `[auto-verify] verify_build passed` PlanReminder.
    /// `auto_verify_after_mutation` (the helper inside `HarnessAgent`)
    /// produces that marker as a side-effect rather than a turn
    /// outcome, so the policy has to consult it alongside `outcomes`.
    /// A future move of auto-verify into a real `LoopPolicy::post_turn`
    /// impl will emit a synthetic outcome and obsolete this marker.
    fn unverified_mutation_reason(
        outcomes: &[AgentTurnOutcome],
        auto_verify_marker: bool,
    ) -> Option<String> {
        // Find the most recent successful mutation.
        let (mut_idx, mutation) = outcomes
            .iter()
            .enumerate()
            .rev()
            .find(|(_, o)| Self::is_mutating_tool(&o.tool_name) && o.tool_result.success)?;
        // Search forward for a successful verify_* after that mutation.
        let verified_by_outcome = outcomes
            .iter()
            .skip(mut_idx + 1)
            .any(|o| Self::is_verify_tool(&o.tool_name) && o.tool_result.success);
        if verified_by_outcome || auto_verify_marker {
            None
        } else {
            Some(format!(
                "[preflight] Last mutation (`{}`) is not yet covered by a successful verify_* run. \
                 Run verify_build / verify_test / verify_lint before declaring done.",
                mutation.tool_name
            ))
        }
    }

    /// Scans context entries for a successful auto-verify marker.
    /// True when the helper-driven `auto_verify_after_mutation` left
    /// a `[auto-verify] verify_build passed` PlanReminder in context.
    fn has_auto_verify_marker(context: &peridot_context::ContextManager) -> bool {
        context.entries().iter().any(|entry| {
            entry.content.contains("[auto-verify]") && entry.content.contains("passed")
        })
    }
}

impl Default for PreflightPolicy {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl LoopPolicy for PreflightPolicy {
    fn name(&self) -> &'static str {
        "preflight"
    }

    async fn on_done(
        &mut self,
        cx: &mut PolicyCx<'_>,
        outcomes: &[AgentTurnOutcome],
    ) -> PeriResult<Decision> {
        if self.config.require_verify_after_mutation {
            let auto_verify_marker = Self::has_auto_verify_marker(cx.context);
            if let Some(reason) = Self::unverified_mutation_reason(outcomes, auto_verify_marker) {
                cx.context
                    .append(ContextEntry::trusted(ContextSource::PlanReminder, reason));
                return Ok(Decision::SkipTurn);
            }
        }
        Ok(Decision::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peridot_common::ToolResult;
    use peridot_llm::Usage;

    fn outcome(name: &str, success: bool, done: bool) -> AgentTurnOutcome {
        AgentTurnOutcome {
            tool_name: name.to_string(),
            tool_result: if success {
                ToolResult::success(name, serde_json::Value::Null)
            } else {
                ToolResult::failure(name)
            },
            usage: Usage::default(),
            done,
        }
    }

    #[test]
    fn mutation_without_verify_blocks_done() {
        let outcomes = vec![
            outcome("file_patch", true, false),
            outcome("agent_done", true, true),
        ];
        let reason = PreflightPolicy::unverified_mutation_reason(&outcomes, false);
        assert!(
            reason.is_some(),
            "expected preflight to flag the missing verify"
        );
    }

    #[test]
    fn auto_verify_marker_satisfies_check_without_outcome() {
        // helper-driven auto-verify leaves a `[auto-verify] verify_build
        // passed` PlanReminder in context but no verify_* turn outcome.
        // The marker should still satisfy the preflight check so this
        // path is usable while auto-verify is still a helper.
        let outcomes = vec![
            outcome("file_patch", true, false),
            outcome("agent_done", true, true),
        ];
        let reason = PreflightPolicy::unverified_mutation_reason(&outcomes, true);
        assert!(
            reason.is_none(),
            "expected the auto-verify marker to clear the preflight check"
        );
    }

    #[test]
    fn mutation_followed_by_successful_verify_allows_done() {
        let outcomes = vec![
            outcome("file_patch", true, false),
            outcome("verify_build", true, false),
            outcome("agent_done", true, true),
        ];
        let reason = PreflightPolicy::unverified_mutation_reason(&outcomes, false);
        assert!(
            reason.is_none(),
            "expected preflight to clear after a successful verify_build"
        );
    }

    #[test]
    fn failed_verify_does_not_count_as_coverage() {
        let outcomes = vec![
            outcome("file_patch", true, false),
            outcome("verify_test", false, false),
        ];
        let reason = PreflightPolicy::unverified_mutation_reason(&outcomes, false);
        assert!(
            reason.is_some(),
            "expected a failed verify to leave the mutation uncovered"
        );
    }

    #[test]
    fn no_mutations_means_no_preflight_complaint() {
        let outcomes = vec![outcome("file_read", true, false)];
        let reason = PreflightPolicy::unverified_mutation_reason(&outcomes, false);
        assert!(reason.is_none());
    }

    #[test]
    fn mutation_after_verify_still_blocks_done() {
        // If the model verifies, then mutates again, the new mutation
        // is uncovered — preflight must catch this.
        let outcomes = vec![
            outcome("file_patch", true, false),
            outcome("verify_build", true, false),
            outcome("file_patch", true, false),
        ];
        let reason = PreflightPolicy::unverified_mutation_reason(&outcomes, false);
        assert!(
            reason.is_some(),
            "fresh mutation after verify must re-trigger preflight"
        );
    }
}
