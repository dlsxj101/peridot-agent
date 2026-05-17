use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use peridot_agents::SubAgent;
use peridot_common::{
    AgentPhase, ExecutionMode, PeriError, PeriResult, SecurityConfig, ToolCall, ToolResult,
};
use peridot_context::{ContextEntry, ContextManager, ContextSource};
use peridot_llm::{
    CompletionRequest, LlmProvider, ToolChoice, ToolDefinition, ToolInvocation, Usage,
};
use peridot_tools::audit::{AuditEvent, append_audit_event};
use peridot_tools::hooks::{HookRunner, tool_hook_variables};
use peridot_tools::{ToolContext, ToolRegistry};

use peridot_common::CancelToken;
use crate::goal::check_goal_satisfied;
use crate::permissions::ensure_tool_allowed;
use crate::prompt::{read_plan_reminder, system_prompt_for_role};
use crate::recovery::{
    StuckAction, StuckDetector, budget_exceeded_message, budget_warning_message, classify_error,
    format_reminder_message, recovery_message, run_budget_warning_hook, run_context_compacted_hook,
    run_error_event_hooks, run_recovery_event_hook, should_emit_budget_warning,
};
use crate::requests::{
    AgentRunEvent, AgentRunRequest, AgentRunSummary, AgentTurnOutcome, AgentTurnRequest, StopReason,
};
use crate::role::AgentRole;
use crate::state::AgentState;
use crate::usage::{accumulate_usage, stream_completion_with_chunks};

/// Peridot harness agent shell.
pub struct HarnessAgent {
    state: AgentState,
    context: ContextManager,
    tools: ToolRegistry,
    cancel: Option<CancelToken>,
    context_snapshot_path: Option<PathBuf>,
    agents_md_path: Option<PathBuf>,
    agents_md_signature: Option<(u64, u64)>,
    role: AgentRole,
    subagent_runner: Option<Arc<dyn SubAgent>>,
    auto_verify_after_mutation: bool,
    auto_grade_on_done: bool,
    /// Optional flag the operator sets via `/compact` to force an LLM
    /// recap on the next turn boundary, even when the buffer is well
    /// below the auto trigger. Atomic so the slash command thread and
    /// the agent loop can share it without locking.
    compact_request: Option<Arc<AtomicBool>>,
}

impl HarnessAgent {
    /// Creates a harness agent from state and dependencies.
    pub fn new(state: AgentState, context: ContextManager, tools: ToolRegistry) -> Self {
        Self {
            state,
            context,
            tools,
            cancel: None,
            context_snapshot_path: None,
            agents_md_path: None,
            agents_md_signature: None,
            role: AgentRole::default(),
            subagent_runner: None,
            auto_verify_after_mutation: false,
            auto_grade_on_done: false,
            compact_request: None,
        }
    }

    /// Attaches a shared atomic flag the operator can set (e.g. via
    /// `/compact`) to force an LLM recap on the next turn boundary.
    pub fn set_compact_request(&mut self, flag: Arc<AtomicBool>) {
        self.compact_request = Some(flag);
    }

    /// Installs a subagent runner. The harness injects this into every
    /// `ToolContext` it builds so `agent_delegate` dispatches through it
    /// (typically an `InnerLoopSubAgent` running a bounded child harness)
    /// instead of only preparing a workspace.
    pub fn set_subagent_runner(&mut self, runner: Arc<dyn SubAgent>) {
        self.subagent_runner = Some(runner);
    }

    /// Enables the "verify after every mutation" auto-loop. When on,
    /// `verify_build` runs automatically after every successful
    /// `file_write` / `file_patch` / `shell_exec` and its result is
    /// injected into context as a `PlanReminder`, so the next model
    /// turn sees compile errors immediately. Off by default.
    pub fn set_auto_verify_after_mutation(&mut self, enabled: bool) {
        self.auto_verify_after_mutation = enabled;
    }

    /// Enables LLM-based grading on `agent_done`. When the verdict
    /// fails, the recommendations are folded back into context and
    /// the loop continues for another turn instead of finishing.
    /// Off by default.
    pub fn set_auto_grade_on_done(&mut self, enabled: bool) {
        self.auto_grade_on_done = enabled;
    }

    /// Assigns the committee role this agent plays. Defaults to
    /// `AgentRole::Executor`, which keeps the legacy single-agent behaviour.
    pub fn set_role(&mut self, role: AgentRole) {
        self.role = role;
    }

    /// Returns the committee role this agent is configured to play.
    pub fn role(&self) -> AgentRole {
        self.role
    }

