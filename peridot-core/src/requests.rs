use std::path::PathBuf;

use peridot_common::{HooksConfig, SecurityConfig, ToolResult};
use peridot_llm::Usage;
use serde::{Deserialize, Serialize};

/// User-interface event emitted by the harness run loop.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentRunEvent {
    /// A bounded run started.
    RunStarted {
        /// Initial task text.
        task: String,
    },
    /// A model/tool turn started.
    TurnStarted {
        /// Zero-based turn index.
        turn_index: u32,
    },
    /// Assistant streaming began.
    AssistantStarted {
        /// Stream label for display.
        label: String,
    },
    /// Assistant text streamed.
    AssistantDelta {
        /// Text delta.
        delta: String,
    },
    /// Assistant output completed.
    AssistantFinished {
        /// Full assistant text.
        text: String,
    },
    /// Parsed model thinking text.
    Thinking {
        /// Thinking text.
        text: String,
    },
    /// Tool execution started.
    ToolStarted {
        /// Tool name.
        name: String,
        /// Tool parameters.
        parameters: serde_json::Value,
    },
    /// Tool execution finished.
    ToolFinished {
        /// Tool name.
        name: String,
        /// Tool result.
        result: ToolResult,
    },
    /// A tool needs explicit user approval before it can proceed.
    ApprovalRequested {
        /// Tool name.
        tool_name: String,
        /// Human-readable reason.
        reason: String,
        /// Parameters the tool was about to execute with.
        #[serde(default)]
        parameters: serde_json::Value,
    },
    /// Usage totals changed.
    UsageUpdated {
        /// Aggregated run usage.
        usage: Usage,
    },
    /// The harness entered recovery.
    Recovery {
        /// Recovery message.
        message: String,
    },
    /// The bounded run finished.
    Finished {
        /// Final run summary.
        summary: AgentRunSummary,
    },
    /// The run summary was saved for later resume.
    SessionSaved {
        /// Saved session id.
        session_id: String,
    },
    /// The run summary could not be saved.
    SessionSaveFailed {
        /// Intended session id.
        session_id: String,
        /// Failure message.
        message: String,
    },
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
    /// Active security and sandbox settings.
    pub security: SecurityConfig,
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
    /// Optional independent model used to verify goal completion.
    pub goal_checker_model: Option<String>,
    /// Maximum number of turns.
    pub max_turns: u32,
    /// Maximum output tokens per turn.
    pub max_tokens: u32,
    /// Maximum estimated cost for the run. Values <= 0 disable budget stopping.
    pub budget_usd: f64,
    /// Budget warning threshold percentage.
    pub budget_warning_pct: u8,
    /// Project root.
    pub project_root: PathBuf,
    /// Denied path prefixes.
    pub denied_paths: Vec<PathBuf>,
    /// Active hook definitions.
    pub hooks: HooksConfig,
    /// Active security and sandbox settings.
    pub security: SecurityConfig,
}

/// Reason a bounded run stopped.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum StopReason {
    /// The agent called agent_done.
    Done,
    /// The run paused because a tool needs explicit approval.
    ApprovalRequired,
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
