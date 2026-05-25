// Until later PRs wire every dispatch hook into the run loop, a couple of
// dispatchers are only exercised by unit tests. Keep them available without
// warnings.
#![allow(dead_code)]

//! Composable run-loop policies.
//!
//! The harness's main run loop has historically interleaved a dozen
//! cross-cutting concerns inline — pending-approval resume, codebase-survey
//! prefetch, error recovery, auto-verify, sub-agent review, budget gating,
//! stuck detection, goal checking, auto-grade. Each one is a "policy" with
//! its own state, its own activation phase, and its own decision about
//! whether the loop should continue, retry, skip, or stop.
//!
//! The [`LoopPolicy`] trait gives every policy four lifecycle hooks
//! (`pre_turn`, `post_turn`, `on_turn_error`, `on_done`). The driver loop
//! calls the matching dispatcher at each phase boundary; policies in
//! priority order each get a chance to react. The first non-[`Decision::Continue`]
//! verdict short-circuits dispatch and steers the driver.
//!
//! Why not a tower-style `next` continuation? Because the responsibilities
//! aren't a middleware stack — they're cross-cutting hooks firing at
//! distinct phase boundaries. A single-method `call(cx) -> Decision` would
//! force every policy to inspect a tagged enum of "what just happened" —
//! that's the same massive `match` in disguise.
//!
//! Policies own their per-instance state (e.g., `attempts: usize`,
//! `last_signature: Option<String>`). Shared mutable state ([`AgentState`],
//! [`ContextManager`]) is borrowed once per dispatch through [`PolicyCx`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use peridot_agents::SubAgent;
use peridot_common::{
    AgentPhase, ExecutionMode, HooksConfig, PeriResult, PermissionMode, SecurityConfig, ToolCall,
    ToolResult,
};
use peridot_context::ContextManager;
use peridot_llm::Usage;

use crate::requests::{
    AgentRunEvent, AgentRunRequest, AgentTurnOutcome, FileDiffPayload, StopReason,
};
use crate::state::AgentState;

/// Abstraction over tool execution that policies can consume without
/// having to re-borrow `HarnessAgent` directly. Concrete impl is
/// `HarnessToolDispatcher` (in `agent.rs`), built per-iteration by
/// `HarnessAgent::build_tool_dispatcher()` so the snapshot of Arcs and
/// the parent-context mission packet stays consistent with the
/// surrounding turn.
///
/// State that varies turn-by-turn (`mode`, `phase`, `permission`,
/// project root, denied paths, hooks, security) is threaded as
/// parameters rather than fields so policies always pass *fresh* values
/// from `PolicyCx`. The dispatcher's own fields are immutable for the
/// duration of one policy dispatch — they're rebuilt on the next loop
/// iteration.
#[async_trait::async_trait]
pub trait ToolDispatcher: Send + Sync {
    /// Execute one tool call. Mirrors
    /// `HarnessAgent::execute_tool_call_with_runtime` but takes the
    /// state values as parameters (no `&self.state` access required).
    ///
    /// The argument list intentionally mirrors the runtime parameters
    /// the harness already threads through — bundling them into a
    /// struct just to soothe the lint would force every call site to
    /// rebuild that struct from the same `PolicyCx` fields.
    #[allow(clippy::too_many_arguments)]
    async fn execute(
        &self,
        call: ToolCall,
        mode: ExecutionMode,
        phase: AgentPhase,
        permission: PermissionMode,
        project_root: PathBuf,
        denied_paths: Vec<PathBuf>,
        hooks: HooksConfig,
        security: SecurityConfig,
    ) -> PeriResult<(ToolResult, Option<FileDiffPayload>)>;
}

pub mod auto_fix;
pub mod auto_grade;
pub mod auto_verify;
pub mod budget;
pub mod codebase_survey;
pub mod goal_check;
pub mod pending_resume;
pub mod preflight;
pub mod recovery;
pub mod stuck;
pub mod sub_agent_review;

pub use auto_fix::AutoFixLoopPolicy;
pub use auto_grade::{AutoGradePolicy, DiffProvider};
pub use auto_verify::AutoVerifyAfterMutationPolicy;
pub use budget::BudgetWarningPolicy;
pub use codebase_survey::CodebaseSurveyPrefetchPolicy;
pub use goal_check::GoalCheckerPolicy;
pub use pending_resume::PendingResumePolicy;
pub use preflight::PreflightPolicy;
pub use recovery::ErrorRecoveryPolicy;
pub use stuck::StuckDetectorPolicy;
pub use sub_agent_review::SubAgentReviewPolicy;

