use peridot_common::{AgentPhase, ExecutionMode, PermissionMode};
use serde::{Deserialize, Serialize};

use crate::SlashCommand;

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
            | SlashCommand::GoalStatus
            | SlashCommand::Clear
            | SlashCommand::Help
            | SlashCommand::Cost
            | SlashCommand::PlanShow
            | SlashCommand::Model(_)
            | SlashCommand::Provider(_)
            | SlashCommand::Committee(_)
            | SlashCommand::Note(_)
            | SlashCommand::Info
            | SlashCommand::Compact
            | SlashCommand::SessionSave
            | SlashCommand::Diff
            | SlashCommand::Undo
            | SlashCommand::Lang(_)
            | SlashCommand::Fork(_)
            | SlashCommand::Teammate(_)
            | SlashCommand::Worktree { .. }
            | SlashCommand::SessionNew(_)
            | SlashCommand::SessionSwitch(_)
            | SlashCommand::SessionClose(_)
            | SlashCommand::SessionList
            | SlashCommand::SubagentModel(_)
            | SlashCommand::Reasoning(_)
            | SlashCommand::McpList
            | SlashCommand::McpAdd { .. }
            | SlashCommand::McpRemove(_)
            | SlashCommand::McpTest(_)
            | SlashCommand::Todos
            | SlashCommand::Rewind
            | SlashCommand::BranchSave(_)
            | SlashCommand::BranchRestore(_)
            | SlashCommand::BranchList
            | SlashCommand::BranchTurn(_)
            | SlashCommand::BranchPicker => {}
        }
    }
}

impl Default for AgentState {
    fn default() -> Self {
        Self::new(ExecutionMode::default(), PermissionMode::default())
    }
}
