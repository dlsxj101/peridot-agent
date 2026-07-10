//! Auto-verify-after-mutation policy.
//!
//! When `auto_verify_after_mutation` is enabled and a mutating tool
//! (file_write / file_patch) succeeds, this policy does *not* verify
//! immediately — it marks the run "dirty". The verify fires once, when
//! the mutation burst settles: on the first following non-mutation turn
//! (any tool, including `agent_done`). A burst of five edits therefore
//! costs one `verify_build`, not five.
//!
//! Coordination with the model and the circuit breaker:
//! - If the model runs a `verify_*` tool itself, that clears the dirty
//!   flag and this policy stays out of the way — [`AutoFixLoopPolicy`]
//!   owns the failure accounting for model-driven verifies, so we never
//!   double-count.
//! - When *this* policy runs the verify and it fails, the failure is
//!   folded into the same [`VerifyFailureState`] machinery the auto-fix
//!   loop uses (shared helpers). `auto_fix.max_attempts` consecutive
//!   identical failures abort the run with [`StopReason::Interrupted`],
//!   exactly like a model-driven failure would.
//!
//! The verify command is resolved with this precedence: explicit
//! `auto_fix.commands` (joined with ` && `) > project detection
//! (AGENTS.md `## commands` > scanner). When nothing resolves, the
//! `verify_build` tool returns a *skip*, recorded as a neutral
//! `[auto-verify] skipped` note that never blocks `agent_done`.

use peridot_common::{AgentPhase, PeriResult, ToolResult};
use peridot_context::{ContextEntry, ContextSource};
use peridot_llm::Usage;

use crate::agent::transition_phase;
use crate::agent_helpers::is_mutating_tool_name;
use crate::loop_policy::{Decision, LoopPolicy, PolicyCx};
use crate::recovery::run_recovery_event_hook;
use crate::requests::{AgentRunEvent, AgentTurnOutcome, StopReason};
use crate::verify_failure::{
    VerifyFailureState, update_verify_failure_state, verify_failure_directive,
};

/// Debounced auto-verify. Owns the "dirty" burst flag plus its own
/// [`VerifyFailureState`] so the circuit breaker can trip on repeated
/// auto-verify failures independently of the model-driven auto-fix loop.
pub struct AutoVerifyAfterMutationPolicy {
    enabled: bool,
    /// Consecutive-identical-failure cap before aborting. Mirrors
    /// `auto_fix.max_attempts`.
    fix_cap: u32,
    /// When false (operator disabled `auto_fix`), auto-verify still runs
    /// and records pass/fail markers for the done gate, but never counts
    /// failures, injects a fix directive, or trips the circuit breaker.
    circuit_breaker_enabled: bool,
    /// Explicit verify commands from `auto_fix.commands`. When non-empty
    /// they override project detection and are passed straight to
    /// `verify_build` as a single ` && `-joined command line.
    commands: Vec<String>,
    /// A mutating tool has succeeded since the last verify flush.
    dirty: bool,
    /// Rolling failure signature for the circuit breaker.
    failure_state: Option<VerifyFailureState>,
}

