use peridot_common::{AgentPhase, ExecutionMode, PermissionMode};
use serde::{Deserialize, Serialize};

use crate::{SlashCommand, slash_state_delta};

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
        let delta = slash_state_delta(command, None);
        if let Some(mode) = delta.mode {
            self.mode = mode;
        }
        if let Some(permission) = delta.permission {
            self.permission = permission;
        }
        match command {
            SlashCommand::GoalStart(goal) => {
                self.goal = Some(goal.clone());
            }
            SlashCommand::Skill { .. }
            | SlashCommand::SkillList
            | SlashCommand::SkillShow(_)
            | SlashCommand::SkillSearch(_)
            | SlashCommand::SkillArchived(_)
            | SlashCommand::SkillPin(_)
            | SlashCommand::SkillUnpin(_)
            | SlashCommand::SkillArchive(_)
            | SlashCommand::SkillRestore(_)
            | SlashCommand::Plan
            | SlashCommand::Execute
            | SlashCommand::GoalMode
            | SlashCommand::Safe
            | SlashCommand::Auto
            | SlashCommand::Yolo
            | SlashCommand::GoalPause
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
            | SlashCommand::Notes(_)
            | SlashCommand::Info
            | SlashCommand::ContextTop
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
            | SlashCommand::SessionDelete(_)
            | SlashCommand::SessionRename { .. }
            | SlashCommand::SessionList
            | SlashCommand::SessionCount
            | SlashCommand::SubagentModel(_)
            | SlashCommand::Reasoning(_)
            | SlashCommand::Fast(_)
            | SlashCommand::McpList
            | SlashCommand::McpAdd { .. }
            | SlashCommand::McpRemove(_)
            | SlashCommand::McpTest(_)
            | SlashCommand::Todos
            | SlashCommand::CodeMap
            | SlashCommand::CodeMapStatus
            | SlashCommand::CodeMapRefresh
            | SlashCommand::CodeMapFind(_)
            | SlashCommand::CodeMapLocate(_)
            | SlashCommand::CodeMapOutline(_)
            | SlashCommand::CodeMapRefs(_)
            | SlashCommand::Attachments
            | SlashCommand::Attach(_)
            | SlashCommand::Detach(_)
            | SlashCommand::Export(_)
            | SlashCommand::Rewind
            | SlashCommand::BranchSave(_)
            | SlashCommand::BranchRestore(_)
            | SlashCommand::BranchList
            | SlashCommand::BranchTurn(_)
            | SlashCommand::BranchTree
            | SlashCommand::BranchSwitch(_)
            | SlashCommand::BranchPicker
            | SlashCommand::SidepanelToggle
            | SlashCommand::Collapse
            | SlashCommand::AutoFix(_) => {}
        }
    }
}

impl Default for AgentState {
    fn default() -> Self {
        Self::new(ExecutionMode::default(), PermissionMode::default())
    }
}
