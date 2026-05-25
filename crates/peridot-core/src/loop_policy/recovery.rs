//! Structured error recovery.
//!
//! Replaces the inline `Err(err)` branch of the run loop. When a turn
//! returns an error, this policy:
//!   1. Branches on [`classify_error`] to choose category-specific
//!      recovery behaviour (parse → format reminder, api_error →
//!      exponential backoff, permission → ask user, etc.).
//!   2. Appends a recovery reminder to context.
//!   3. Tracks a per-signature attempt counter; gives up after
//!      [`MAX_ERROR_RECOVERY_ATTEMPTS`].
//!   4. Returns [`Decision::Retry`] to re-run the turn, or
//!      [`Decision::Stop`] when the budget is exhausted.

use peridot_common::{AgentPhase, PeriError, PeriResult};
use peridot_context::{ContextEntry, ContextSource};

use crate::agent::transition_phase;
use crate::loop_policy::{Decision, LoopPolicy, PolicyCx};
use crate::recovery::{
    classify_error, format_reminder_message, recovery_analysis_message, recovery_message,
    run_error_event_hooks, run_recovery_event_hook,
};
use crate::requests::{AgentRunEvent, AgentTurnOutcome, StopReason};

/// Maximum number of consecutive recovery attempts before aborting.
/// Mirrors the inline constant in agent.rs to keep behaviour identical.
const MAX_ERROR_RECOVERY_ATTEMPTS: usize = 3;
/// Number of consecutive parse failures before we escalate the format
/// reminder into a louder, more explicit version.
const FORMAT_REMINDER_THRESHOLD: usize = 3;

/// Stateful structured-recovery policy.
pub struct ErrorRecoveryPolicy {
    /// Number of consecutive retries with the same error signature.
    attempts: usize,
    /// Most recent error signature; used to detect "same failure repeated".
    last_signature: Option<String>,
    /// Counts consecutive parse failures so the format reminder can
    /// escalate from "here's the protocol" to "you're still getting it
    /// wrong, read this carefully" without spamming the reminder every
    /// turn.
    consecutive_parse_failures: usize,
}

impl ErrorRecoveryPolicy {
    pub fn new() -> Self {
        Self {
            attempts: 0,
            last_signature: None,
            consecutive_parse_failures: 0,
        }
    }
}

impl Default for ErrorRecoveryPolicy {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl LoopPolicy for ErrorRecoveryPolicy {
    fn name(&self) -> &'static str {
        "error_recovery"
    }

    /// Reset retry state after every successful turn — only consecutive
    /// failures should escalate.
    async fn post_turn(
        &mut self,
        _cx: &mut PolicyCx<'_>,
        _outcome: &AgentTurnOutcome,
    ) -> PeriResult<Decision> {
        self.attempts = 0;
        self.last_signature = None;
        self.consecutive_parse_failures = 0;
        Ok(Decision::Continue)
    }