impl AutoVerifyAfterMutationPolicy {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            fix_cap: 3,
            circuit_breaker_enabled: true,
            commands: Vec::new(),
            dirty: false,
            failure_state: None,
        }
    }

    /// Sets the consecutive-failure cap (from `auto_fix.max_attempts`).
    pub fn with_fix_cap(mut self, cap: u32) -> Self {
        self.fix_cap = cap.max(1);
        self
    }

    /// Enables/disables circuit-breaker counting (from `auto_fix.enabled`).
    pub fn with_circuit_breaker(mut self, enabled: bool) -> Self {
        self.circuit_breaker_enabled = enabled;
        self
    }

    /// Sets explicit verify commands (from `auto_fix.commands`).
    pub fn with_commands(mut self, commands: Vec<String>) -> Self {
        self.commands = commands;
        self
    }

    /// Parameters handed to `verify_build`. Explicit `auto_fix.commands`
    /// win over the tool's own project detection.
    fn verify_params(&self) -> serde_json::Value {
        if self.commands.is_empty() {
            serde_json::json!({})
        } else {
            serde_json::json!({ "command": self.commands.join(" && ") })
        }
    }

    /// Folds a completed verify result into context and, on failure,
    /// into the circuit breaker.
    fn handle_verify_result(
        &mut self,
        cx: &mut PolicyCx<'_>,
        result: ToolResult,
    ) -> PeriResult<Decision> {
        // Skip: no command resolved. Neither pass nor fail — never blocks
        // done, never counts against the breaker.
        let skipped = result
            .output
            .get("skipped")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if skipped {
            self.failure_state = None;
            cx.context.append(ContextEntry::trusted(
                ContextSource::PlanReminder,
                format!("[auto-verify] skipped: {}", result.summary),
            ));
            return Ok(Decision::Continue);
        }

        if result.success {
            self.failure_state = None;
            cx.context.append(ContextEntry::trusted(
                ContextSource::PlanReminder,
                format!("[auto-verify] verify_build passed: {}", result.summary),
            ));
            return Ok(Decision::Continue);
        }

        // Failure. Always leave a FAILED marker so the done gate blocks.
        cx.context.append(ContextEntry::trusted(
            ContextSource::PlanReminder,
            format!(
                "[auto-verify] verify_build FAILED: {}\nFix this before declaring agent_done.",
                result.summary
            ),
        ));

        if !self.circuit_breaker_enabled {
            return Ok(Decision::Continue);
        }

        let synthetic = AgentTurnOutcome {
            tool_name: "verify_build".to_string(),
            tool_result: result,
            usage: Usage::default(),
            done: false,
        };
        let failure = update_verify_failure_state(&mut self.failure_state, &synthetic).clone();
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
        Ok(Decision::Continue)
    }
}

