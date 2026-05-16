use std::path::PathBuf;

use peridot_common::{
    AgentPhase, ExecutionMode, PeriError, PeriResult, SecurityConfig, ToolCall, ToolResult,
};
use peridot_context::{ContextEntry, ContextManager, ContextSource};
use peridot_llm::{CompletionRequest, LlmProvider, Usage, parse_action};
use peridot_tools::audit::{AuditEvent, append_audit_event};
use peridot_tools::hooks::{HookRunner, tool_hook_variables};
use peridot_tools::{ToolContext, ToolRegistry};

use crate::goal::check_goal_satisfied;
use crate::permissions::ensure_tool_allowed;
use crate::prompt::{read_plan_reminder, system_prompt_for_mode};
use crate::recovery::{
    StuckDetector, budget_exceeded_message, budget_warning_message, classify_error,
    format_reminder_message, recovery_message, run_budget_warning_hook, run_context_compacted_hook,
    run_error_event_hooks, run_recovery_event_hook, should_emit_budget_warning,
};
use crate::requests::{
    AgentRunEvent, AgentRunRequest, AgentRunSummary, AgentTurnOutcome, AgentTurnRequest, StopReason,
};
use crate::state::AgentState;
use crate::usage::{accumulate_usage, stream_completion_with_chunks};

/// Peridot harness agent shell.
pub struct HarnessAgent {
    state: AgentState,
    context: ContextManager,
    tools: ToolRegistry,
}

impl HarnessAgent {
    /// Creates a harness agent from state and dependencies.
    pub fn new(state: AgentState, context: ContextManager, tools: ToolRegistry) -> Self {
        Self {
            state,
            context,
            tools,
        }
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
                system: Some(system_prompt_for_mode(self.state.mode)),
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
            accumulate_usage(&mut total_usage, outcome.usage);
            events(AgentRunEvent::UsageUpdated { usage: total_usage });
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

fn approval_required_error(err: &PeriError) -> bool {
    match err {
        PeriError::PermissionDenied(reason) => reason.contains("requires explicit user approval"),
        _ => false,
    }
}
