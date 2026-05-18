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
use peridot_tools::{AskUserPort, ToolContext, ToolRegistry};

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
use peridot_common::CancelToken;

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
    ask_user_port: Option<Arc<dyn AskUserPort>>,
    auto_verify_after_mutation: bool,
    auto_grade_on_done: bool,
    /// Optional flag the operator sets via `/compact` to force an LLM
    /// recap on the next turn boundary, even when the buffer is well
    /// below the auto trigger. Atomic so the slash command thread and
    /// the agent loop can share it without locking.
    compact_request: Option<Arc<AtomicBool>>,
    /// Sidecar path used to persist a pending tool call when the
    /// previous run halted on `ApprovalRequired`. On the next session
    /// start the harness reads the file, executes the exact tool call
    /// (under whatever security/permission posture the operator
    /// granted), folds the result into context, and deletes the
    /// sidecar. The model is NOT re-asked; the run picks up from the
    /// point it stopped.
    pending_resume_path: Option<PathBuf>,
    /// Pre-built tool definitions surfaced to the LLM. The registry is
    /// frozen at `new()` time (the field below takes ownership), so the
    /// provider-neutral descriptor list never changes during the session.
    /// Caching it here avoids walking the `BTreeMap` and re-running each
    /// tool's `parameters_schema()` (which often allocates a fresh JSON
    /// tree) on every turn.
    cached_tool_definitions: Vec<ToolDefinition>,
}