    /// Configures the AGENTS.md file the agent loop watches for changes.
    /// On every turn the loop compares the file's `(modified_unix, len)`
    /// fingerprint to the last seen one and, if it changed, re-reads the
    /// file and pushes its contents into context as a `PlanReminder` entry
    /// while emitting `AgentRunEvent::AgentsMdLoaded`.
    pub fn set_agents_md_path(&mut self, path: PathBuf) {
        self.agents_md_path = Some(path);
        self.agents_md_signature = None;
    }

    /// Attaches a cancellation token consulted between turns.
    pub fn with_cancel_token(mut self, token: CancelToken) -> Self {
        self.cancel = Some(token);
        self
    }

    /// Replaces the cancellation token in place.
    pub fn set_cancel_token(&mut self, token: CancelToken) {
        self.cancel = Some(token);
    }

    /// Returns a clone of the attached cancellation token, if any. Used by
    /// the committee loop in `peridot-cli` to check cancellation between
    /// executor turns and reviewer passes.
    pub fn cancel_token(&self) -> Option<CancelToken> {
        self.cancel.clone()
    }

    /// Configures the on-disk path the agent loop should snapshot its
    /// [`ContextManager`] entries into after every turn. The write happens
    /// atomically via `tempfile + rename` so concurrent crashes never expose
    /// half-written blobs.
    pub fn set_context_snapshot_path(&mut self, path: PathBuf) {
        self.context_snapshot_path = Some(path);
    }

    /// Returns the current agent state.
    pub fn state(&self) -> &AgentState {
        &self.state
    }

    /// Returns the context manager.
    pub fn context(&self) -> &ContextManager {
        &self.context
    }

    /// Returns a mutable context manager.
    pub fn context_mut(&mut self) -> &mut ContextManager {
        &mut self.context
    }

    /// Returns the tool registry.
    pub fn tools(&self) -> &ToolRegistry {
        &self.tools
    }

    /// Executes one tool call through the registered tool boundary.
    pub async fn execute_tool_call(
        &self,
        call: ToolCall,
        project_root: impl Into<PathBuf>,
    ) -> PeriResult<ToolResult> {
        self.execute_tool_call_with_denied_paths(call, project_root, Vec::new())
            .await
    }

    /// Executes one tool call with explicit project path boundaries.
    pub async fn execute_tool_call_with_denied_paths(
        &self,
        call: ToolCall,
        project_root: impl Into<PathBuf>,
        denied_paths: Vec<PathBuf>,
    ) -> PeriResult<ToolResult> {
        self.execute_tool_call_with_runtime(
            call,
            project_root,
            denied_paths,
            peridot_common::HooksConfig::default(),
            SecurityConfig::default(),
        )
        .await
    }

    /// Executes one tool call with explicit boundaries and hook configuration.
    pub async fn execute_tool_call_with_runtime(
        &self,
        call: ToolCall,
        project_root: impl Into<PathBuf>,
        denied_paths: Vec<PathBuf>,
        hooks: peridot_common::HooksConfig,
        security: SecurityConfig,
    ) -> PeriResult<ToolResult> {
        let tool = self
            .tools
            .get(&call.name)
            .ok_or_else(|| PeriError::Tool(format!("unknown tool: {}", call.name)))?;
        ensure_tool_allowed(self.state.mode, self.state.phase, tool.group(), &call.name)?;
        let project_root = project_root.into();
        let mut ctx = ToolContext::new(project_root.clone(), self.state.permission)
            .with_denied_paths(denied_paths)
            .with_hooks(hooks)
            .with_security(security);
        if let Some(token) = self.cancel.clone() {
            ctx = ctx.with_cancel(token);
        }
        if let Some(runner) = self.subagent_runner.clone() {
            ctx = ctx.with_subagent_runner(runner);
        }
        tool.validate_params(&call.parameters)?;
        let runner = HookRunner::new(&project_root, ctx.hooks.clone());
        let mut variables = tool_hook_variables(&call.name, &call.parameters);
        variables.insert(
            "project_root".to_string(),
            project_root.display().to_string(),
        );
        variables.insert("workspace".to_string(), project_root.display().to_string());
        variables.insert("mode".to_string(), self.state.mode.to_string());
        variables.insert("permission".to_string(), self.state.permission.to_string());
        runner.run_tool_hooks(&format!("pre:{}", call.name), &variables)?;
        let tool_name = call.name.clone();
        let params = call.parameters.clone();
        let result = tool.execute(call.parameters, &ctx).await?;
        let _ = append_audit_event(
            &project_root,
            &AuditEvent::tool_call(
                &tool_name,
                result.success,
                &result.summary,
                serde_json::json!({
                    "params": params,
                    "phase": self.state.phase,
                    "mode": self.state.mode,
                    "permission": self.state.permission
                }),
            ),
        );
        variables.insert(
            "result_json".to_string(),
            serde_json::to_string(&result).map_err(|err| {
                PeriError::Parse(format!("failed to serialize hook result: {err}"))
            })?,
        );
        runner.run_tool_hooks(&format!("post:{}", call.name), &variables)?;
        Ok(result)
    }

