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
        let tool = self
            .tools
            .get(&call.name)
            .ok_or_else(|| PeriError::Tool(format!("unknown tool: {}", call.name)))?;
        let ctx = ToolContext::new(project_root, self.state.permission);
        tool.validate_params(&call.parameters)?;
        tool.execute(call.parameters, &ctx).await
    }
}
