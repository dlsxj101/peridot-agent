//! Core harness state and high-level agent orchestration.

use std::path::PathBuf;

use peridot_common::{
    AgentPhase, ExecutionMode, HooksConfig, PeriError, PeriResult, PermissionMode, ToolCall,
    ToolGroup, ToolResult,
};
use peridot_context::{ContextEntry, ContextManager, ContextSource};
use peridot_llm::{CompletionRequest, LlmProvider, Usage, parse_action};
use peridot_tools::audit::{AuditEvent, append_audit_event};
use peridot_tools::hooks::{HookRunner, tool_hook_variables};
use peridot_tools::{ToolContext, ToolRegistry};
use serde::{Deserialize, Serialize};

/// Current runtime state of the harness agent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgentState {
    /// Execution mode.
    pub mode: ExecutionMode,
    /// Permission mode.
    pub permission: PermissionMode,
    /// Current state-machine phase.
    pub phase: AgentPhase,
    /// Optional durable goal objective.
    pub goal: Option<String>,
}

impl AgentState {
    /// Creates a new agent state.
    pub fn new(mode: ExecutionMode, permission: PermissionMode) -> Self {
        Self {
            mode,
            permission,
            phase: AgentPhase::Planning,
            goal: None,
        }
    }

    /// Attaches a durable goal objective to the state.
    pub fn with_goal(mut self, goal: impl Into<String>) -> Self {
        self.goal = Some(goal.into());
        self.mode = ExecutionMode::Goal;
        self
    }

    /// Applies a parsed slash command to this state when it affects mode or permission.
    pub fn apply_slash_command(&mut self, command: &SlashCommand) {
        match command {
            SlashCommand::Plan => self.mode = ExecutionMode::Plan,
            SlashCommand::Execute => self.mode = ExecutionMode::Execute,
            SlashCommand::GoalStart(goal) => {
                self.mode = ExecutionMode::Goal;
                self.goal = Some(goal.clone());
            }
            SlashCommand::Safe => self.permission = PermissionMode::Safe,
            SlashCommand::Auto => self.permission = PermissionMode::Auto,
            SlashCommand::Yolo => self.permission = PermissionMode::Yolo,
            SlashCommand::GoalPause
            | SlashCommand::GoalResume
            | SlashCommand::GoalClear
            | SlashCommand::GoalStatus => {}
        }
    }
}

impl Default for AgentState {
    fn default() -> Self {
        Self::new(ExecutionMode::default(), PermissionMode::default())
    }
}

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
            HooksConfig::default(),
        )
        .await
    }

    /// Executes one tool call with explicit boundaries and hook configuration.
    pub async fn execute_tool_call_with_runtime(
        &self,
        call: ToolCall,
        project_root: impl Into<PathBuf>,
        denied_paths: Vec<PathBuf>,
        hooks: HooksConfig,
    ) -> PeriResult<ToolResult> {
        let tool = self
            .tools
            .get(&call.name)
            .ok_or_else(|| PeriError::Tool(format!("unknown tool: {}", call.name)))?;
        ensure_tool_allowed(self.state.mode, self.state.phase, tool.group(), &call.name)?;
        let project_root = project_root.into();
        let ctx = ToolContext::new(project_root.clone(), self.state.permission)
            .with_denied_paths(denied_paths)
            .with_hooks(hooks);
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
        if let Some(user_input) = request.user_input {
            self.context
                .append(ContextEntry::trusted(ContextSource::User, user_input));
        }

        let completion = provider
            .complete(CompletionRequest {
                model: request.model,
                system: Some(system_prompt_for_mode(self.state.mode)),
                messages: self.context.to_messages(),
                max_tokens: Some(request.max_tokens),
                thinking: self.state.mode == ExecutionMode::Goal,
            })
            .await?;
        self.context.append(ContextEntry::trusted(
            ContextSource::Assistant,
            completion.text.clone(),
        ));

        let parsed = parse_action(&completion.text)?;
        let tool_name = parsed.tool_call.name.clone();
        self.state.phase = AgentPhase::Executing;
        let tool_result = self
            .execute_tool_call_with_runtime(
                parsed.tool_call,
                request.project_root,
                request.denied_paths,
                request.hooks,
            )
            .await?;
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
        let mut outcomes = Vec::new();
        let mut total_usage = Usage::default();
        for turn_index in 0..request.max_turns {
            let outcome = self
                .run_turn(
                    provider,
                    AgentTurnRequest {
                        user_input: (turn_index == 0).then(|| request.task.clone()),
                        model: request.model.clone(),
                        max_tokens: request.max_tokens,
                        project_root: request.project_root.clone(),
                        denied_paths: request.denied_paths.clone(),
                        hooks: request.hooks.clone(),
                    },
                )
                .await?;
            accumulate_usage(&mut total_usage, outcome.usage);
            let done = outcome.done;
            outcomes.push(outcome);
            if done {
                return Ok(AgentRunSummary {
                    turns: outcomes,
                    usage: total_usage,
                    stopped_reason: StopReason::Done,
                });
            }
            if request.budget_usd > 0.0 && total_usage.estimated_cost_usd >= request.budget_usd {
                return Ok(AgentRunSummary {
                    turns: outcomes,
                    usage: total_usage,
                    stopped_reason: StopReason::Budget,
                });
            }
        }

        Ok(AgentRunSummary {
            turns: outcomes,
            usage: total_usage,
            stopped_reason: StopReason::MaxTurns,
        })
    }
}

