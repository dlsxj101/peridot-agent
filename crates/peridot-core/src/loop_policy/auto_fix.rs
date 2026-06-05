//! Auto-fix loop policy.
//!
//! When `verify_*` fails, this policy injects a structured "fix this
//! first" directive keyed by the failure signature so repeated identical
//! failures force a strategy change. When the same `(tool, signature)`
//! pair fails `fix_cap` times in a row the policy returns
//! [`Decision::Stop`] with the circuit-breaker message; the caller is
//! expected to terminate the run with [`StopReason::Interrupted`].
//!
//! Replaces the inline `HarnessAgent::handle_auto_fix_loop` helper and
//! the `verify_failure_state: Option<VerifyFailureState>` local that
//! the driver carried.

use peridot_common::{AgentPhase, PeriResult};
use peridot_context::{ContextEntry, ContextSource};

use crate::agent::transition_phase;
use crate::loop_policy::{Decision, LoopPolicy, PolicyCx};
use crate::recovery::run_recovery_event_hook;
use crate::requests::{AgentRunEvent, AgentTurnOutcome, StopReason};
use crate::verify_failure::{
    VerifyFailureState, update_verify_failure_state, verify_failure_directive,
};

/// Tracks the most recent `verify_*` failure across turns. When the
/// model keeps trying the same fix and getting the same failure, the
/// circuit-breaker trips after `fix_cap` attempts.
pub struct AutoFixLoopPolicy {
    fix_cap: u32,
    state: Option<VerifyFailureState>,
}

impl AutoFixLoopPolicy {
    pub fn new(fix_cap: u32) -> Self {
        Self {
            fix_cap,
            state: None,
        }
    }
}

