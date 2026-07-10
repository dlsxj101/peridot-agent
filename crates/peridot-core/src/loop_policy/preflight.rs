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
/// `require_verify_after_mutation` now defaults to ON: auto-verify is a
/// policy that flushes a `verify_build` on the settling turn (including
/// the `agent_done` turn itself), so by the time this gate runs the most
/// recent mutation is either covered by a fresh `[auto-verify]` marker or
/// the model's own `verify_*` outcome. A FAILED marker blocks done; a
/// skip / infra-error marker (no build command, couldn't launch) only
/// warns and lets done through.
#[derive(Clone, Copy, Debug)]
pub struct PreflightConfig {
    /// If true, require a successful `verify_*` tool call (or a passing
    /// auto-verify) after the last mutating tool before accepting
    /// `agent_done`.
    pub require_verify_after_mutation: bool,
    /// If true, require no `pending`-state tools to remain.
    pub require_no_pending_tools: bool,
}

impl Default for PreflightConfig {
    fn default() -> Self {
        Self {
            require_verify_after_mutation: true,
            require_no_pending_tools: false,
        }
    }
}

/// Classification of the most recent `[auto-verify]` marker in context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AutoVerifyStatus {
    /// `verify_build passed`.
    Passed,
    /// `verify_build FAILED`.
    Failed,
    /// No build command could be resolved — neither pass nor fail.
    Skipped,
    /// The verify command could not even be launched (infra error).
    CouldNotRun,
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
    /// `auto_verify` is the classification of the most recent
    /// `[auto-verify]` marker in context (auto-verify produces that
    /// marker as a side-effect rather than a turn outcome, so the gate
    /// consults it alongside `outcomes`). A passing model-driven
    /// `verify_*` outcome after the last mutation also clears the gate.
    fn unverified_mutation_reason(
        outcomes: &[AgentTurnOutcome],
        auto_verify: Option<AutoVerifyStatus>,
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
        if verified_by_outcome {
            return None;
        }
        match auto_verify {
            // Green, or nothing to verify against, or infra hiccup — do
            // not block. Skip / CouldNotRun only warn (the marker itself
            // is already in context for the operator).
            Some(AutoVerifyStatus::Passed)
            | Some(AutoVerifyStatus::Skipped)
            | Some(AutoVerifyStatus::CouldNotRun) => None,
            Some(AutoVerifyStatus::Failed) => Some(format!(
                "[preflight] Last mutation (`{}`) did not pass verification — see the \
                 [auto-verify] FAILED note and auto-fix directive above. Fix it and let \
                 verify pass before declaring done.",
                mutation.tool_name
            )),
            None => Some(format!(
                "[preflight] Last mutation (`{}`) is not yet covered by a successful verify_* run. \
                 Run verify_build / verify_test / verify_lint before declaring done.",
                mutation.tool_name
            )),
        }
    }

    /// Classifies the most recent `[auto-verify]` PlanReminder in
    /// context, newest first. `None` when no auto-verify note exists.
    fn latest_auto_verify_status(
        context: &peridot_context::ContextManager,
    ) -> Option<AutoVerifyStatus> {
        for entry in context.entries().iter().rev() {
            let content = entry.content.trim();
            let Some(rest) = content.strip_prefix("[auto-verify]") else {
                continue;
            };
            // Classify on the marker head only (everything before the
            // first colon). The tail carries the raw verify summary,
            // which for a failing test command routinely contains words
            // like "skipped" or "passed" — matching on the whole content
            // would let a FAILED marker slip through the done gate.
            let head = rest.trim_start();
            let head = head.split(':').next().unwrap_or(head).trim_end();
            match head {
                "verify_build could not run" => return Some(AutoVerifyStatus::CouldNotRun),
                "skipped" => return Some(AutoVerifyStatus::Skipped),
                "verify_build FAILED" => return Some(AutoVerifyStatus::Failed),
                "verify_build passed" => return Some(AutoVerifyStatus::Passed),
                // Unrecognised [auto-verify] shape — keep scanning older ones.
                _ => {}
            }
        }
        None
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
            let auto_verify = Self::latest_auto_verify_status(cx.context);
            if let Some(reason) = Self::unverified_mutation_reason(outcomes, auto_verify) {
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
        let reason = PreflightPolicy::unverified_mutation_reason(&outcomes, None);
        assert!(
            reason.is_some(),
            "expected preflight to flag the missing verify"
        );
    }

    #[test]
    fn passed_auto_verify_marker_satisfies_check_without_outcome() {
        // Auto-verify leaves a `[auto-verify] verify_build passed`
        // PlanReminder but no verify_* turn outcome — the marker still
        // clears the gate.
        let outcomes = vec![
            outcome("file_patch", true, false),
            outcome("agent_done", true, true),
        ];
        let reason =
            PreflightPolicy::unverified_mutation_reason(&outcomes, Some(AutoVerifyStatus::Passed));
        assert!(
            reason.is_none(),
            "expected the passed auto-verify marker to clear the preflight check"
        );
    }

    #[test]
    fn failed_auto_verify_marker_blocks_done() {
        let outcomes = vec![
            outcome("file_patch", true, false),
            outcome("agent_done", true, true),
        ];
        let reason =
            PreflightPolicy::unverified_mutation_reason(&outcomes, Some(AutoVerifyStatus::Failed));
        let reason = reason.expect("FAILED auto-verify must block done");
        assert!(reason.contains("did not pass verification"));
    }

    #[test]
    fn skipped_auto_verify_marker_allows_done() {
        // No build command detected → auto-verify skipped → the gate
        // must not block (there is nothing to verify against).
        let outcomes = vec![
            outcome("file_patch", true, false),
            outcome("agent_done", true, true),
        ];
        let reason =
            PreflightPolicy::unverified_mutation_reason(&outcomes, Some(AutoVerifyStatus::Skipped));
        assert!(reason.is_none(), "a skip must let done through");
    }

    #[test]
    fn could_not_run_auto_verify_marker_allows_done() {
        let outcomes = vec![
            outcome("file_patch", true, false),
            outcome("agent_done", true, true),
        ];
        let reason = PreflightPolicy::unverified_mutation_reason(
            &outcomes,
            Some(AutoVerifyStatus::CouldNotRun),
        );
        assert!(reason.is_none(), "an infra error must only warn, not block");
    }

    #[test]
    fn mutation_followed_by_successful_verify_allows_done() {
        let outcomes = vec![
            outcome("file_patch", true, false),
            outcome("verify_build", true, false),
            outcome("agent_done", true, true),
        ];
        let reason = PreflightPolicy::unverified_mutation_reason(&outcomes, None);
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
        let reason = PreflightPolicy::unverified_mutation_reason(&outcomes, None);
        assert!(
            reason.is_some(),
            "expected a failed verify to leave the mutation uncovered"
        );
    }

    #[test]
    fn no_mutations_means_no_preflight_complaint() {
        let outcomes = vec![outcome("file_read", true, false)];
        let reason = PreflightPolicy::unverified_mutation_reason(&outcomes, None);
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
        let reason = PreflightPolicy::unverified_mutation_reason(&outcomes, None);
        assert!(
            reason.is_some(),
            "fresh mutation after verify must re-trigger preflight"
        );
    }

    #[test]
    fn latest_auto_verify_status_classifies_markers() {
        use peridot_context::{ContextEntry, ContextManager, ContextSource};
        let mut context = ContextManager::new();
        context.append(ContextEntry::trusted(
            ContextSource::PlanReminder,
            "[auto-verify] verify_build passed: ok".to_string(),
        ));
        assert_eq!(
            PreflightPolicy::latest_auto_verify_status(&context),
            Some(AutoVerifyStatus::Passed)
        );
        context.append(ContextEntry::trusted(
            ContextSource::PlanReminder,
            "[auto-verify] verify_build FAILED: boom".to_string(),
        ));
        assert_eq!(
            PreflightPolicy::latest_auto_verify_status(&context),
            Some(AutoVerifyStatus::Failed),
            "newest marker wins"
        );
        context.append(ContextEntry::trusted(
            ContextSource::PlanReminder,
            "[auto-verify] skipped: no build command detected".to_string(),
        ));
        assert_eq!(
            PreflightPolicy::latest_auto_verify_status(&context),
            Some(AutoVerifyStatus::Skipped)
        );
    }

    #[test]
    fn failed_marker_with_skipped_or_passed_in_summary_still_classifies_failed() {
        use peridot_context::{ContextEntry, ContextManager, ContextSource};
        let mut context = ContextManager::new();
        // A failing test command's summary routinely contains "passed"
        // and "skipped" — the classifier must key off the marker head,
        // not the summary tail, or the done gate opens on a failure.
        context.append(ContextEntry::trusted(
            ContextSource::PlanReminder,
            "[auto-verify] verify_build FAILED: 3 passed; 1 failed; 2 skipped\nFix this before declaring agent_done."
                .to_string(),
        ));
        assert_eq!(
            PreflightPolicy::latest_auto_verify_status(&context),
            Some(AutoVerifyStatus::Failed)
        );
    }
}
