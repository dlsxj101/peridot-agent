use std::path::PathBuf;

use peridot_common::{
    AgentPhase, ExecutionMode, PeriError, PeriResult, SecurityConfig, ToolCall, ToolResult,
};
use peridot_context::{ContextEntry, ContextManager, ContextSource};
use peridot_llm::{CompletionRequest, LlmProvider, Usage, parse_action};
use peridot_tools::audit::{AuditEvent, append_audit_event};
use peridot_tools::hooks::{HookRunner, tool_hook_variables};
use peridot_tools::{ToolContext, ToolRegistry};

use crate::cancel::CancelToken;
use crate::goal::check_goal_satisfied;
use crate::permissions::ensure_tool_allowed;
use crate::prompt::{read_plan_reminder, system_prompt_for_role};
use crate::recovery::{
    StuckDetector, budget_exceeded_message, budget_warning_message, classify_error,
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
        }
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
        let ctx = ToolContext::new(project_root.clone(), self.state.permission)
            .with_denied_paths(denied_paths)
            .with_hooks(hooks)
            .with_security(security);
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
        if let Some(user_input) = request.user_input {
            self.context
                .append(ContextEntry::trusted(ContextSource::User, user_input));
        }
        if let Some(plan) = read_plan_reminder(&request.project_root) {
            self.context
                .append(ContextEntry::trusted(ContextSource::PlanReminder, plan));
        }
        let estimated_tokens = self.context.estimated_tokens();
        if self.context.compact_if_needed() {
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
        let completion = stream_completion_with_chunks(
            provider,
            CompletionRequest {
                model: request.model,
                system: Some(system_prompt_for_role(self.state.mode, self.role)),
                messages: self.context.to_messages(),
                max_tokens: Some(request.max_tokens),
                thinking: self.state.mode == ExecutionMode::Goal,
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
        self.context.append(ContextEntry::trusted(
            ContextSource::Assistant,
            completion.text.clone(),
        ));

        let parsed = parse_action(&completion.text)?;
        if let Some(thinking) = parsed.thinking.as_ref()
            && !thinking.trim().is_empty()
        {
            events(AgentRunEvent::Thinking {
                text: thinking.clone(),
            });
        }
        let tool_name = parsed.tool_call.name.clone();
        let tool_parameters = parsed.tool_call.parameters.clone();
        self.state.phase = AgentPhase::Executing;
        events(AgentRunEvent::ToolStarted {
            name: tool_name.clone(),
            parameters: tool_parameters.clone(),
        });
        let tool_result = match self
            .execute_tool_call_with_runtime(
                parsed.tool_call,
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
        events(AgentRunEvent::ToolFinished {
            name: tool_name.clone(),
            result: tool_result.clone(),
        });
        self.context
            .append_observation(serde_json::to_string(&tool_result).map_err(|err| {
                PeriError::Parse(format!("failed to serialize tool result: {err}"))
            })?)?;

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
        let mut outcomes = Vec::new();
        let mut total_usage = Usage::default();
        let mut stuck_detector = StuckDetector::new(3);
        let mut budget_warning_sent = false;
        let mut consecutive_parse_failures = 0usize;
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
            accumulate_usage(&mut total_usage, outcome.usage);
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
            let recovery = stuck_detector.record(&outcome);
            outcomes.push(outcome);
            if let Some(message) = recovery {
                self.state.phase = AgentPhase::Recovering;
                run_recovery_event_hook(&request.project_root, &request.hooks, "stuck", &message)?;
                self.context
                    .append(ContextEntry::trusted(ContextSource::PlanReminder, message));
                events(AgentRunEvent::Recovery {
                    message: "stuck detector requested a new strategy".to_string(),
                });
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
                let summary = AgentRunSummary {
                    turns: outcomes,
                    usage: total_usage,
                    stopped_reason: StopReason::Done,
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