#[async_trait::async_trait]
impl LoopPolicy for AutoFixLoopPolicy {
    fn name(&self) -> &'static str {
        "auto_fix_loop"
    }

    async fn post_turn(
        &mut self,
        cx: &mut PolicyCx<'_>,
        outcome: &AgentTurnOutcome,
    ) -> PeriResult<Decision> {
        let is_verify_tool = outcome.tool_name.starts_with("verify_");
        let turn_success = outcome.tool_result.success;
        if is_verify_tool && !turn_success {
            let failure = update_verify_failure_state(&mut self.state, outcome).clone();
            (cx.events)(AgentRunEvent::AutoFixAttempt {
                attempt: failure.attempts,
                max: self.fix_cap,
                tool_name: failure.tool_name.clone(),
                passed: false,
            });
            if failure.attempts >= self.fix_cap {
                let message = format!(
                    "Auto-fix loop circuit breaker: {} failed {} times with signature `{}`. Aborting so the operator can intervene.",
                    failure.tool_name, failure.attempts, failure.signature
                );
                transition_phase(
                    cx.state,
                    AgentPhase::Recovering,
                    "auto_fix_abort",
                    cx.events,
                );
                run_recovery_event_hook(cx.project_root, cx.hooks, "auto_fix_abort", &message)?;
                return Ok(Decision::Stop(StopReason::Interrupted, Some(message)));
            }
            cx.context.append(ContextEntry::trusted(
                ContextSource::PlanReminder,
                verify_failure_directive(&failure, self.fix_cap),
            ));
        } else {
            if is_verify_tool && turn_success && self.state.is_some() {
                (cx.events)(AgentRunEvent::AutoFixAttempt {
                    attempt: self.state.as_ref().map(|s| s.attempts + 1).unwrap_or(1),
                    max: self.fix_cap,
                    tool_name: outcome.tool_name.clone(),
                    passed: true,
                });
            }
            self.state = None;
        }
        Ok(Decision::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_policy::test_support::{NoopProvider, NoopToolDispatcher};
    use crate::loop_policy::{PolicyCx, dispatch_post_turn};
    use crate::requests::{AgentRunEvent, AgentRunRequest};
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

    fn outcome(name: &str, success: bool) -> AgentTurnOutcome {
        // verify_build outcomes include a `signature` field in the
        // structured output; AutoFixLoopPolicy uses that to detect
        // "same failure repeated." Construct directly so failed
        // outcomes can carry a payload (no failure_with_output helper).
        AgentTurnOutcome {
            tool_name: name.to_string(),
            tool_result: ToolResult {
                success,
                summary: if success {
                    "verify_build passed".to_string()
                } else {
                    "verify_build failed: cargo error".to_string()
                },
                output: serde_json::json!({"signature": "fixed-signature"}),
            },
            usage: Usage::default(),
            done: false,
        }
    }

    #[tokio::test]
    async fn auto_fix_aborts_after_signature_repeats_past_cap() {
        let mut state = AgentState::new(ExecutionMode::Execute, PermissionMode::Auto);
        let mut context = ContextManager::new();
        let request = make_request();
        let mut usage = Usage::default();
        let hooks = HooksConfig::default();
        let security = SecurityConfig::default();
        let mut events_buf = Vec::<AgentRunEvent>::new();
        let mut events = |e: AgentRunEvent| events_buf.push(e);
        let project_root = std::path::PathBuf::from(".");
        // cap=2 → 2nd identical failure triggers Stop
        let mut policies: Vec<Box<dyn LoopPolicy>> = vec![Box::new(AutoFixLoopPolicy::new(2))];

        // First failure: increments attempts, returns Continue + reminder.
        let failure_outcome = outcome("verify_build", false);
        let first = {
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
                provider: &NoopProvider,
                tool_dispatcher: &NoopToolDispatcher,
                subagent_runner: None,
                pending_resume_path: None,
            };
            dispatch_post_turn(&mut policies, &mut cx, &failure_outcome)
                .await
                .unwrap()
        };
        assert!(
            matches!(first, Decision::Continue),
            "first failure should keep loop going, got {first:?}"
        );

        // Second identical failure: hits cap, returns Stop.
        let second = {
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
                provider: &NoopProvider,
                tool_dispatcher: &NoopToolDispatcher,
                subagent_runner: None,
                pending_resume_path: None,
            };
            dispatch_post_turn(&mut policies, &mut cx, &failure_outcome)
                .await
                .unwrap()
        };
        match second {
            Decision::Stop(crate::requests::StopReason::Interrupted, Some(msg)) => {
                assert!(msg.contains("Auto-fix loop circuit breaker"));
                assert!(msg.contains("verify_build"));
            }
            other => panic!("expected Stop on 2nd failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn auto_fix_resets_on_success_after_prior_failure() {
        let mut state = AgentState::new(ExecutionMode::Execute, PermissionMode::Auto);
        let mut context = ContextManager::new();
        let request = make_request();
        let mut usage = Usage::default();
        let hooks = HooksConfig::default();
        let security = SecurityConfig::default();
        let mut events_buf = Vec::<AgentRunEvent>::new();
        let mut events = |e: AgentRunEvent| events_buf.push(e);
        let project_root = std::path::PathBuf::from(".");
        let mut policies: Vec<Box<dyn LoopPolicy>> = vec![Box::new(AutoFixLoopPolicy::new(3))];

        let failure_outcome = outcome("verify_build", false);
        let success_outcome = outcome("verify_build", true);

        // 1: failure → Continue
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
                provider: &NoopProvider,
                tool_dispatcher: &NoopToolDispatcher,
                subagent_runner: None,
                pending_resume_path: None,
            };
            dispatch_post_turn(&mut policies, &mut cx, &failure_outcome)
                .await
                .unwrap();
        }
        // 2: success → state reset, fires AutoFixAttempt {passed:true}
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
                provider: &NoopProvider,
                tool_dispatcher: &NoopToolDispatcher,
                subagent_runner: None,
                pending_resume_path: None,
            };
            dispatch_post_turn(&mut policies, &mut cx, &success_outcome)
                .await
                .unwrap();
        }
        // 3: new failure → Continue again (counter was reset)
        let third = {
            let mut cx = PolicyCx {
                state: &mut state,
                context: &mut context,
                events: &mut events,
                usage: &mut usage,
                request: &request,
                turn_index: 2,
                project_root: &project_root,
                hooks: &hooks,
                security: &security,
                provider: &NoopProvider,
                tool_dispatcher: &NoopToolDispatcher,
                subagent_runner: None,
                pending_resume_path: None,
            };
            dispatch_post_turn(&mut policies, &mut cx, &failure_outcome)
                .await
                .unwrap()
        };
        assert!(
            matches!(third, Decision::Continue),
            "post-success failure should be a fresh attempt, not at cap"
        );
    }

    #[test]
    fn non_verify_tool_is_a_passthrough() {
        // A `file_patch` outcome is not a verify_* tool. The policy
        // must not record it as a failure even if it succeeded — it
        // only tracks verify_* attempts.
        use tokio::runtime::Builder;
        let rt = Builder::new_current_thread().enable_all().build().unwrap();
        let decision = rt.block_on(async {
            let mut state = AgentState::new(ExecutionMode::Execute, PermissionMode::Auto);
            let mut context = ContextManager::new();
            let request = make_request();
            let mut usage = Usage::default();
            let hooks = HooksConfig::default();
            let security = SecurityConfig::default();
            let mut events_buf = Vec::<AgentRunEvent>::new();
            let mut events = |e: AgentRunEvent| events_buf.push(e);
            let project_root = std::path::PathBuf::from(".");
            let mut policies: Vec<Box<dyn LoopPolicy>> = vec![Box::new(AutoFixLoopPolicy::new(2))];
            let outcome = outcome("file_patch", true);
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
                provider: &NoopProvider,
                tool_dispatcher: &NoopToolDispatcher,
                subagent_runner: None,
                pending_resume_path: None,
            };
            dispatch_post_turn(&mut policies, &mut cx, &outcome)
                .await
                .unwrap()
        });
        assert!(matches!(decision, Decision::Continue));
    }
}
