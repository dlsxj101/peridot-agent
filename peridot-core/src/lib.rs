//! Core harness state and high-level agent orchestration.

mod agent;
mod cancel;
mod goal;
mod permissions;
mod prompt;
mod recovery;
mod requests;
mod slash;
mod state;
#[cfg(test)]
mod tests;
mod usage;

pub use agent::HarnessAgent;
pub use cancel::CancelToken;
pub use goal::{GoalController, GoalStatus};
pub use permissions::allowed_tool_groups;
pub use requests::{
    AgentRunEvent, AgentRunRequest, AgentRunSummary, AgentTurnOutcome, AgentTurnRequest,
    McpStatusUpdate, PlanStepUpdate, StopReason,
};
pub use slash::{SlashCommand, parse_slash_command};
pub use state::AgentState;
