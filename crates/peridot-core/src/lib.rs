//! Core harness state and high-level agent orchestration.

mod agent;
mod agent_helpers;
mod complexity;
mod goal;
mod grader;
mod inner_loop;
mod loop_policy;
mod permissions;
mod prompt;
mod recovery;
mod requests;
mod role;
mod slash;
mod state;
#[cfg(test)]
#[cfg(test)]
mod tests;
mod usage;

pub use agent::HarnessAgent;
pub use complexity::{TaskComplexity, classify_task_complexity};
pub use goal::{GoalController, GoalStatus};
pub use grader::{GraderVerdict, grade_work};
pub use inner_loop::InnerLoopSubAgent;
pub use peridot_common::CancelToken;
pub use permissions::allowed_tool_groups;
pub use requests::{
    AGENT_RUN_EVENT_SCHEMA_VERSION, AgentRunEvent, AgentRunRequest, AgentRunSummary,
    AgentTurnOutcome, AgentTurnRequest, FileDiffPayload, McpStatusUpdate, PlanStepUpdate,
    ReviewerVerdict, StopReason,
};
pub use role::AgentRole;
pub use slash::{
    AutoFixAction, SlashCommand, SlashStateDelta, SubagentModelChange, parse_slash_command,
    slash_state_delta,
};
pub use state::AgentState;
pub use usage::accumulate_usage;