    async fn on_turn_error(
        &mut self,
        cx: &mut PolicyCx<'_>,
        err: &PeriError,
    ) -> PeriResult<Decision> {
        let category = classify_error(err);
        // Early exit for terminal categories — no retry buys anything.
        //
        // NOTE on "permission": the driver's `approval_required_error`
        // early-exit (in `run_until_done_with_events`) already routes
        // *recoverable* permission errors ("requires explicit user
        // approval") to the approval-resume path before we even see them.
        // The permission errors that reach this policy are *hard blocks*
        // (path-boundary violations, denied destructive shell commands).
        // For those, retrying won't help, but bailing with
        // `StopReason::ApprovalRequired` is wrong too — it'd misclassify
        // the failure as a recoverable pause. So we fall through to the
        // generic retry path, which will hit MAX_ERROR_RECOVERY_ATTEMPTS
        // and exit cleanly. With `max_turns=1` the loop simply terminates
        // with `MaxTurns`, matching the pre-extraction behaviour.
        if category == "config" {
            // Config errors are operator-actionable, not model-recoverable.
            // Stop immediately so the user sees the real problem.
            return Ok(Decision::Stop(
                StopReason::Interrupted,
                Some(format!("Config error (not retried): {err}")),
            ));
        }

        transition_phase(cx.state, AgentPhase::Recovering, "turn_error", cx.events);
        run_error_event_hooks(cx.project_root, cx.hooks, err)?;

        if category == "parse" {
            self.consecutive_parse_failures += 1;
        } else {
            self.consecutive_parse_failures = 0;
        }

        // Always inject the standard recovery reminder so the model
        // sees fresh guidance on the next turn.
        cx.context.append(ContextEntry::trusted(
            ContextSource::PlanReminder,
            format!(
                "{}\n\n{}",
                recovery_message(err),
                recovery_analysis_message(err)
            ),
        ));

        // Persistent parse failures get the explicit format reminder
        // appended on top of the generic recovery message.
        if self.consecutive_parse_failures >= FORMAT_REMINDER_THRESHOLD {
            cx.context.append(ContextEntry::trusted(
                ContextSource::PlanReminder,
                format_reminder_message(),
            ));
        }

        let signature = err.to_string();
        if self.last_signature.as_deref() == Some(signature.as_str()) {
            self.attempts += 1;
        } else {
            self.last_signature = Some(signature.clone());
            self.attempts = 1;
        }

        (cx.events)(AgentRunEvent::Recovery {
            message: signature.clone(),
        });

        if self.attempts >= MAX_ERROR_RECOVERY_ATTEMPTS {
            let message = format!(
                "Recovery failed after {} attempts. Peridot stopped instead of retrying \
                 indefinitely. Reason: {signature}. Check the provider/model setting, \
                 credentials, network/API error, or task direction, then run again.",
                self.attempts
            );
            transition_phase(
                cx.state,
                AgentPhase::Recovering,
                "recovery_abort",
                cx.events,
            );
            run_recovery_event_hook(cx.project_root, cx.hooks, "recovery_abort", &message)?;
            return Ok(Decision::Stop(StopReason::Interrupted, Some(message)));
        }

        // API errors get a brief backoff before the retry. Other
        // categories retry immediately — the model will see the
        // reminder we just appended and adjust its next turn.
        if category == "api_error" {
            // Exponential-ish backoff capped at 16s. Sleep is `cfg(test)`
            // disabled via the existing helper so unit tests run fast.
            let secs = match self.attempts {
                1 => 1,
                2 => 4,
                _ => 16,
            };
            sleep_for_backoff(secs).await;
        }

        Ok(Decision::Retry)
    }
}

#[cfg(not(test))]
async fn sleep_for_backoff(secs: u64) {
    tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
}

