//! Core harness state and high-level agent orchestration.

use std::path::PathBuf;

use peridot_common::{
    AgentPhase, ExecutionMode, PeriError, PeriResult, PermissionMode, ToolCall, ToolResult,
};
use peridot_context::ContextManager;
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
        let tool = self
            .tools
            .get(&call.name)
            .ok_or_else(|| PeriError::Tool(format!("unknown tool: {}", call.name)))?;
        let ctx =
            ToolContext::new(project_root, self.state.permission).with_denied_paths(denied_paths);
        tool.validate_params(&call.parameters)?;
        tool.execute(call.parameters, &ctx).await
    }
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
}