fn ensure_tool_allowed(
    mode: ExecutionMode,
    phase: AgentPhase,
    group: ToolGroup,
    name: &str,
) -> PeriResult<()> {
    if mode == ExecutionMode::Plan {
        let allowed = matches!(
            group,
            ToolGroup::File | ToolGroup::Git | ToolGroup::Plan | ToolGroup::Agent | ToolGroup::Web
        ) && !matches!(name, "file_write" | "file_patch" | "shell_exec");
        if !allowed {
            return Err(PeriError::PermissionDenied(format!(
                "Plan mode blocks tool {name}"
            )));
        }
    }

    if phase == AgentPhase::Verifying {
        let allowed = matches!(group, ToolGroup::Verify | ToolGroup::File | ToolGroup::Plan);
        if !allowed {
            return Err(PeriError::PermissionDenied(format!(
                "Verifying phase blocks tool {name}"
            )));
        }
    }

    Ok(())
}

/// Returns the tool groups available for a state.
pub fn allowed_tool_groups(mode: ExecutionMode, phase: AgentPhase) -> Vec<ToolGroup> {
    if mode == ExecutionMode::Plan {
        return vec![
            ToolGroup::File,
            ToolGroup::Git,
            ToolGroup::Plan,
            ToolGroup::Agent,
            ToolGroup::Web,
        ];
    }
    if phase == AgentPhase::Verifying {
        return vec![ToolGroup::Verify, ToolGroup::File, ToolGroup::Plan];
    }
    vec![
        ToolGroup::Shell,
        ToolGroup::File,
        ToolGroup::Git,
        ToolGroup::Web,
        ToolGroup::Plan,
        ToolGroup::Verify,
        ToolGroup::Agent,
        ToolGroup::Mcp,
    ]
}

fn system_prompt_for_mode(mode: ExecutionMode) -> String {
    format!(
        "You are Peridot Agent running in {mode} mode. Respond with JSON containing action and parameters."
    )
}

fn accumulate_usage(total: &mut Usage, usage: Usage) {
    total.input_tokens += usage.input_tokens;
    total.output_tokens += usage.output_tokens;
    total.cache_read_tokens += usage.cache_read_tokens;
    total.cache_creation_tokens += usage.cache_creation_tokens;
    total.estimated_cost_usd += usage.estimated_cost_usd;
}