impl HarnessAgent {
    /// Creates a harness agent from state and dependencies.
    pub fn new(state: AgentState, context: ContextManager, tools: ToolRegistry) -> Self {
        let cached_tool_definitions = registry_tool_definitions(&tools);
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
            ask_user_port: None,
            auto_verify_after_mutation: false,
            auto_grade_on_done: false,
            compact_request: None,
            pending_resume_path: None,
            cached_tool_definitions,
        }
    }

    /// Configures the file Pending tool calls are persisted to when an
    /// approval halt fires, and re-loaded from on next session start.
    /// Typically points at `.peridot/sessions/<id>/pending_resume.bin`.
    pub fn set_pending_resume_path(&mut self, path: PathBuf) {
        self.pending_resume_path = Some(path);
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

    /// Installs an ask-user port. The harness injects it into every
    /// `ToolContext` it builds so `agent_ask_user` actually awaits a
    /// real user answer through the interactive front-end. Headless and
    /// test harnesses leave it unset and the tool keeps its synthesised
    /// default fallback.
    pub fn set_ask_user_port(&mut self, port: Arc<dyn AskUserPort>) {
        self.ask_user_port = Some(port);
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
        if let Some(port) = self.ask_user_port.clone() {
            ctx = ctx.with_ask_user_port(port);
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
        let checkpoint_id = if tool.modifies_state() {
            write_file_checkpoint(&project_root, &tool_name, &params)
                .ok()
                .flatten()
        } else {
            None
        };
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
                    "permission": self.state.permission,
                    "checkpoint_id": checkpoint_id
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
        // Plan reminder is co-injected with a real user turn only.
        // Re-injecting on every internal multi-step iteration (after a
        // tool result, etc.) inflates the context by `todo.md`-sized
        // chunks each step — the operator saw 956 tokens for a one-word
        // "hi" because the same plan reminder was appended twice in a
        // two-step run. The model already sees the reminder once via
        // the previous turn's context, so a single injection per real
        // user prompt is sufficient.
        if let Some(user_input) = request.user_input {
            self.context
                .append(ContextEntry::trusted(ContextSource::User, user_input));
            if let Some(plan) = read_plan_reminder(&request.project_root) {
                self.context
                    .append(ContextEntry::trusted(ContextSource::PlanReminder, plan));
            }
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
                self.context.llm_compaction_threshold(),
            )?;
            // Surface the auto-compaction in the transcript so the
            // operator can see that the 90%-of-window guard rail did
            // fire. Without this, compaction is invisible — the hook
            // path only spawns user-defined shell scripts.
            let threshold = self.context.llm_compaction_threshold();
            let post_tokens = self.context.estimated_tokens();
            events(AgentRunEvent::Thinking {
                text: format!(
                    "context compacted: {estimated_tokens} tok → {post_tokens} tok (threshold {threshold})"
                ),
            });
        }
        // Always emit the current context size so the TUI can render a
        // `ctx used/window` indicator in the status line. The displayed window
        // is the full model context; auto-compaction still triggers at
        // `window * auto_compaction_pct`.
        let window = self.context.model_context_window_tokens();
        let tokens_now = self.context.estimated_tokens();
        events(AgentRunEvent::ContextUtilizationChanged {
            tokens_used: tokens_now as u64,
            threshold: window as u64,
        });

        events(AgentRunEvent::AssistantStarted {
            label: "assistant".to_string(),
        });
        let tool_definitions = self.cached_tool_definitions.clone();
        let completion = stream_completion_with_chunks(
            provider,
            CompletionRequest {
                model: request.model,
                system: Some(system_prompt_for_role(self.state.mode, self.role).to_string()),
                messages: self.context.to_messages(),
                max_tokens: Some(request.max_tokens),
                thinking: self.state.mode == ExecutionMode::Goal,
                reasoning_effort: request.reasoning_effort,
                service_tier: request.service_tier,
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

        let tool_call_id = invocation.id.clone();
        // Native tool-call protocol path. Record only the tool call this harness
        // will execute. The loop deliberately enforces a single-tool-per-turn
        // invariant; replaying ignored parallel calls would make Responses-style
        // providers reject the next turn because those calls have no matching
        // `function_call_output`.
        self.context.append(ContextEntry::assistant_with_tool_calls(
            completion.text.clone(),
            vec![invocation.clone()],
        ));

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
        let suppress_done_ui = tool_name == "agent_done" && !completion.text.trim().is_empty();
        if !suppress_done_ui {
            events(AgentRunEvent::ToolStarted {
                name: tool_name.clone(),
                parameters: tool_parameters.clone(),
            });
        }
        let pending_for_resume = ToolCall {
            name: tool_name.clone(),
            parameters: tool_parameters.clone(),
        };
        let tool_result = match self
            .execute_tool_call_with_runtime(
                ToolCall {
                    name: tool_name.clone(),
                    parameters: tool_parameters.clone(),
                },
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
                    // Persist the exact pending tool call so the next
                    // session can resume from this point instead of
                    // restarting the whole task. Best-effort: a write
                    // failure just degrades to the legacy restart UX.
                    if let Some(path) = self.pending_resume_path.as_ref()
                        && let Ok(bytes) = serde_json::to_vec(&pending_for_resume)
                    {
                        if let Some(parent) = path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = std::fs::write(path, &bytes);
                    }
                    events(AgentRunEvent::ApprovalRequested {
                        tool_name,
                        reason: reason.clone(),
                        parameters: tool_parameters,
                    });
                }
                return Err(err);
            }
        };
        // Bind tool_call back for downstream code that reads from it.
        let tool_call = pending_for_resume;
        // Hold a reference so the compiler doesn't complain about the
        // unused binding when only one branch consumes it.
        let _ = &tool_call;
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
        let observation = serde_json::to_string(&tool_result)
            .map_err(|err| PeriError::Parse(format!("failed to serialize tool result: {err}")))?;
        self.context.append(
            ContextEntry::trusted(ContextSource::Tool, observation).with_tool_call_id(tool_call_id),
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
        // Pending-resume check: when the previous run halted on
        // ApprovalRequired we persisted the pending tool call to a
        // sidecar. If that file is present, execute the call directly
        // against the new (presumably relaxed) security posture so
        // the run picks up exactly where it stopped instead of asking
        // the model to redo everything that led up to the gated step.
        let pending_resume = take_pending_resume(self.pending_resume_path.as_ref());
        if let Some(call) = pending_resume {
            let pending_name = call.name.clone();
            let pending_params = call.parameters.clone();
            events(AgentRunEvent::ToolStarted {
                name: pending_name.clone(),
                parameters: pending_params.clone(),
            });
            match self
                .execute_tool_call_with_runtime(
                    call,
                    request.project_root.clone(),
                    request.denied_paths.clone(),
                    request.hooks.clone(),
                    request.security.clone(),
                )
                .await
            {
                Ok(result) => {
                    events(AgentRunEvent::ToolFinished {
                        name: pending_name.clone(),
                        result: result.clone(),
                    });
                    self.context.append(ContextEntry::trusted(
                        ContextSource::PlanReminder,
                        format!(
                            "[resume] Operator approved {pending_name}. Result: {}",
                            result.summary
                        ),
                    ));
                }
                Err(err) => {
                    // Resume failed (still blocked, or environment changed).
                    // Surface it but don't abort — the model can pick up
                    // and try a different approach on its next turn.
                    self.context.append(ContextEntry::trusted(
                        ContextSource::PlanReminder,
                        format!(
                            "[resume] Tried to resume {pending_name} after approval but failed: {err}. Try a different approach."
                        ),
                    ));
                }
            }
        }
        let mut outcomes = Vec::new();
        let mut total_usage = Usage::default();
        let mut stuck_detector = StuckDetector::new(3);
        let mut budget_warning_sent = false;
        let mut consecutive_parse_failures = 0usize;
        // Auto-fix loop state. Tracks the current failing verifier by
        // a compact signature so the model can tell "same failure,
        // same attempted fix" apart from a new failure uncovered by
        // progress.
        let mut verify_failure_state: Option<VerifyFailureState> = None;
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
                        service_tier: request.service_tier.clone(),
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
                    if self
                        .cancel
                        .as_ref()
                        .map(|token| token.is_cancelled())
                        .unwrap_or(false)
                    {
                        events(AgentRunEvent::Interrupted {
                            stage: "turn_error".to_string(),
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
            // Auto-fix loop: when verify_* fails, inject a structured
            // "fix this first" directive keyed by the failure signature
            // so repeated identical failures force a strategy change.
            // Cap repeated failures so a permanently-broken checker
            // doesn't blow the budget.
            let is_verify_tool = outcome.tool_name.starts_with("verify_");
            if is_verify_tool && !turn_success {
                let failure =
                    update_verify_failure_state(&mut verify_failure_state, &outcome).clone();
                if failure.attempts >= VERIFY_FIX_CAP {
                    let message = format!(
                        "Auto-fix loop circuit breaker: {} failed {} times with signature `{}`. Aborting so the operator can intervene.",
                        failure.tool_name, failure.attempts, failure.signature
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
                    verify_failure_directive(&failure, VERIFY_FIX_CAP),
                ));
            } else {
                verify_failure_state = None;
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
                        self.context
                            .append(ContextEntry::trusted(ContextSource::PlanReminder, note));
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
            // Sub-agent review: after agent_delegate returns, parse
            // the SubAgentResult payload and surface its workspace
            // diff to the parent as a [sub-agent review] directive.
            // Stops the parent from rubber-stamping a textual summary
            // without ever looking at what actually changed.
            if outcome.tool_name == "agent_delegate" && outcome.tool_result.success {
                let review = build_subagent_review(&outcome.tool_result.output);
                if !review.is_empty() {
                    self.context
                        .append(ContextEntry::trusted(ContextSource::PlanReminder, review));
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
                    let verify_summary = recent_verify_summary(&self.context).unwrap_or_default();
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
                                    message: format!("auto-grade failed: {}", verdict.summary),
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

/// Reads + deletes the pending-resume sidecar. Returns `Some` only
/// when the file exists, parses as a `ToolCall`, and was successfully
/// removed afterward. Any I/O or parse failure returns `None` so the
/// caller silently falls back to the legacy restart-from-scratch flow.
fn take_pending_resume(path: Option<&PathBuf>) -> Option<ToolCall> {
    let path = path?;
    if !path.exists() {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let call: ToolCall = serde_json::from_slice(&bytes).ok()?;
    // Best-effort delete: if the remove fails we still proceed —
    // worst case the next session re-applies the same tool call.
    let _ = std::fs::remove_file(path);
    Some(call)
}

fn write_file_checkpoint(
    project_root: &std::path::Path,
    tool_name: &str,
    params: &serde_json::Value,
) -> PeriResult<Option<String>> {
    if !matches!(tool_name, "file_write" | "file_patch") {
        return Ok(None);
    }
    let Some(relative) = params.get("path").and_then(serde_json::Value::as_str) else {
        return Ok(None);
    };
    let path = project_root.join(relative);
    let path = peridot_tools::ensure_within_project(project_root, &path)?;
    let existed = path.exists();
    let previous_content = if existed {
        Some(std::fs::read_to_string(&path).map_err(|err| {
            PeriError::Tool(format!(
                "failed to read checkpoint source {}: {err}",
                path.display()
            ))
        })?)
    } else {
        None
    };
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let id = format!("{nanos}-{tool_name}");
    let checkpoints_dir = project_root.join(".peridot/checkpoints");
    std::fs::create_dir_all(&checkpoints_dir).map_err(|err| {
        PeriError::Tool(format!(
            "failed to create checkpoint dir {}: {err}",
            checkpoints_dir.display()
        ))
    })?;
    let checkpoint = serde_json::json!({
        "id": id,
        "tool_name": tool_name,
        "path": relative,
        "existed": existed,
        "previous_content": previous_content,
    });
    let checkpoint_path = checkpoints_dir.join(format!("{id}.json"));
    std::fs::write(
        &checkpoint_path,
        serde_json::to_vec_pretty(&checkpoint)
            .map_err(|err| PeriError::Parse(format!("failed to serialize checkpoint: {err}")))?,
    )
    .map_err(|err| {
        PeriError::Tool(format!(
            "failed to write checkpoint {}: {err}",
            checkpoint_path.display()
        ))
    })?;
    Ok(Some(id))
}

/// Builds the `[sub-agent review]` directive injected after a
/// successful `agent_delegate` call. Extracts the workspace diff from
/// the serialized `SubAgentResult` (under `output.diff`) and wraps it
/// in an explicit "verify this" instruction. Empty diff → empty
/// directive (the helper returns "" and the caller skips the append).
fn build_subagent_review(output: &serde_json::Value) -> String {
    let diff = output.get("diff").and_then(|v| v.as_str()).unwrap_or("");
    let summary = output.get("summary").and_then(|v| v.as_str()).unwrap_or("");
    let workspace = output
        .get("workspace")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if diff.trim().is_empty() {
        // No diff captured — happens for prepare-only LocalSubAgentRunner
        // (no inner execution) or when `git diff` failed. Still surface
        // a soft review reminder so the parent doesn't blindly trust
        // the text summary.
        return format!(
            "[sub-agent review] Sub-agent reported: \"{summary}\". No diff captured. Inspect the workspace at {workspace} before declaring done."
        );
    }
    // Cap the diff at 4000 chars so a giant refactor doesn't blow the
    // parent's context window. The parent can always read individual
    // files for detail.
    let trimmed = if diff.chars().count() > 4000 {
        let head: String = diff.chars().take(4000).collect();
        format!("{head}\n…(diff truncated; read individual files for detail)")
    } else {
        diff.to_string()
    };
    format!(
        "[sub-agent review] Sub-agent finished with summary: \"{summary}\".\n\
         Workspace: {workspace}\n\
         Captured diff:\n\
         ```\n{trimmed}\n```\n\
         Inspect this diff before declaring done. If the change looks wrong, fix it; do not trust the summary text alone."
    )
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct VerifyFailureState {
    tool_name: String,
    signature: String,
    attempts: u32,
}

fn update_verify_failure_state<'a>(
    state: &'a mut Option<VerifyFailureState>,
    outcome: &AgentTurnOutcome,
) -> &'a VerifyFailureState {
    let signature = verify_failure_signature(&outcome.tool_result);
    let attempts = match state.as_ref() {
        Some(previous)
            if previous.tool_name == outcome.tool_name && previous.signature == signature =>
        {
            previous.attempts.saturating_add(1)
        }
        _ => 1,
    };
    *state = Some(VerifyFailureState {
        tool_name: outcome.tool_name.clone(),
        signature,
        attempts,
    });
    state.as_ref().expect("verify failure state just written")
}

fn verify_failure_signature(result: &ToolResult) -> String {
    let mut material = String::new();
    material.push_str(result.summary.trim());
    if !result.output.is_null()
        && let Ok(output) = serde_json::to_string(&result.output)
    {
        material.push('\n');
        material.push_str(&output);
    }
    let normalized = material
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(12)
        .collect::<Vec<_>>()
        .join(" | ");
    truncate_chars(&normalized, 180)
}

fn verify_failure_directive(failure: &VerifyFailureState, cap: u32) -> String {
    let repeat_note = if failure.attempts > 1 {
        " This is the same verifier failure signature as the previous attempt; change strategy before editing again."
    } else {
        ""
    };
    format!(
        "Auto-fix directive ({}/{}): {} failed with signature `{}`. STOP all new work. Diagnose that verifier output, make the smallest targeted change, and re-run `{}`.{} If it fails again with the same signature, do not repeat the same patch pattern; explain the blocker or ask the operator.",
        failure.attempts, cap, failure.tool_name, failure.signature, failure.tool_name, repeat_note,
    )
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
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
        serde_json::Value::String(raw) => {
            serde_json::from_str(raw).unwrap_or(serde_json::json!({}))
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod helpers_tests {
    use super::*;

    #[test]
    fn is_mutating_tool_name_covers_the_canonical_list() {
        assert!(is_mutating_tool_name("file_write"));
        assert!(is_mutating_tool_name("file_patch"));
        assert!(is_mutating_tool_name("shell_exec"));
        assert!(!is_mutating_tool_name("verify_build"));
        assert!(!is_mutating_tool_name("file_read"));
        assert!(!is_mutating_tool_name("agent_done"));
    }

    #[test]
    fn build_subagent_review_with_diff_carries_workspace_and_diff() {
        let payload = serde_json::json!({
            "summary": "added function foo",
            "workspace": "/tmp/sub-1",
            "diff": "+++ src/lib.rs\n+fn foo() {}\n",
        });
        let review = build_subagent_review(&payload);
        assert!(review.contains("[sub-agent review]"));
        assert!(review.contains("added function foo"));
        assert!(review.contains("/tmp/sub-1"));
        assert!(review.contains("fn foo()"));
        assert!(review.contains("do not trust the summary"));
    }

    #[test]
    fn build_subagent_review_without_diff_still_warns_to_inspect() {
        let payload = serde_json::json!({
            "summary": "task complete",
            "workspace": "/tmp/sub-2",
            "diff": "",
        });
        let review = build_subagent_review(&payload);
        assert!(review.contains("[sub-agent review]"));
        assert!(review.contains("No diff captured"));
        assert!(review.contains("/tmp/sub-2"));
    }

    #[test]
    fn take_pending_resume_returns_none_when_file_missing() {
        let path = std::env::temp_dir().join(format!(
            "peridot-pending-missing-{}.bin",
            std::process::id()
        ));
        // Ensure the file does not exist.
        let _ = std::fs::remove_file(&path);
        assert!(take_pending_resume(Some(&path)).is_none());
    }

    #[test]
    fn take_pending_resume_consumes_valid_sidecar() {
        let path =
            std::env::temp_dir().join(format!("peridot-pending-valid-{}.bin", std::process::id()));
        let call = ToolCall::new(
            "shell_exec",
            serde_json::json!({ "command": "npm install left-pad" }),
        );
        std::fs::write(&path, serde_json::to_vec(&call).unwrap()).unwrap();
        let recovered = take_pending_resume(Some(&path)).expect("recovered");
        assert_eq!(recovered.name, "shell_exec");
        assert_eq!(
            recovered.parameters.get("command").and_then(|v| v.as_str()),
            Some("npm install left-pad")
        );
        assert!(
            !path.exists(),
            "sidecar should be deleted after consumption"
        );
    }

    #[test]
    fn take_pending_resume_handles_unparseable_payload() {
        let path = std::env::temp_dir().join(format!(
            "peridot-pending-garbage-{}.bin",
            std::process::id()
        ));
        std::fs::write(&path, b"not json at all").unwrap();
        assert!(take_pending_resume(Some(&path)).is_none());
        // garbage file is left on disk for the operator to inspect
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn write_file_checkpoint_captures_previous_file_content() {
        let root =
            std::env::temp_dir().join(format!("peridot-file-checkpoint-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "old").unwrap();

        let id = write_file_checkpoint(
            &root,
            "file_patch",
            &serde_json::json!({"path": "src/lib.rs"}),
        )
        .unwrap()
        .unwrap();
        let checkpoint =
            std::fs::read_to_string(root.join(".peridot/checkpoints").join(format!("{id}.json")))
                .unwrap();
        let value: serde_json::Value = serde_json::from_str(&checkpoint).unwrap();

        assert_eq!(value["path"], "src/lib.rs");
        assert_eq!(value["existed"], true);
        assert_eq!(value["previous_content"], "old");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn verify_failure_state_repeats_same_signature_and_resets_on_new_one() {
        let mut state = None;
        let first = AgentTurnOutcome {
            tool_name: "verify_build".to_string(),
            tool_result: ToolResult::failure("error[E0425]: cannot find value `x`"),
            usage: Usage::default(),
            done: false,
        };
        let repeated = update_verify_failure_state(&mut state, &first).clone();
        assert_eq!(repeated.attempts, 1);
        let repeated = update_verify_failure_state(&mut state, &first).clone();
        assert_eq!(repeated.attempts, 2);

        let changed = AgentTurnOutcome {
            tool_name: "verify_build".to_string(),
            tool_result: ToolResult::failure("error[E0308]: mismatched types"),
            usage: Usage::default(),
            done: false,
        };
        let changed = update_verify_failure_state(&mut state, &changed).clone();
        assert_eq!(changed.attempts, 1);
        assert!(changed.signature.contains("E0308"));
    }

    #[test]
    fn verify_failure_directive_mentions_repeated_signature() {
        let failure = VerifyFailureState {
            tool_name: "verify_test".to_string(),
            signature: "test foo failed".to_string(),
            attempts: 2,
        };
        let directive = verify_failure_directive(&failure, 3);
        assert!(directive.contains("2/3"));
        assert!(directive.contains("same verifier failure signature"));
        assert!(directive.contains("re-run `verify_test`"));
    }

    #[test]
    fn build_subagent_review_truncates_giant_diffs() {
        let big_diff = "a".repeat(8000);
        let payload = serde_json::json!({
            "summary": "",
            "workspace": "/tmp/sub-3",
            "diff": big_diff,
        });
        let review = build_subagent_review(&payload);
        assert!(review.contains("diff truncated"));
        assert!(review.chars().count() < 6000);
    }
}