/// Verdict a [`LoopPolicy`] returns from any of its lifecycle hooks.
///
/// The driver loop interprets these as steering signals:
///
/// - [`Decision::Continue`] — proceed to the next phase / next policy. This
///   is the no-op default; policies that don't care about the current
///   phase return it via the default trait body.
/// - [`Decision::SkipTurn`] — re-enter the top of the loop. Used by
///   `on_done` policies (goal checker, grader) that want the loop to
///   continue running rather than terminate even though the model said
///   it was done.
/// - [`Decision::Retry`] — re-execute the current turn. Only valid from
///   `on_turn_error`; the recovery policy uses this after appending a
///   reminder to the context.
/// - [`Decision::Stop`] — finish the run with the given stop reason. The
///   optional message is forwarded as the failure summary when the reason
///   is [`StopReason::Interrupted`] etc.
#[derive(Clone, Debug)]
pub enum Decision {
    /// Proceed to the next phase or the next policy in priority order.
    Continue,
    /// Re-enter the top of the loop (continue to the next turn iteration
    /// without terminating the run).
    SkipTurn,
    /// Stop the run with the given reason and optional message.
    Stop(StopReason, Option<String>),
    /// Re-execute the current turn (only valid from `on_turn_error`).
    Retry,
}

/// Mutable context passed to every [`LoopPolicy`] hook.
///
/// Constructed by the driver once per dispatch and dropped before any
/// inline driver code runs again, so policies can hold `&mut` borrows
/// without conflicting with the surrounding loop body.
///
/// Why one big borrow struct instead of `&mut self` on `HarnessAgent`?
/// Because policies need access to subfields of the harness independently
/// (state vs. context vs. events) while the harness itself is already
/// `&mut` borrowed by the running `run_until_done_with_events` call.
/// Passing a constructed `PolicyCx` is the cleanest way to thread those
/// disjoint borrows through.
pub struct PolicyCx<'a> {
    /// Mutable handle to the agent's coarse-grained state (phase,
    /// permission mode, goal, etc.).
    pub state: &'a mut AgentState,
    /// Mutable handle to the running context (transcript, evidence
    /// ledger, plan reminders).
    pub context: &'a mut ContextManager,
    /// Callback for emitting [`AgentRunEvent`]s. Polled through a
    /// `&mut dyn FnMut + Send` so the dispatcher doesn't need a generic
    /// parameter per policy — the driver's outer `F: FnMut + Send` adapts
    /// to the trait object at the dispatch boundary. `Send` so the
    /// surrounding policy future stays `Send`-clean.
    pub events: &'a mut (dyn FnMut(AgentRunEvent) + Send),
    /// Aggregated run usage. Mutated by policies that account for
    /// out-of-band LLM calls (goal checker, grader).
    pub usage: &'a mut Usage,
    /// The run request the loop is executing. Read-only for policies.
    pub request: &'a AgentRunRequest,
    /// Zero-based turn index (0 == very first turn of the run).
    pub turn_index: usize,
    /// Project root. Pulled out of `request` for convenience because
    /// most policies need it.
    pub project_root: &'a Path,
    /// Hook configuration. Same — convenience handle into `request`.
    pub hooks: &'a HooksConfig,
    /// Security configuration. Same — convenience handle into `request`.
    pub security: &'a SecurityConfig,
    /// LLM provider. Used by `on_done`-time gates (goal-checker,
    /// auto-grader) that issue out-of-band model completions. The
    /// driver constructs this directly from its `&dyn LlmProvider`
    /// parameter, so no unsized coercion is needed at the field-
    /// assignment site.
    pub provider: &'a dyn peridot_llm::LlmProvider,
    /// Tool execution abstraction. Built per-loop-iteration by
    /// `HarnessAgent::build_tool_dispatcher()` so its Arcs + parent
    /// mission packet stay consistent with the current turn. Policies
    /// that need to invoke tools (`PendingResumePolicy`,
    /// `AutoVerifyAfterMutationPolicy`) use this to dispatch without
    /// re-borrowing `&mut HarnessAgent`.
    pub tool_dispatcher: &'a dyn ToolDispatcher,
    /// Sub-agent runner snapshot. Same construction story as
    /// `tool_dispatcher`. `None` when the harness has no runner
    /// configured (e.g., in unit tests).
    pub subagent_runner: Option<Arc<dyn SubAgent>>,
    /// Optional override path for the pending-resume sidecar.
    /// `None` means use the default workspace-derived location.
    /// `PendingResumePolicy` reads this when looking up the sidecar.
    pub pending_resume_path: Option<&'a Path>,
}