/// Request for one agent turn.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentTurnRequest {
    /// Optional user input to append before the turn.
    pub user_input: Option<String>,
    /// Model name.
    pub model: String,
    /// Maximum output tokens.
    pub max_tokens: u32,
    /// Project root.
    pub project_root: PathBuf,
    /// Denied path prefixes.
    pub denied_paths: Vec<PathBuf>,
    /// Active hook definitions.
    pub hooks: HooksConfig,
}

/// Outcome of one agent turn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentTurnOutcome {
    /// Tool name that was invoked.
    pub tool_name: String,
    /// Tool result.
    pub tool_result: ToolResult,
    /// Provider usage for the turn.
    pub usage: Usage,
    /// Whether the task is complete.
    pub done: bool,
}

/// Request for a bounded agent run.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentRunRequest {
    /// Initial task.
    pub task: String,
    /// Model name.
    pub model: String,
    /// Maximum number of turns.
    pub max_turns: u32,
    /// Maximum output tokens per turn.
    pub max_tokens: u32,
    /// Maximum estimated cost for the run. Values <= 0 disable budget stopping.
    pub budget_usd: f64,
    /// Project root.
    pub project_root: PathBuf,
    /// Denied path prefixes.
    pub denied_paths: Vec<PathBuf>,
    /// Active hook definitions.
    pub hooks: HooksConfig,
}

/// Reason a bounded run stopped.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum StopReason {
    /// The agent called agent_done.
    Done,
    /// The run hit max turns.
    MaxTurns,
    /// The run hit its configured cost budget.
    Budget,
}

/// Summary of a bounded agent run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentRunSummary {
    /// Turn outcomes.
    pub turns: Vec<AgentTurnOutcome>,
    /// Aggregated usage.
    pub usage: Usage,
    /// Stop reason.
    pub stopped_reason: StopReason,
}

/// Slash commands supported by Peridot's interactive surfaces.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SlashCommand {
    /// Switch to plan mode.
    Plan,
    /// Switch to execute mode.
    Execute,
    /// Start goal mode with an objective.
    GoalStart(String),
    /// Pause goal execution.
    GoalPause,
    /// Resume goal execution.
    GoalResume,
    /// Clear the active goal.
    GoalClear,
    /// Show goal status.
    GoalStatus,
    /// Switch to safe permission mode.
    Safe,
    /// Switch to auto permission mode.
    Auto,
    /// Switch to yolo permission mode.
    Yolo,
}

/// Parses a user slash command.
pub fn parse_slash_command(input: &str) -> Option<SlashCommand> {
    let input = input.trim();
    let body = input.strip_prefix('/')?;
    let mut parts = body.splitn(2, char::is_whitespace);
    let command = parts.next()?.trim();
    let rest = parts.next().unwrap_or("").trim();

    match command {
        "plan" if rest.is_empty() => Some(SlashCommand::Plan),
        "execute" if rest.is_empty() => Some(SlashCommand::Execute),
        "safe" if rest.is_empty() => Some(SlashCommand::Safe),
        "auto" if rest.is_empty() => Some(SlashCommand::Auto),
        "yolo" if rest.is_empty() => Some(SlashCommand::Yolo),
        "goal" => match rest {
            "pause" => Some(SlashCommand::GoalPause),
            "resume" => Some(SlashCommand::GoalResume),
            "clear" => Some(SlashCommand::GoalClear),
            "status" => Some(SlashCommand::GoalStatus),
            "" => None,
            goal => Some(SlashCommand::GoalStart(goal.to_string())),
        },
        _ => None,
    }
}

/// Goal execution status.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum GoalStatus {
    /// Goal is actively running.
    Running,
    /// Goal is paused.
    Paused,
    /// Goal completed.
    Done,
    /// Goal was cleared.
    Cleared,
}

/// Runtime guardrails for a goal-mode task.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoalController {
    /// Durable objective text.
    pub objective: String,
    /// Current goal status.
    pub status: GoalStatus,
    /// Maximum turn count.
    pub max_turns: u32,
    /// Current turn count.
    pub turns_used: u32,
    /// Budget cap in USD.
    pub budget_usd: f64,
    /// Current cost in USD.
    pub cost_usd: f64,
}

