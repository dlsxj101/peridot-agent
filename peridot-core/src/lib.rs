//! Core harness state and high-level agent orchestration.

mod agent;
mod goal;
mod grader;
mod permissions;
mod prompt;
mod recovery;
mod requests;
mod role;
mod slash;
mod state;
#[cfg(test)]
mod tests;
mod usage;

pub use agent::HarnessAgent;
pub use peridot_common::CancelToken;
pub use goal::{GoalController, GoalStatus};
pub use grader::{GraderVerdict, grade_work};
pub use permissions::allowed_tool_groups;
pub use requests::{
    AgentRunEvent, AgentRunRequest, AgentRunSummary, AgentTurnOutcome, AgentTurnRequest,
    McpStatusUpdate, PlanStepUpdate, ReviewerVerdict, StopReason,
};
pub use role::AgentRole;
pub use slash::{SlashCommand, SubagentModelChange, parse_slash_command};
pub use state::AgentState;
pub use usage::accumulate_usage;