#[cfg(test)]
async fn sleep_for_backoff(_secs: u64) {
    // Tests don't actually wait — the categorisation is the only
    // observable behaviour under test.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_policy::dispatch_on_turn_error;
    use crate::requests::AgentRunRequest;
    use crate::state::AgentState;
    use peridot_common::{ExecutionMode, HooksConfig, PermissionMode, SecurityConfig};
    use peridot_context::ContextManager;
    use peridot_llm::Usage;

    fn make_request() -> AgentRunRequest {
        AgentRunRequest {
            task: String::new(),
            model: "mock".to_string(),
            goal_checker_model: None,
            max_turns: 5,
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

    fn run(
        err: PeriError,
        policy: &mut ErrorRecoveryPolicy,
        context: &mut ContextManager,
    ) -> Decision {
        let mut state = AgentState::new(ExecutionMode::Plan, PermissionMode::Safe);
        let request = make_request();
        let mut usage = Usage::default();
        let hooks = HooksConfig::default();
        let security = SecurityConfig::default();
        let mut events_buf = Vec::<AgentRunEvent>::new();
        let mut events = |e: AgentRunEvent| events_buf.push(e);
        let project_root = std::path::PathBuf::from(".");
        let mut policies: Vec<Box<dyn LoopPolicy>> = Vec::new();
        // Test exercises the policy directly via on_turn_error so the
        // dispatcher's policies list stays empty.
        let _ = &mut policies;
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
            policy.on_turn_error(&mut cx, &err).await.unwrap()
        })
    }

    #[test]
    fn config_error_stops_immediately() {
        let mut policy = ErrorRecoveryPolicy::new();
        let mut context = ContextManager::new();
        let decision = run(
            PeriError::Config("bad config".into()),
            &mut policy,
            &mut context,
        );
        match decision {
            Decision::Stop(StopReason::Interrupted, Some(msg)) => {
                assert!(msg.contains("Config error"));
            }
            other => panic!("expected Stop(Interrupted, _), got {other:?}"),
        }
    }

    #[test]
    fn hard_block_permission_error_falls_through_to_retry_path() {
        // Hard-blocked permission errors (path-boundary violations,
        // denied destructive shell) are NOT special-cased here — the
        // driver's `approval_required_error` early-exit already routed
        // recoverable approval-required errors away before they reached
        // the policy. What's left is the unrecoverable kind; it gets
        // the normal retry-with-counter treatment so eventually
        // MAX_ERROR_RECOVERY_ATTEMPTS aborts (or `max_turns` does).
        let mut policy = ErrorRecoveryPolicy::new();
        let mut context = ContextManager::new();
        let decision = run(
            PeriError::PermissionDenied("can't write /etc/passwd".into()),
            &mut policy,
            &mut context,
        );
        assert!(
            matches!(decision, Decision::Retry),
            "permission errors that pass the approval-required early-exit should retry, not stop with ApprovalRequired; got {decision:?}"
        );
    }

    #[test]
    fn parse_error_retries_and_appends_reminder() {
        let mut policy = ErrorRecoveryPolicy::new();
        let mut context = ContextManager::new();
        let initial = context.entries().len();
        let decision = run(
            PeriError::Parse("bad json".into()),
            &mut policy,
            &mut context,
        );
        assert!(matches!(decision, Decision::Retry));
        assert!(
            context.entries().len() > initial,
            "expected a recovery reminder to be appended"
        );
        assert_eq!(policy.consecutive_parse_failures, 1);
    }

    #[test]
    fn three_consecutive_parse_failures_escalates_reminder() {
        let mut policy = ErrorRecoveryPolicy::new();
        let mut context = ContextManager::new();
        // First two failures append the generic reminder.
        for _ in 0..2 {
            run(
                PeriError::Parse("bad json".into()),
                &mut policy,
                &mut context,
            );
        }
        let before_third = context.entries().len();
        run(
            PeriError::Parse("bad json".into()),
            &mut policy,
            &mut context,
        );
        // Third failure adds BOTH the standard reminder and the
        // explicit format reminder.
        assert!(
            context.entries().len() >= before_third + 2,
            "expected format-reminder escalation on 3rd parse failure"
        );
    }

    #[test]
    fn repeated_same_error_eventually_stops() {
        let mut policy = ErrorRecoveryPolicy::new();
        let mut context = ContextManager::new();
        // 1st and 2nd retries.
        for _ in 0..2 {
            let d = run(
                PeriError::Tool("transient".into()),
                &mut policy,
                &mut context,
            );
            assert!(matches!(d, Decision::Retry));
        }
        // 3rd attempt with the same signature: gives up.
        let d = run(
            PeriError::Tool("transient".into()),
            &mut policy,
            &mut context,
        );
        match d {
            Decision::Stop(StopReason::Interrupted, Some(msg)) => {
                assert!(msg.contains("Recovery failed after"));
            }
            other => panic!("expected Stop after 3rd identical error, got {other:?}"),
        }
    }

    #[test]
    fn distinct_signatures_do_not_share_attempt_counter() {
        let mut policy = ErrorRecoveryPolicy::new();
        let mut context = ContextManager::new();
        // Three different errors — each is the first instance of its
        // signature, so all should Retry.
        for kind in ["alpha", "beta", "gamma"] {
            let d = run(PeriError::Tool(kind.into()), &mut policy, &mut context);
            assert!(
                matches!(d, Decision::Retry),
                "{kind} should retry, got {d:?}"
            );
        }
    }

    #[tokio::test]
    async fn dispatcher_returns_recovery_decision() {
        // Smoke-test the dispatcher integration.
        let mut policies: Vec<Box<dyn LoopPolicy>> = vec![Box::new(ErrorRecoveryPolicy::new())];
        let mut state = AgentState::new(ExecutionMode::Plan, PermissionMode::Safe);
        let mut context = ContextManager::new();
        let request = make_request();
        let mut usage = Usage::default();
        let hooks = HooksConfig::default();
        let security = SecurityConfig::default();
        let mut events_buf = Vec::<AgentRunEvent>::new();
        let mut events = |e: AgentRunEvent| events_buf.push(e);
        let project_root = std::path::PathBuf::from(".");
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
        let decision =
            dispatch_on_turn_error(&mut policies, &mut cx, &PeriError::Parse("bad".into()))
                .await
                .unwrap();
        assert!(matches!(decision, Decision::Retry));
    }
}