    /// Runs one model/tool turn and records the observation in context.
    pub async fn run_turn<P>(
        &mut self,
        provider: &P,
        request: AgentTurnRequest,
    ) -> PeriResult<AgentTurnOutcome>
    where
        P: LlmProvider + ?Sized,
    {
        self.run_turn_with_events(provider, request, &mut |_| {})
            .await
    }

    /// Runs one model/tool turn and emits user-interface events.
    pub async fn run_turn_with_events<P, F>(
        &mut self,
        provider: &P,
        request: AgentTurnRequest,
        events: &mut F,
    ) -> PeriResult<AgentTurnOutcome>
    where
        P: LlmProvider + ?Sized,
        F: FnMut(AgentRunEvent),
    {
        // Start of a new turn: bump the turn id so every entry
        // appended below shares one id, enabling later `/branch turn`
        // forks at this exact point.
        self.context.bump_turn_id();
        if let Some(user_input) = request.user_input {
            self.context
                .append(ContextEntry::trusted(ContextSource::User, user_input));
        }
        if let Some(plan) = read_plan_reminder(&request.project_root) {
            self.context
                .append(ContextEntry::trusted(ContextSource::PlanReminder, plan));
        }
        let estimated_tokens = self.context.estimated_tokens();
        // Tier 3 first: ask the model to produce a structured recap.
        // When the operator queued `/compact` we bypass the threshold
        // entirely; otherwise the dynamic threshold (auto_compaction_pct
        // of the active model window) decides. Falls back to Tier 1
        // (deterministic summary) if the LLM call errors or produces
        // no compaction. Provider errors are swallowed so a compaction
        // hiccup never aborts the run.
        let force_compact = self
            .compact_request
            .as_ref()
            .map(|flag| flag.swap(false, Ordering::SeqCst))
            .unwrap_or(false);
        let mut compacted = if force_compact {
            self.context
                .force_compact_with_llm(provider, &request.model)
                .await
                .unwrap_or_default()
        } else {
            self.context
                .compact_with_llm(provider, &request.model)
                .await
                .unwrap_or_default()
        };
        if !compacted {
            compacted = self.context.compact_if_needed();
        }
        if compacted {
            run_context_compacted_hook(
                &request.project_root,
                &request.hooks,
                estimated_tokens,
                self.context.compaction_threshold_tokens(),
            )?;
        }

        events(AgentRunEvent::AssistantStarted {
            label: "assistant".to_string(),
        });
        let tool_definitions = registry_tool_definitions(&self.tools);
        let completion = stream_completion_with_chunks(
            provider,
            CompletionRequest {
                model: request.model,
                system: Some(system_prompt_for_role(self.state.mode, self.role)),
                messages: self.context.to_messages(),
                max_tokens: Some(request.max_tokens),
                thinking: self.state.mode == ExecutionMode::Goal,
                reasoning_effort: request.reasoning_effort,
                tools: tool_definitions,
                tool_choice: ToolChoice::Auto,
            },
            |chunk| {
                if !chunk.delta.is_empty() {
                    events(AgentRunEvent::AssistantDelta {
                        delta: chunk.delta.clone(),
                    });
                }
            },
        )
        .await?;
        events(AgentRunEvent::AssistantFinished {
            text: completion.text.clone(),
        });

        // We only honour the first tool call per turn. Parallel calls are surfaced
        // as a thinking event so the operator sees them, but the loop keeps its
        // single-tool-per-turn invariant; future work can fan them out.
        let first_tool_call = completion.tool_calls.first().cloned();
        if completion.tool_calls.len() > 1 {
            events(AgentRunEvent::Thinking {
                text: format!(
                    "ignoring {} additional parallel tool call(s)",
                    completion.tool_calls.len() - 1
                ),
            });
        }

        // No tool call → treat the assistant's text as a chat-style reply and finish
        // the turn synthetically through `agent_done`. Push the assistant text into
        // context so future turns see the reply, then execute `agent_done` for the
        // audit trail and phase transition. We deliberately do NOT emit
        // `ToolStarted`/`ToolFinished` events: the chat text is already on screen as
        // an `Assistant` transcript entry, and re-rendering it as
        // `❯ agent_done running` + `✔ agent_done <text>` produces a duplicated reply
        // in green. The outer `TurnEnded` / `Finished` events still fire from
        // `run_until_done_with_events`, so the loop stays observable without the
        // visual noise.
        let Some(invocation) = first_tool_call else {
            if !completion.text.trim().is_empty() {
                self.context.append(ContextEntry::trusted(
                    ContextSource::Assistant,
                    completion.text.clone(),
                ));
            }
            let summary = if completion.text.trim().is_empty() {
                "no response".to_string()
            } else {
                completion.text.clone()
            };
            let tool_call = ToolCall {
                name: "agent_done".to_string(),
                parameters: serde_json::json!({ "summary": summary }),
            };
            self.state.phase = AgentPhase::Executing;
            let tool_result = self
                .execute_tool_call_with_runtime(
                    tool_call,
                    request.project_root,
                    request.denied_paths,
                    request.hooks,
                    request.security,
                )
                .await?;
            self.state.phase = AgentPhase::Done;
            return Ok(AgentTurnOutcome {
                tool_name: "agent_done".to_string(),
                tool_result,
                usage: completion.usage,
                done: true,
            });
        };

        // Native tool-call protocol path. Record the assistant turn carrying the
        // structured `tool_calls` so the next request to the provider replays the
        // exact wire shape the model was trained on (assistant message + tool_calls
        // → tool message with matching tool_call_id → next assistant turn).
        self.context.append(ContextEntry::assistant_with_tool_calls(
            completion.text.clone(),
            completion.tool_calls.clone(),
        ));

        let tool_call_id = invocation.id.clone();
        let tool_call = ToolCall {
            name: invocation.name.clone(),
            parameters: tool_invocation_parameters(&invocation),
        };
        let tool_name = tool_call.name.clone();
        let tool_parameters = tool_call.parameters.clone();
        self.state.phase = AgentPhase::Executing;
        // When the model both streams a reply AND closes the turn with
        // `agent_done`, the `agent_done` summary almost always duplicates the
        // text the user just read (qwen does this consistently). Suppress the
        // tool UI events in that case so the transcript shows the reply once.
        // The tool still runs internally for audit / phase transition; we
        // simply don't surface the redundant `❯ agent_done` / `✔ agent_done
        // <summary>` lines. When the model used `agent_done` AS the response
        // channel (no preceding text), the events DO fire so the summary
        // reaches the user — that's the only signal they'd otherwise see.
        let suppress_done_ui =
            tool_name == "agent_done" && !completion.text.trim().is_empty();
        if !suppress_done_ui {
            events(AgentRunEvent::ToolStarted {
                name: tool_name.clone(),
                parameters: tool_parameters.clone(),
            });
        }
        let tool_result = match self
            .execute_tool_call_with_runtime(
                tool_call,
                request.project_root,
                request.denied_paths,
                request.hooks,
                request.security,
            )
            .await
        {
            Ok(result) => result,
            Err(err) => {
                if let PeriError::PermissionDenied(reason) = &err {
                    events(AgentRunEvent::ApprovalRequested {
                        tool_name,
                        reason: reason.clone(),
                        parameters: tool_parameters,
                    });
                }
                return Err(err);
            }
        };
        if !suppress_done_ui {
            events(AgentRunEvent::ToolFinished {
                name: tool_name.clone(),
                result: tool_result.clone(),
            });
        }
        // The tool result is a tool-role message paired with the assistant's
        // `tool_call_id`. We bypass `append_observation` because it stamps every
        // entry as untrusted and offload-eligible; here we want the provider to
        // receive it through the native tool message channel instead, so the
        // model sees its own past action and result without re-running them.
        let observation = serde_json::to_string(&tool_result).map_err(|err| {
            PeriError::Parse(format!("failed to serialize tool result: {err}"))
        })?;
        self.context.append(
            ContextEntry::trusted(ContextSource::Tool, observation)
                .with_tool_call_id(tool_call_id),
        );

        if tool_name == "agent_done" && tool_result.success {
            self.state.phase = AgentPhase::Done;
        } else {
            self.state.phase = AgentPhase::Verifying;
        }

        Ok(AgentTurnOutcome {
            tool_name,
            tool_result,
            usage: completion.usage,
            done: self.state.phase == AgentPhase::Done,
        })
    }