impl GoalController {
    /// Creates a running goal controller.
    pub fn new(objective: impl Into<String>, max_turns: u32, budget_usd: f64) -> Self {
        Self {
            objective: objective.into(),
            status: GoalStatus::Running,
            max_turns,
            turns_used: 0,
            budget_usd,
            cost_usd: 0.0,
        }
    }

    /// Applies a goal-specific slash command.
    pub fn apply(&mut self, command: &SlashCommand) {
        match command {
            SlashCommand::GoalPause => self.status = GoalStatus::Paused,
            SlashCommand::GoalResume => self.status = GoalStatus::Running,
            SlashCommand::GoalClear => self.status = GoalStatus::Cleared,
            _ => {}
        }
    }

    /// Records one completed turn and added cost.
    pub fn record_turn(&mut self, cost_usd: f64) {
        self.turns_used += 1;
        self.cost_usd += cost_usd;
    }

    /// Returns true when a guardrail requires stopping.
    pub fn should_stop(&self) -> bool {
        matches!(
            self.status,
            GoalStatus::Paused | GoalStatus::Done | GoalStatus::Cleared
        ) || self.turns_used >= self.max_turns
            || self.cost_usd >= self.budget_usd
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use peridot_common::{HookConfig, HookFailureMode};
    use peridot_llm::{AuthMethod, CompletionResponse, PricingTable};
    use peridot_tools::register_builtin_tools;
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;

    #[test]
    fn parses_goal_slash_commands() {
        assert_eq!(
            parse_slash_command("/goal fix tests"),
            Some(SlashCommand::GoalStart("fix tests".to_string()))
        );
        assert_eq!(
            parse_slash_command("/goal pause"),
            Some(SlashCommand::GoalPause)
        );
        assert_eq!(parse_slash_command("/safe"), Some(SlashCommand::Safe));
    }

    #[test]
    fn goal_controller_stops_on_budget() {
        let mut goal = GoalController::new("finish", 10, 1.0);
        assert!(!goal.should_stop());

        goal.record_turn(1.2);

        assert!(goal.should_stop());
    }

    #[test]
    fn agent_state_applies_mode_commands() {
        let mut state = AgentState::default();
        state.apply_slash_command(&SlashCommand::GoalStart("ship".to_string()));

        assert_eq!(state.mode, ExecutionMode::Goal);
        assert_eq!(state.goal.as_deref(), Some("ship"));
    }

    struct StaticProvider {
        responses: Mutex<Vec<String>>,
        cost_usd: f64,
    }

    impl StaticProvider {
        fn new(responses: Vec<String>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().rev().collect()),
                cost_usd: 0.0,
            }
        }