/// Lifecycle hooks a policy can implement.
///
/// All four default to no-op `Ok(Decision::Continue)` so a policy only
/// overrides the phases it cares about. The dispatcher functions below
/// run policies in registration order; the first non-`Continue` decision
/// wins.
///
/// Policy futures must be `Send` so the entire run loop can be driven
/// from a `Send` future (the inner-loop sub-agent's `SubAgent::run` impl
/// requires it). The event-sink closure inside [`PolicyCx`] is correspondingly
/// `Send`-bounded; callers of [`crate::HarnessAgent::run_until_done_with_events`]
/// already pass `Send` closures.
#[async_trait::async_trait]
pub trait LoopPolicy: Send + Sync {
    /// Short stable label for telemetry and error messages
    /// (e.g., `"error_recovery"`, `"auto_verify"`).
    fn name(&self) -> &'static str;

    /// Fires before any LLM call this turn. Used by one-shot policies
    /// (pending-approval resume, codebase-survey prefetch) that need
    /// to inject context before the model sees the first prompt.
    async fn pre_turn(&mut self, _cx: &mut PolicyCx<'_>) -> PeriResult<Decision> {
        Ok(Decision::Continue)
    }

    /// Fires after a successful turn produces an outcome. Used by
    /// policies that observe model output (auto-verify after mutation,
    /// sub-agent review injection, budget warning, stuck detector).
    async fn post_turn(
        &mut self,
        _cx: &mut PolicyCx<'_>,
        _outcome: &AgentTurnOutcome,
    ) -> PeriResult<Decision> {
        Ok(Decision::Continue)
    }

    /// Fires when the turn returned [`peridot_common::PeriError`]. The
    /// recovery policy is the primary consumer; it returns
    /// [`Decision::Retry`] to re-run the turn after appending a reminder,
    /// or [`Decision::Stop`] to abort the run.
    async fn on_turn_error(
        &mut self,
        _cx: &mut PolicyCx<'_>,
        _err: &peridot_common::PeriError,
    ) -> PeriResult<Decision> {
        Ok(Decision::Continue)
    }

    /// Fires once when the most recent outcome's `done` field is true.
    /// Policies can veto the done state and force the loop to continue
    /// (via [`Decision::SkipTurn`]) — the deterministic preflight check,
    /// goal checker, and auto-grader all use this hook.
    ///
    /// LLM-issuing done-gates read the provider from
    /// [`PolicyCx::provider`]. Trivial deterministic gates ignore it.
    async fn on_done(
        &mut self,
        _cx: &mut PolicyCx<'_>,
        _outcomes: &[AgentTurnOutcome],
    ) -> PeriResult<Decision> {
        Ok(Decision::Continue)
    }
}

/// Runs the `pre_turn` hook on every policy in order. Returns the first
/// non-[`Decision::Continue`] verdict, or `Continue` if every policy
/// agreed.
pub async fn dispatch_pre_turn(
    policies: &mut [Box<dyn LoopPolicy>],
    cx: &mut PolicyCx<'_>,
) -> PeriResult<Decision> {
    for policy in policies.iter_mut() {
        match policy.pre_turn(cx).await? {
            Decision::Continue => {}
            other => return Ok(other),
        }
    }
    Ok(Decision::Continue)
}

/// Runs the `post_turn` hook on every policy in order.
pub async fn dispatch_post_turn(
    policies: &mut [Box<dyn LoopPolicy>],
    cx: &mut PolicyCx<'_>,
    outcome: &AgentTurnOutcome,
) -> PeriResult<Decision> {
    for policy in policies.iter_mut() {
        match policy.post_turn(cx, outcome).await? {
            Decision::Continue => {}
            other => return Ok(other),
        }
    }
    Ok(Decision::Continue)
}

/// Runs the `on_turn_error` hook on every policy in order.
pub async fn dispatch_on_turn_error(
    policies: &mut [Box<dyn LoopPolicy>],
    cx: &mut PolicyCx<'_>,
    err: &peridot_common::PeriError,
) -> PeriResult<Decision> {
    for policy in policies.iter_mut() {
        match policy.on_turn_error(cx, err).await? {
            Decision::Continue => {}
            other => return Ok(other),
        }
    }
    Ok(Decision::Continue)
}