    /// Runs model/tool turns until done or guardrail exhaustion.
    pub async fn run_until_done<P>(
        &mut self,
        provider: &P,
        request: AgentRunRequest,
    ) -> PeriResult<AgentRunSummary>
    where
        P: LlmProvider + ?Sized,
    {
        self.run_until_done_with_events(provider, request, |_| {})
            .await
    }

    /// Runs model/tool turns until done while emitting user-interface events.
    pub async fn run_until_done_with_events<P, F>(
        &mut self,
        provider: &P,
        request: AgentRunRequest,
        mut events: F,
    ) -> PeriResult<AgentRunSummary>
    where
        P: LlmProvider + ?Sized,
        F: FnMut(AgentRunEvent),
    {
        events(AgentRunEvent::RunStarted {
            task: request.task.clone(),
        });
        // Stamp the run's wall-clock start so the final `AgentRunSummary` can
        // report how long the task took. Using `Instant` instead of
        // `SystemTime` keeps the measurement monotonic across NTP jumps.
        let started_at = std::time::Instant::now();
        let mut outcomes = Vec::new();
        let mut total_usage = Usage::default();
        let mut stuck_detector = StuckDetector::new(3);
        let mut budget_warning_sent = false;
        let mut consecutive_parse_failures = 0usize;
        // Auto-fix loop counter. Increments every time a `verify_*` tool
        // (build / test / lint) returns a non-success ToolResult; resets
        // on any other outcome. When it crosses the cap we abort the run
        // so a perpetually-failing checker can't burn the budget while
        // the model thrashes — operators can re-run after fixing the
        // root cause manually.
        let mut consecutive_verify_failures = 0u32;
        const VERIFY_FIX_CAP: u32 = 3;
        for turn_index in 0..request.max_turns {
            if self
                .cancel
                .as_ref()
                .map(|token| token.is_cancelled())
                .unwrap_or(false)
            {
                events(AgentRunEvent::Interrupted {
                    stage: "turn_start".to_string(),
                });
                let summary = AgentRunSummary {
                    turns: outcomes,
                    usage: total_usage,
                    stopped_reason: StopReason::Interrupted,
                duration_ms: started_at.elapsed().as_millis() as u64,
                };
                events(AgentRunEvent::Finished {
                    summary: summary.clone(),
                });
                return Ok(summary);
            }
            self.refresh_agents_md(&mut events);
            events(AgentRunEvent::TurnStarted { turn_index });
            let outcome = match self
                .run_turn_with_events(
                    provider,
                    AgentTurnRequest {
                        user_input: (turn_index == 0).then(|| request.task.clone()),
                        model: request.model.clone(),
                        max_tokens: request.max_tokens,
                        reasoning_effort: request.reasoning_effort,
                        project_root: request.project_root.clone(),
                        denied_paths: request.denied_paths.clone(),
                        hooks: request.hooks.clone(),
                        security: request.security.clone(),
                    },
                    &mut events,
                )
                .await
            {
                Ok(outcome) => {
                    consecutive_parse_failures = 0;
                    outcome
                }
                Err(err) => {
                    if approval_required_error(&err) {
                        let summary = AgentRunSummary {
                            turns: outcomes,
                            usage: total_usage,
                            stopped_reason: StopReason::ApprovalRequired,
                        duration_ms: started_at.elapsed().as_millis() as u64,
                        };
                        events(AgentRunEvent::Finished {
                            summary: summary.clone(),
                        });
                        return Ok(summary);
                    }
                    self.state.phase = AgentPhase::Recovering;
                    run_error_event_hooks(&request.project_root, &request.hooks, &err)?;
                    if classify_error(&err) == "parse" {
                        consecutive_parse_failures += 1;
                    } else {
                        consecutive_parse_failures = 0;
                    }
                    self.context.append(ContextEntry::trusted(
                        ContextSource::PlanReminder,
                        recovery_message(&err),
                    ));
                    events(AgentRunEvent::Recovery {
                        message: err.to_string(),
                    });
                    if consecutive_parse_failures == 3 {
                        self.context.append(ContextEntry::trusted(
                            ContextSource::PlanReminder,
                            format_reminder_message(),
                        ));
                    }
                    continue;
                }
            };
            let turn_success = outcome.tool_result.success;
            // Auto-fix loop: when verify_* fails, inject a hard "fix
            // this first" directive so the model anchors on the
            // failing check instead of drifting back to feature work.
            // Cap at `VERIFY_FIX_CAP` consecutive failures so a
            // permanently-broken checker doesn't blow the budget.
            let is_verify_tool = outcome.tool_name.starts_with("verify_");
            if is_verify_tool && !turn_success {
                consecutive_verify_failures += 1;
                if consecutive_verify_failures >= VERIFY_FIX_CAP {
                    let message = format!(
                        "Auto-fix loop circuit breaker: {} failed {} times in a row. Aborting so the operator can intervene.",
                        outcome.tool_name, consecutive_verify_failures
                    );
                    self.state.phase = AgentPhase::Recovering;
                    run_recovery_event_hook(
                        &request.project_root,
                        &request.hooks,
                        "auto_fix_abort",
                        &message,
                    )?;
                    events(AgentRunEvent::Recovery {
                        message: message.clone(),
                    });
                    outcomes.push(outcome);
                    let summary = AgentRunSummary {
                        turns: outcomes,
                        usage: total_usage,
                        stopped_reason: StopReason::Interrupted,
                        duration_ms: started_at.elapsed().as_millis() as u64,
                    };
                    events(AgentRunEvent::Finished {
                        summary: summary.clone(),
                    });
                    return Ok(summary);
                }
                self.context.append(ContextEntry::trusted(
                    ContextSource::PlanReminder,
                    format!(
                        "Auto-fix directive ({}/{}): {} reported failure. STOP all new work. Read the failing output above, make the smallest change that restores the check, and re-run the same `{}` to confirm. Do not proceed to other tasks until the verifier passes.",
                        consecutive_verify_failures,
                        VERIFY_FIX_CAP,
                        outcome.tool_name,
                        outcome.tool_name,
                    ),
                ));
            } else {
                consecutive_verify_failures = 0;
            }
            accumulate_usage(&mut total_usage, outcome.usage);
            // Auto-verify after mutation: if the operator opted into
            // "verify after every mutation", run `verify_build`
            // immediately so a broken compile surfaces while the
            // diff is still fresh. The result is appended as a
            // PlanReminder so the next model turn reads it without
            // expecting a paired native tool_call_id.
            if self.auto_verify_after_mutation
                && turn_success
                && is_mutating_tool_name(&outcome.tool_name)
            {
                let auto_verify = self
                    .execute_tool_call_with_runtime(
                        ToolCall {
                            name: "verify_build".to_string(),
                            parameters: serde_json::json!({}),
                        },
                        request.project_root.clone(),
                        request.denied_paths.clone(),
                        request.hooks.clone(),
                        request.security.clone(),
                    )
                    .await;
                match auto_verify {
                    Ok(result) => {
                        let note = if result.success {
                            format!("[auto-verify] verify_build passed: {}", result.summary)
                        } else {
                            format!(
                                "[auto-verify] verify_build FAILED: {}\nFix this before declaring agent_done.",
                                result.summary
                            )
                        };
                        self.context.append(ContextEntry::trusted(
                            ContextSource::PlanReminder,
                            note,
                        ));
                    }
                    Err(err) => {
                        // Verify infrastructure isn't available
                        // (e.g. no build command configured). Surface
                        // a quiet note but never abort the run.
                        self.context.append(ContextEntry::trusted(
                            ContextSource::PlanReminder,
                            format!("[auto-verify] verify_build could not run: {err}"),
                        ));
                    }
                }
            }
            events(AgentRunEvent::UsageUpdated { usage: total_usage });
            events(AgentRunEvent::TurnEnded {
                turn_index,
                success: turn_success,
            });
            if let Some(path) = self.context_snapshot_path.as_ref() {
                snapshot_context_to_disk(path, &self.context, &mut events);
            }
            if should_emit_budget_warning(
                request.budget_usd,
                request.budget_warning_pct,
                total_usage.estimated_cost_usd,
                budget_warning_sent,
            ) {
                run_budget_warning_hook(
                    &request.project_root,
                    &request.hooks,
                    total_usage.estimated_cost_usd,
                    request.budget_usd,
                )?;
                budget_warning_sent = true;
                if self.state.mode == ExecutionMode::Goal {
                    self.context.append(ContextEntry::trusted(
                        ContextSource::PlanReminder,
                        budget_warning_message(total_usage.estimated_cost_usd, request.budget_usd),
                    ));
                }
            }
            let done = outcome.done;
            let stuck_action = stuck_detector.record(&outcome);
            outcomes.push(outcome);
            match stuck_action {
                StuckAction::Continue => {}
                StuckAction::Recover(message) => {
                    self.state.phase = AgentPhase::Recovering;
                    run_recovery_event_hook(
                        &request.project_root,
                        &request.hooks,
                        "stuck",
                        &message,
                    )?;
                    self.context
                        .append(ContextEntry::trusted(ContextSource::PlanReminder, message));
                    events(AgentRunEvent::Recovery {
                        message: "stuck detector requested a new strategy".to_string(),
                    });
                }
                StuckAction::Abort(message) => {
                    // Hard circuit-breaker. The model has ignored the recovery
                    // directive for too many turns; stop the run before we burn
                    // more tokens. Surface a Recovery event so the TUI's
                    // activity panel records what happened.
                    self.state.phase = AgentPhase::Recovering;
                    run_recovery_event_hook(
                        &request.project_root,
                        &request.hooks,
                        "stuck_abort",
                        &message,
                    )?;
                    events(AgentRunEvent::Recovery {
                        message: message.clone(),
                    });
                    let summary = AgentRunSummary {
                        turns: outcomes,
                        usage: total_usage,
                        stopped_reason: StopReason::Interrupted,
                    duration_ms: started_at.elapsed().as_millis() as u64,
                    };
                    events(AgentRunEvent::Finished {
                        summary: summary.clone(),
                    });
                    return Ok(summary);
                }
            }
            if done {
                if let Some(goal_checker_model) = request.goal_checker_model.as_deref()
                    && self.state.mode == ExecutionMode::Goal
                {
                    let verdict = check_goal_satisfied(
                        provider,
                        goal_checker_model,
                        &request.task,
                        &outcomes,
                    )
                    .await?;
                    accumulate_usage(&mut total_usage, verdict.usage);
                    self.context.append(ContextEntry::trusted(
                        ContextSource::PlanReminder,
                        format!("Goal checker verdict: {}", verdict.reason),
                    ));
                    if !verdict.satisfied {
                        self.state.phase = AgentPhase::Recovering;
                        run_recovery_event_hook(
                            &request.project_root,
                            &request.hooks,
                            "goal_checker",
                            &verdict.reason,
                        )?;
                        self.context.append(ContextEntry::trusted(
                            ContextSource::PlanReminder,
                            "Goal checker says the objective is not satisfied yet. Continue with a concrete next action.".to_string(),
                        ));
                        continue;
                    }
                }
                // Auto-grade on agent_done: ask an LLM to gate the
                // task. When the grader fails the verdict we fold
                // recommendations back into context and `continue` —
                // the model gets one more turn to address them
                // instead of finishing. Cheap callers can keep this
                // off; the gate is `auto_grade_on_done`.
                if self.auto_grade_on_done {
                    let diff = collect_git_diff(&request.project_root);
                    let verify_summary =
                        recent_verify_summary(&self.context).unwrap_or_default();
                    match crate::grader::grade_work(
                        provider,
                        &request.model,
                        &request.task,
                        &diff,
                        &verify_summary,
                    )
                    .await
                    {
                        Ok(verdict) => {
                            accumulate_usage(&mut total_usage, verdict.usage);
                            if !verdict.passed {
                                self.state.phase = AgentPhase::Recovering;
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
                                directive.push_str("\nAddress the recommendations and call agent_done again only when the change actually ships.");
                                self.context.append(ContextEntry::trusted(
                                    ContextSource::PlanReminder,
                                    directive,
                                ));
                                events(AgentRunEvent::Recovery {
                                    message: format!(
                                        "auto-grade failed: {}",
                                        verdict.summary
                                    ),
                                });
                                continue;
                            }
                            self.context.append(ContextEntry::trusted(
                                ContextSource::PlanReminder,
                                format!("[auto-grade] Grader passed: {}", verdict.summary),
                            ));
                        }
                        Err(err) => {
                            // Grader infrastructure failed (provider
                            // hiccup, unparseable response after
                            // retries). Surface a note and let
                            // agent_done stand — degrading to the
                            // legacy "first done wins" path is safer
                            // than blocking the operator on a flaky
                            // grader.
                            self.context.append(ContextEntry::trusted(
                                ContextSource::PlanReminder,
                                format!("[auto-grade] Grader unavailable: {err}"),
                            ));
                        }
                    }
                }
                let summary = AgentRunSummary {
                    turns: outcomes,
                    usage: total_usage,
                    stopped_reason: StopReason::Done,
                duration_ms: started_at.elapsed().as_millis() as u64,
                };
                events(AgentRunEvent::Finished {
                    summary: summary.clone(),
                });
                return Ok(summary);
            }
            if request.budget_usd > 0.0 && total_usage.estimated_cost_usd >= request.budget_usd {
                self.state.phase = AgentPhase::Recovering;
                self.context.append(ContextEntry::trusted(
                    ContextSource::PlanReminder,
                    budget_exceeded_message(total_usage.estimated_cost_usd, request.budget_usd),
                ));
                let summary = AgentRunSummary {
                    turns: outcomes,
                    usage: total_usage,
                    stopped_reason: StopReason::Budget,
                duration_ms: started_at.elapsed().as_millis() as u64,
                };
                events(AgentRunEvent::Finished {
                    summary: summary.clone(),
                });
                return Ok(summary);
            }
        }

        let summary = AgentRunSummary {
            turns: outcomes,
            usage: total_usage,
            stopped_reason: StopReason::MaxTurns,
        duration_ms: started_at.elapsed().as_millis() as u64,
        };
        events(AgentRunEvent::Finished {
            summary: summary.clone(),
        });
        Ok(summary)
    }
}