        fn with_cost(responses: Vec<String>, cost_usd: f64) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().rev().collect()),
                cost_usd,
            }
        }
    }

    #[async_trait]
    impl LlmProvider for StaticProvider {
        async fn complete(&self, _request: CompletionRequest) -> PeriResult<CompletionResponse> {
            let text = self
                .responses
                .lock()
                .unwrap()
                .pop()
                .ok_or_else(|| PeriError::Provider("no response".to_string()))?;
            Ok(CompletionResponse {
                text,
                usage: Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_read_tokens: 0,
                    cache_creation_tokens: 0,
                    estimated_cost_usd: self.cost_usd,
                },
            })
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

        fn auth_method(&self) -> AuthMethod {
            AuthMethod::NotConfigured
        }
    }

    #[tokio::test]
    async fn run_until_done_executes_tools_and_stops() {
        let root = std::env::temp_dir().join(format!("peridot-core-loop-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut registry = ToolRegistry::new();
        register_builtin_tools(&mut registry).unwrap();
        let mut agent = HarnessAgent::new(
            AgentState::new(ExecutionMode::Execute, PermissionMode::Auto),
            ContextManager::new(),
            registry,
        );
        let provider = StaticProvider::new(vec![
            json!({
                "action": "file_write",
                "parameters": {"path": "loop.txt", "content": "ok\n"}
            })
            .to_string(),
            json!({
                "action": "agent_done",
                "parameters": {"summary": "finished"}
            })
            .to_string(),
        ]);

        let summary = agent
            .run_until_done(
                &provider,
                AgentRunRequest {
                    task: "write loop.txt".to_string(),
                    model: "mock".to_string(),
                    max_turns: 4,
                    max_tokens: 512,
                    budget_usd: 5.0,
                    project_root: root.clone(),
                    denied_paths: Vec::new(),
                    hooks: HooksConfig::default(),
                },
            )
            .await
            .unwrap();

        assert_eq!(summary.stopped_reason, StopReason::Done);
        assert_eq!(summary.turns.len(), 2);
        assert_eq!(
            std::fs::read_to_string(root.join("loop.txt")).unwrap(),
            "ok\n"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn plan_mode_blocks_file_write() {
        let root = std::env::temp_dir().join(format!("peridot-core-plan-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut registry = ToolRegistry::new();
        register_builtin_tools(&mut registry).unwrap();
        let agent = HarnessAgent::new(
            AgentState::new(ExecutionMode::Plan, PermissionMode::Auto),
            ContextManager::new(),
            registry,
        );

        let result = agent
            .execute_tool_call(
                ToolCall::new("file_write", json!({"path":"blocked.txt","content":"nope"})),
                &root,
            )
            .await;

        assert!(matches!(result, Err(PeriError::PermissionDenied(_))));
        assert!(!root.join("blocked.txt").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn tool_hooks_wrap_execution() {
        let root = std::env::temp_dir().join(format!("peridot-core-hooks-{}", std::process::id()));
        let hooks_dir = root.join(".peridot/hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        let script = hooks_dir.join("mark.sh");
        std::fs::write(&script, "#!/bin/sh\necho $1 >> hook.log\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut registry = ToolRegistry::new();
        register_builtin_tools(&mut registry).unwrap();
        let agent = HarnessAgent::new(
            AgentState::new(ExecutionMode::Execute, PermissionMode::Auto),
            ContextManager::new(),
            registry,
        );

        agent
            .execute_tool_call_with_runtime(
                ToolCall::new("file_write", json!({"path":"hooked.txt","content":"ok"})),
                &root,
                Vec::new(),
                HooksConfig {
                    tool: vec![HookConfig {
                        event: "pre:file_write".to_string(),
                        run: ".peridot/hooks/mark.sh {path}".to_string(),
                        description: None,
                        on_failure: HookFailureMode::Block,
                        only_paths: Vec::new(),
                    }],
                    ..HooksConfig::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(root.join("hook.log")).unwrap(),
            "hooked.txt\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("hooked.txt")).unwrap(),
            "ok"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn run_until_done_stops_on_budget() {
        let root = std::env::temp_dir().join(format!("peridot-core-budget-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut registry = ToolRegistry::new();
        register_builtin_tools(&mut registry).unwrap();
        let mut agent = HarnessAgent::new(
            AgentState::new(ExecutionMode::Execute, PermissionMode::Auto),
            ContextManager::new(),
            registry,
        );
        let provider = StaticProvider::with_cost(
            vec![json!({"action":"plan_update","parameters":{"update":"one"}}).to_string()],
            0.25,
        );

        let summary = agent
            .run_until_done(
                &provider,
                AgentRunRequest {
                    task: "spend budget".to_string(),
                    model: "mock".to_string(),
                    max_turns: 4,
                    max_tokens: 512,
                    budget_usd: 0.1,
                    project_root: root.clone(),
                    denied_paths: Vec::new(),
                    hooks: HooksConfig::default(),
                },
            )
            .await
            .unwrap();

        assert_eq!(summary.stopped_reason, StopReason::Budget);
        assert_eq!(summary.turns.len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }
}