#[async_trait::async_trait]
impl LoopPolicy for AutoVerifyAfterMutationPolicy {
    fn name(&self) -> &'static str {
        "auto_verify_after_mutation"
    }

    async fn post_turn(
        &mut self,
        cx: &mut PolicyCx<'_>,
        outcome: &AgentTurnOutcome,
    ) -> PeriResult<Decision> {
        if !self.enabled {
            return Ok(Decision::Continue);
        }

        // A successful mutation only marks the burst dirty — the verify
        // is deferred until the burst settles.
        if outcome.tool_result.success && is_mutating_tool_name(&outcome.tool_name) {
            self.dirty = true;
            return Ok(Decision::Continue);
        }

        // The model ran a verify itself: it owns this verification. Clear
        // dirty and defer failure accounting to AutoFixLoopPolicy so we
        // never double-count. A green model verify also resets our
        // rolling failure signature.
        if outcome.tool_name.starts_with("verify_") {
            self.dirty = false;
            if outcome.tool_result.success {
                self.failure_state = None;
            }
            return Ok(Decision::Continue);
        }

        // Nothing pending → nothing to do.
        if !self.dirty {
            return Ok(Decision::Continue);
        }

        // Flush the burst with a single verify.
        self.dirty = false;
        let exec = cx
            .tool_dispatcher
            .execute(
                peridot_common::ToolCall {
                    name: "verify_build".to_string(),
                    parameters: self.verify_params(),
                },
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
            Ok((result, _file_diff)) => self.handle_verify_result(cx, result),
            Err(err) => {
                // Infra error (couldn't even launch the command): a quiet
                // note, never a block or a breaker tick.
                cx.context.append(ContextEntry::trusted(
                    ContextSource::PlanReminder,
                    format!("[auto-verify] verify_build could not run: {err}"),
                ));
                Ok(Decision::Continue)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_policy::test_support::NoopProvider;
    use crate::loop_policy::{LoopPolicy, PolicyCx, ToolDispatcher};
    use crate::requests::AgentRunRequest;
    use crate::state::AgentState;
    use peridot_common::{
        AgentPhase, ExecutionMode, HooksConfig, PermissionMode, SecurityConfig, ToolCall,
    };
    use peridot_context::{ContextManager, ContextSource};
    use peridot_llm::Usage;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// Mock dispatcher that returns a fixed verify_build result and
    /// records every call. Used to assert what AutoVerify dispatches
    /// without spinning up a real ToolRegistry.
    struct RecordingDispatcher {
        calls: Mutex<Vec<ToolCall>>,
        result: peridot_common::ToolResult,
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
        ) -> peridot_common::PeriResult<(
            peridot_common::ToolResult,
            Option<crate::requests::FileDiffPayload>,
        )> {
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

    fn make_outcome(tool_name: &str, success: bool) -> AgentTurnOutcome {
        AgentTurnOutcome {
            tool_name: tool_name.to_string(),
            tool_result: if success {
                peridot_common::ToolResult::success(tool_name, serde_json::Value::Null)
            } else {
                peridot_common::ToolResult::failure(tool_name)
            },
            usage: Usage::default(),
            done: false,
        }
    }

    fn skip_result() -> peridot_common::ToolResult {
        peridot_common::ToolResult {
            success: true,
            summary: "no build command detected for this project".to_string(),
            output: serde_json::json!({"skipped": true, "reason": "no build command detected"}),
        }
    }

    /// Runs a single `post_turn` on a persistent policy instance so
    /// stateful tests (debounce, circuit breaker) can drive a sequence
    /// of turns while sharing `context`.
    async fn run_one(
        policy: &mut AutoVerifyAfterMutationPolicy,
        outcome: &AgentTurnOutcome,
        dispatcher: &RecordingDispatcher,
        context: &mut ContextManager,
    ) -> Decision {
        let mut state = AgentState::new(ExecutionMode::Execute, PermissionMode::Auto);
        let request = make_request();
        let mut usage = Usage::default();
        let hooks = HooksConfig::default();
        let security = SecurityConfig::default();
        let mut events = |_e: crate::requests::AgentRunEvent| {};
        let project_root = std::path::PathBuf::from(".");
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
            provider: &NoopProvider,
            tool_dispatcher: dispatcher,
            subagent_runner: None,
            pending_resume_path: None,
        };
        policy.post_turn(&mut cx, outcome).await.unwrap()
    }

    #[tokio::test]
    async fn burst_of_mutations_runs_verify_once_on_settle() {
        // Three consecutive mutations must NOT each trigger a verify;
        // the single flush happens on the following non-mutation turn.
        let dispatcher = RecordingDispatcher {
            calls: Mutex::new(Vec::new()),
            result: peridot_common::ToolResult::success("build OK", serde_json::Value::Null),
        };
        let mut context = ContextManager::new();
        let mut policy = AutoVerifyAfterMutationPolicy::new(true);

        for _ in 0..3 {
            run_one(
                &mut policy,
                &make_outcome("file_write", true),
                &dispatcher,
                &mut context,
            )
            .await;
        }
        assert!(
            dispatcher.calls.lock().unwrap().is_empty(),
            "mutations alone must defer verify (debounce)"
        );

        // A non-mutation turn settles the burst → exactly one verify.
        run_one(
            &mut policy,
            &make_outcome("file_read", true),
            &dispatcher,
            &mut context,
        )
        .await;
        let calls = dispatcher.calls.lock().unwrap();
        assert_eq!(
            calls.len(),
            1,
            "expected exactly one verify_build for the whole burst"
        );
        assert_eq!(calls[0].name, "verify_build");
        drop(calls);
        assert!(
            context
                .entries()
                .iter()
                .any(|e| e.content.contains("[auto-verify]") && e.content.contains("passed")),
            "expected a passed marker after the flush"
        );
    }

    #[tokio::test]
    async fn model_self_verify_suppresses_auto_verify() {
        // Model mutates then runs verify_build itself → auto-verify must
        // NOT run a second verify; it just clears its dirty flag.
        let dispatcher = RecordingDispatcher {
            calls: Mutex::new(Vec::new()),
            result: peridot_common::ToolResult::success("never run", serde_json::Value::Null),
        };
        let mut context = ContextManager::new();
        let mut policy = AutoVerifyAfterMutationPolicy::new(true);

        run_one(
            &mut policy,
            &make_outcome("file_patch", true),
            &dispatcher,
            &mut context,
        )
        .await;
        run_one(
            &mut policy,
            &make_outcome("verify_build", true),
            &dispatcher,
            &mut context,
        )
        .await;
        assert!(
            dispatcher.calls.lock().unwrap().is_empty(),
            "model's own verify must suppress the auto-verify flush"
        );

        // And the dirty flag is cleared: a later non-mutation turn must
        // not resurrect a verify.
        run_one(
            &mut policy,
            &make_outcome("file_read", true),
            &dispatcher,
            &mut context,
        )
        .await;
        assert!(dispatcher.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn repeated_auto_verify_failure_trips_circuit_breaker() {
        // cap = 3: three mutation→settle cycles with the same failing
        // verify signature must abort with Interrupted on the 3rd.
        let dispatcher = RecordingDispatcher {
            calls: Mutex::new(Vec::new()),
            result: peridot_common::ToolResult::failure("error[E0425]: cannot find value `x`"),
        };
        let mut context = ContextManager::new();
        let mut policy = AutoVerifyAfterMutationPolicy::new(true).with_fix_cap(3);

        let mut decisions = Vec::new();
        for _ in 0..3 {
            run_one(
                &mut policy,
                &make_outcome("file_write", true),
                &dispatcher,
                &mut context,
            )
            .await;
            let decision = run_one(
                &mut policy,
                &make_outcome("file_read", true),
                &dispatcher,
                &mut context,
            )
            .await;
            decisions.push(decision);
        }
        assert!(matches!(decisions[0], Decision::Continue));
        assert!(matches!(decisions[1], Decision::Continue));
        match &decisions[2] {
            Decision::Stop(StopReason::Interrupted, Some(msg)) => {
                assert!(msg.contains("circuit breaker"));
                assert!(msg.contains("verify_build"));
            }
            other => panic!("expected Stop(Interrupted) on 3rd failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn agent_done_turn_flush_trips_breaker_so_done_gate_cannot_loop() {
        // The done gate (Preflight) blocks agent_done while the last
        // mutation's verify keeps failing. This must not loop forever:
        // the agent_done turn itself flushes the deferred verify, so the
        // circuit breaker advances every done attempt and eventually
        // aborts with Interrupted (which the run loop honours before it
        // ever re-enters the done gate).
        let dispatcher = RecordingDispatcher {
            calls: Mutex::new(Vec::new()),
            result: peridot_common::ToolResult::failure("error[E0425]: cannot find value `x`"),
        };
        let mut context = ContextManager::new();
        let mut policy = AutoVerifyAfterMutationPolicy::new(true).with_fix_cap(2);

        // Attempt 1: mutate, then try to finish → flush fails (count 1).
        run_one(
            &mut policy,
            &make_outcome("file_write", true),
            &dispatcher,
            &mut context,
        )
        .await;
        let first_done = run_one(
            &mut policy,
            &make_outcome("agent_done", true),
            &dispatcher,
            &mut context,
        )
        .await;
        assert!(
            matches!(first_done, Decision::Continue),
            "first blocked done should keep looping"
        );

        // Attempt 2: mutate again, retry finish → flush fails (count 2 = cap).
        run_one(
            &mut policy,
            &make_outcome("file_write", true),
            &dispatcher,
            &mut context,
        )
        .await;
        let second_done = run_one(
            &mut policy,
            &make_outcome("agent_done", true),
            &dispatcher,
            &mut context,
        )
        .await;
        assert!(
            matches!(second_done, Decision::Stop(StopReason::Interrupted, _)),
            "repeated failing done flush must abort, not loop; got {second_done:?}"
        );
    }

    #[tokio::test]
    async fn circuit_breaker_disabled_never_aborts() {
        // With auto_fix disabled, repeated failures leave FAILED markers
        // but never trip the breaker.
        let dispatcher = RecordingDispatcher {
            calls: Mutex::new(Vec::new()),
            result: peridot_common::ToolResult::failure("error[E0425]: cannot find value `x`"),
        };
        let mut context = ContextManager::new();
        let mut policy = AutoVerifyAfterMutationPolicy::new(true)
            .with_fix_cap(2)
            .with_circuit_breaker(false);
        for _ in 0..5 {
            run_one(
                &mut policy,
                &make_outcome("file_write", true),
                &dispatcher,
                &mut context,
            )
            .await;
            let decision = run_one(
                &mut policy,
                &make_outcome("file_read", true),
                &dispatcher,
                &mut context,
            )
            .await;
            assert!(
                matches!(decision, Decision::Continue),
                "disabled breaker must never Stop"
            );
        }
    }

    #[tokio::test]
    async fn skip_result_leaves_neutral_marker() {
        let dispatcher = RecordingDispatcher {
            calls: Mutex::new(Vec::new()),
            result: skip_result(),
        };
        let mut context = ContextManager::new();
        let mut policy = AutoVerifyAfterMutationPolicy::new(true);
        run_one(
            &mut policy,
            &make_outcome("file_write", true),
            &dispatcher,
            &mut context,
        )
        .await;
        let decision = run_one(
            &mut policy,
            &make_outcome("agent_done", true),
            &dispatcher,
            &mut context,
        )
        .await;
        assert!(matches!(decision, Decision::Continue));
        assert!(
            context
                .entries()
                .iter()
                .any(|e| e.content.contains("[auto-verify] skipped")),
            "expected a neutral skipped marker"
        );
        assert!(
            !context
                .entries()
                .iter()
                .any(|e| e.content.contains("FAILED") || e.content.contains("passed")),
            "skip is neither pass nor fail"
        );
    }

    #[tokio::test]
    async fn explicit_commands_override_detection() {
        let dispatcher = RecordingDispatcher {
            calls: Mutex::new(Vec::new()),
            result: peridot_common::ToolResult::success("ok", serde_json::Value::Null),
        };
        let mut context = ContextManager::new();
        let mut policy = AutoVerifyAfterMutationPolicy::new(true)
            .with_commands(vec!["cargo check".to_string(), "cargo test".to_string()]);
        run_one(
            &mut policy,
            &make_outcome("file_write", true),
            &dispatcher,
            &mut context,
        )
        .await;
        run_one(
            &mut policy,
            &make_outcome("file_read", true),
            &dispatcher,
            &mut context,
        )
        .await;
        let calls = dispatcher.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].parameters.get("command").and_then(|v| v.as_str()),
            Some("cargo check && cargo test"),
            "auto_fix.commands must be joined with ' && ' and passed to verify_build"
        );
    }

    #[tokio::test]
    async fn appends_failed_marker_when_verify_build_returns_failure() {
        let dispatcher = RecordingDispatcher {
            calls: Mutex::new(Vec::new()),
            result: peridot_common::ToolResult::failure("compile error in src/lib.rs"),
        };
        let mut context = ContextManager::new();
        let mut policy = AutoVerifyAfterMutationPolicy::new(true);
        run_one(
            &mut policy,
            &make_outcome("file_write", true),
            &dispatcher,
            &mut context,
        )
        .await;
        let decision = run_one(
            &mut policy,
            &make_outcome("file_read", true),
            &dispatcher,
            &mut context,
        )
        .await;
        assert!(matches!(decision, Decision::Continue));
        let failed = context.entries().iter().any(|e| {
            e.source == ContextSource::PlanReminder
                && e.content.contains("[auto-verify]")
                && e.content.contains("FAILED")
        });
        assert!(failed, "expected the FAILED marker reminder in context");
    }

    #[tokio::test]
    async fn non_mutation_without_dirty_never_verifies() {
        let dispatcher = RecordingDispatcher {
            calls: Mutex::new(Vec::new()),
            result: peridot_common::ToolResult::success("never run", serde_json::Value::Null),
        };
        let mut context = ContextManager::new();
        let mut policy = AutoVerifyAfterMutationPolicy::new(true);
        run_one(
            &mut policy,
            &make_outcome("file_read", true),
            &dispatcher,
            &mut context,
        )
        .await;
        assert!(
            dispatcher.calls.lock().unwrap().is_empty(),
            "a non-mutation turn with no pending burst must not verify"
        );
    }

    #[tokio::test]
    async fn skips_when_disabled() {
        let dispatcher = RecordingDispatcher {
            calls: Mutex::new(Vec::new()),
            result: peridot_common::ToolResult::success("never run", serde_json::Value::Null),
        };
        let mut context = ContextManager::new();
        let mut policy = AutoVerifyAfterMutationPolicy::new(false);
        run_one(
            &mut policy,
            &make_outcome("file_patch", true),
            &dispatcher,
            &mut context,
        )
        .await;
        run_one(
            &mut policy,
            &make_outcome("file_read", true),
            &dispatcher,
            &mut context,
        )
        .await;
        assert!(dispatcher.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn failed_mutation_does_not_mark_dirty() {
        // A failed mutation didn't change anything → no burst to flush.
        let dispatcher = RecordingDispatcher {
            calls: Mutex::new(Vec::new()),
            result: peridot_common::ToolResult::success("never run", serde_json::Value::Null),
        };
        let mut context = ContextManager::new();
        let mut policy = AutoVerifyAfterMutationPolicy::new(true);
        run_one(
            &mut policy,
            &make_outcome("file_patch", false),
            &dispatcher,
            &mut context,
        )
        .await;
        run_one(
            &mut policy,
            &make_outcome("file_read", true),
            &dispatcher,
            &mut context,
        )
        .await;
        assert!(dispatcher.calls.lock().unwrap().is_empty());
    }
}