impl HarnessAgent {
    /// Compares the configured AGENTS.md path's `(modified_unix, len)`
    /// fingerprint against the last seen one and, when it changed, re-reads
    /// the file, injects its contents into context as a `PlanReminder`
    /// entry, and emits `AgentRunEvent::AgentsMdLoaded` with the rule count
    /// and origin path. The first call after `set_agents_md_path` always
    /// fires the inject because `agents_md_signature` starts as `None`.
    fn refresh_agents_md<F>(&mut self, events: &mut F)
    where
        F: FnMut(AgentRunEvent),
    {
        let Some(path) = self.agents_md_path.clone() else {
            return;
        };
        let Ok(meta) = std::fs::metadata(&path) else {
            return;
        };
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let len = meta.len();
        let signature = (modified, len);
        if self.agents_md_signature == Some(signature) {
            return;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            return;
        };
        let rule_count = content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count() as u32;
        self.context.append(ContextEntry::trusted(
            ContextSource::PlanReminder,
            format!(
                "AGENTS.md reloaded ({rule_count} lines) from {}:\n{content}",
                path.display()
            ),
        ));
        events(AgentRunEvent::AgentsMdLoaded {
            rule_count,
            paths: vec![path.display().to_string()],
        });
        self.agents_md_signature = Some(signature);
    }
}