/// Runs the `on_done` hook on every policy in order.
pub async fn dispatch_on_done(
    policies: &mut [Box<dyn LoopPolicy>],
    cx: &mut PolicyCx<'_>,
    outcomes: &[AgentTurnOutcome],
) -> PeriResult<Decision> {
    for policy in policies.iter_mut() {
        match policy.on_done(cx, outcomes).await? {
            Decision::Continue => {}
            other => return Ok(other),
        }
    }
    Ok(Decision::Continue)
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Helpers shared by policy unit tests across the `loop_policy/`
    //! submodules. Lives behind `#[cfg(test)]` so it never reaches the
    //! production build.
    use peridot_common::PeriResult;
    use peridot_llm::{CompletionRequest, CompletionResponse, LlmProvider, PricingTable};

    /// LLM provider that panics on every call. Used to fill the
    /// `provider` field of `PolicyCx` in tests for policies that
    /// don't actually issue LLM completions — the field has to be
    /// populated, but it's never read.
    pub(crate) struct NoopProvider;

    #[async_trait::async_trait]
    impl LlmProvider for NoopProvider {
        async fn complete(&self, _request: CompletionRequest) -> PeriResult<CompletionResponse> {
            panic!("NoopProvider::complete called — tests should not exercise the provider")
        }
        fn supports_cache(&self) -> bool {
            false
        }
        fn supports_prefill(&self) -> bool {
            false
        }
        fn supports_thinking(&self) -> bool {
            false
        }
        fn pricing(&self) -> PricingTable {
            PricingTable::default()
        }
        fn auth_method(&self) -> peridot_llm::AuthMethod {
            peridot_llm::AuthMethod::NotConfigured
        }
    }

    /// Tool dispatcher that panics on call. Used in policy unit tests
    /// where `PolicyCx::tool_dispatcher` must be a real `&dyn`
    /// reference but the policy under test never actually invokes it.
    pub(crate) struct NoopToolDispatcher;

    #[async_trait::async_trait]
    impl crate::loop_policy::ToolDispatcher for NoopToolDispatcher {
        async fn execute(
            &self,
            _call: peridot_common::ToolCall,
            _mode: peridot_common::ExecutionMode,
            _phase: peridot_common::AgentPhase,
            _permission: peridot_common::PermissionMode,
            _project_root: std::path::PathBuf,
            _denied_paths: Vec<std::path::PathBuf>,
            _hooks: peridot_common::HooksConfig,
            _security: peridot_common::SecurityConfig,
        ) -> PeriResult<(
            peridot_common::ToolResult,
            Option<crate::requests::FileDiffPayload>,
        )> {
            panic!("NoopToolDispatcher::execute called — test should not exercise it")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peridot_common::{ExecutionMode, PermissionMode};

    /// Policy that records which hooks fired in which order so tests can
    /// assert on dispatch ordering and the no-op default.
    struct Recorder {
        name: &'static str,
        calls: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        pre: Decision,
        post: Decision,
    }

    #[async_trait::async_trait]
    impl LoopPolicy for Recorder {
        fn name(&self) -> &'static str {
            self.name
        }
        async fn pre_turn(&mut self, _cx: &mut PolicyCx<'_>) -> PeriResult<Decision> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("{}.pre", self.name));
            Ok(self.pre.clone())
        }
        async fn post_turn(
            &mut self,
            _cx: &mut PolicyCx<'_>,
            _o: &AgentTurnOutcome,
        ) -> PeriResult<Decision> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("{}.post", self.name));
            Ok(self.post.clone())
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

    #[tokio::test]
    async fn dispatcher_short_circuits_on_first_non_continue() {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let mut policies: Vec<Box<dyn LoopPolicy>> = vec![
            Box::new(Recorder {
                name: "first",
                calls: calls.clone(),
                pre: Decision::Continue,
                post: Decision::Continue,
            }),
            Box::new(Recorder {
                name: "second",
                calls: calls.clone(),
                pre: Decision::SkipTurn,
                post: Decision::Continue,
            }),
            // Third should never run because second returned SkipTurn.
            Box::new(Recorder {
                name: "third",
                calls: calls.clone(),
                pre: Decision::Continue,
                post: Decision::Continue,
            }),
        ];
        let mut state = AgentState::new(ExecutionMode::Plan, PermissionMode::Safe);
        let mut context = ContextManager::new();
        let request = make_request();
        let mut usage = Usage::default();
        let hooks = HooksConfig::default();
        let security = SecurityConfig::default();
        let mut events_buf = Vec::new();
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

        let decision = dispatch_pre_turn(&mut policies, &mut cx).await.unwrap();
        assert!(matches!(decision, Decision::SkipTurn));
        let recorded = calls.lock().unwrap().clone();
        assert_eq!(
            recorded,
            vec!["first.pre".to_string(), "second.pre".to_string()]
        );
    }

    #[tokio::test]
    async fn empty_policy_list_returns_continue() {
        let mut policies: Vec<Box<dyn LoopPolicy>> = Vec::new();
        let mut state = AgentState::new(ExecutionMode::Plan, PermissionMode::Safe);
        let mut context = ContextManager::new();
        let request = make_request();
        let mut usage = Usage::default();
        let hooks = HooksConfig::default();
        let security = SecurityConfig::default();
        let mut events_buf = Vec::new();
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

        let pre = dispatch_pre_turn(&mut policies, &mut cx).await.unwrap();
        assert!(matches!(pre, Decision::Continue));
    }
}