fn approval_required_error(err: &PeriError) -> bool {
    match err {
        PeriError::PermissionDenied(reason) => reason.contains("requires explicit user approval"),
        _ => false,
    }
}

/// Returns true when the named tool mutates the workspace. Matches the
/// hardcoded list the committee loop uses (`run_committee_loop_with_events`)
/// so auto-verify and reviewer triggering stay aligned.
fn is_mutating_tool_name(name: &str) -> bool {
    matches!(name, "file_write" | "file_patch" | "shell_exec")
}

/// Runs `git diff` in `project_root` and returns its stdout. Returns
/// an empty string when git isn't available or the directory isn't a
/// repo — auto-grade is best-effort, never blocks the run.
fn collect_git_diff(project_root: &std::path::Path) -> String {
    std::process::Command::new("git")
        .args(["diff", "HEAD"])
        .current_dir(project_root)
        .output()
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).to_string())
        .unwrap_or_default()
}

/// Scans the most recent context entries for a `verify_*` tool result
/// or `[auto-verify]` PlanReminder note. Used as the third grader
/// input so the LLM gates on actual check output, not just the diff.
fn recent_verify_summary(context: &ContextManager) -> Option<String> {
    for entry in context.entries().iter().rev().take(20) {
        let content = entry.content.trim();
        if content.starts_with("[auto-verify]") {
            return Some(content.to_string());
        }
        if entry.source == ContextSource::Tool && content.contains("verify_") {
            return Some(content.to_string());
        }
    }
    None
}

fn snapshot_context_to_disk<F>(path: &std::path::Path, context: &ContextManager, events: &mut F)
where
    F: FnMut(AgentRunEvent),
{
    let entries = context.snapshot_entries();
    let bytes = match serde_json::to_vec(&entries) {
        Ok(bytes) => bytes,
        Err(err) => {
            events(AgentRunEvent::Recovery {
                message: format!("context snapshot serialize failed: {err}"),
            });
            return;
        }
    };
    if let Some(parent) = path.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        events(AgentRunEvent::Recovery {
            message: format!(
                "context snapshot create_dir_all {} failed: {err}",
                parent.display()
            ),
        });
        return;
    }
    let temp = path.with_extension("tmp");
    if let Err(err) = std::fs::write(&temp, &bytes) {
        events(AgentRunEvent::Recovery {
            message: format!("context snapshot write {} failed: {err}", temp.display()),
        });
        return;
    }
    if let Err(err) = std::fs::rename(&temp, path) {
        events(AgentRunEvent::Recovery {
            message: format!(
                "context snapshot rename {} -> {} failed: {err}",
                temp.display(),
                path.display()
            ),
        });
    }
}

/// Builds the provider-neutral tool definitions surfaced through native tool calling
/// from every tool registered on the harness. Order is deterministic because the
/// registry stores tools in a `BTreeMap`.
fn registry_tool_definitions(registry: &ToolRegistry) -> Vec<ToolDefinition> {
    registry
        .descriptors()
        .into_iter()
        .map(|descriptor| ToolDefinition {
            name: descriptor.name,
            description: descriptor.description,
            parameters: descriptor.parameters,
        })
        .collect()
}

/// Decodes the model's tool call arguments. Providers normalise them to a
/// `serde_json::Value`, but Chat Completions returns the JSON as a raw string that
/// may already have been parsed into `Value::Null` on the wire — coerce both shapes
/// into an object so downstream `validate_params` checks keep working.
fn tool_invocation_parameters(invocation: &ToolInvocation) -> serde_json::Value {
    match &invocation.arguments {
        serde_json::Value::Null => serde_json::json!({}),
        serde_json::Value::String(raw) => serde_json::from_str(raw).unwrap_or(serde_json::json!({})),
        other => other.clone(),
    }
}

