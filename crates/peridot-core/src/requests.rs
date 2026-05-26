use std::path::PathBuf;

use peridot_common::{AgentPhase, HooksConfig, SecurityConfig, ToolResult};
use peridot_llm::Usage;
use serde::{Deserialize, Serialize};

/// Wire-format version of [`AgentRunEvent`].
///
/// Daemons emit this value in their initial `peridot.handshake` notification
/// so editor extensions can detect skew (e.g., an older VS Code extension
/// talking to a newer daemon that added an event variant). Bump this whenever:
///   - a variant is removed or renamed,
///   - an existing variant's field is removed/renamed/changes type,
///   - serialization semantics change in a way an older client cannot ignore.
///
/// Adding a *new* variant with new fields does NOT require a bump — older
/// clients should treat unknown variants as a no-op. Bump on breaking changes
/// only.
pub const AGENT_RUN_EVENT_SCHEMA_VERSION: u32 = 1;

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
        /// Stable label for the tool's [`peridot_common::RiskClass`]
        /// (e.g., `"read_only"`, `"destructive"`). Surfaced so the UI can
        /// colour-code tool chips and explain *why* a tool needs approval
        /// without re-deriving the class downstream. Optional for backward
        /// compatibility — older daemons serialised events without it and
        /// new consumers must tolerate `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        risk_class: Option<String>,
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
    /// The agent's high-level phase transitioned. Emitted exactly once per
    /// `AgentState::phase` change so editors / TUIs can render the live
    /// state machine without polling.
    PhaseChanged {
        /// Previous phase.
        from: AgentPhase,
        /// New phase.
        to: AgentPhase,
        /// Short label describing why this transition happened (e.g.,
        /// `"tool_started"`, `"verify_failed"`, `"approval_required"`).
        /// Stable strings — not user-facing prose.
        reason: String,
    },
    /// Context was just compacted with the LLM recap. Carries the
    /// structured [`peridot_context::CompactedContext`] snapshot so
    /// editors / TUIs can render "files read so far", "open todos",
    /// "untrusted inputs" etc. directly instead of parsing the prose
    /// PlanReminder the harness still injects for backward compatibility.
    /// Emitted exactly once per successful LLM compaction.
    ContextCompacted {
        /// Structured snapshot of the recap.
        compacted: peridot_context::CompactedContext,
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
    /// A model/tool turn finished (paired with [`AgentRunEvent::TurnStarted`]).
    TurnEnded {
        /// Zero-based turn index that just completed.
        turn_index: u32,
        /// Whether the turn's tool call reported success.
        success: bool,
    },
    /// The active plan was updated by a tool (e.g., the plan tool).
    PlanUpdated {
        /// Current ordered plan steps.
        steps: Vec<PlanStepUpdate>,
        /// Optional zero-based index of the currently active step.
        current: Option<u32>,
    },
    /// Run-level budget or turn-count guardrail changed.
    BudgetUpdated {
        /// Cost spent so far, in USD.
        cost_used: f64,
        /// Configured cost ceiling (None means unbounded).
        cost_limit: Option<f64>,
        /// Turns consumed so far.
        turns_used: u32,
        /// Configured turn ceiling (None means unbounded).
        turns_limit: Option<u32>,
    },
    /// Context-manager utilization changed.
    ContextUtilizationChanged {
        /// Estimated tokens that will be sent in the next provider request.
        ///
        /// This intentionally includes the system prompt, provider messages,
        /// tool schemas, and a small wire-format overhead estimate. Older
        /// clients display this field directly, so it carries the most useful
        /// "will this next request fit?" number.
        tokens_used: u64,
        /// Full model context window used for display.
        threshold: u64,
        /// Legacy context-manager entry estimate before provider request
        /// assembly. Useful for debugging the difference between stored
        /// conversation context and actual prompt footprint.
        #[serde(default)]
        context_tokens: u64,
        /// Token estimate for the assembled provider messages.
        #[serde(default)]
        message_tokens: u64,
        /// Token estimate for the system prompt.
        #[serde(default)]
        system_tokens: u64,
        /// Token estimate for native tool schemas attached to the request.
        #[serde(default)]
        tool_schema_tokens: u64,
        /// Estimated provider wire/protocol overhead.
        #[serde(default)]
        overhead_tokens: u64,
    },
    /// MCP server status changed (one or more servers connected / disconnected).
    McpStatusChanged {
        /// Current server snapshot.
        servers: Vec<McpStatusUpdate>,
    },
    /// AGENTS.md rules loaded at session start.
    AgentsMdLoaded {
        /// Total parsed rule count.
        rule_count: u32,
        /// Origin file paths.
        paths: Vec<String>,
    },
    /// A hook fired with the carried outcome label.
    HookFired {
        /// Hook name.
        name: String,
        /// Hook category (lifecycle, tool, event, ...).
        category: String,
        /// Outcome label such as `allow`, `block`, or `ok`.
        outcome: String,
    },
    /// The active run was interrupted by an external cancellation signal.
    Interrupted {
        /// Stage name (model_call, tool_call, verification, ...).
        stage: String,
    },
    /// The Planner role (M-COM2) finished its pre-flight pass and produced a
    /// task plan that the Executor will see as a `PlanReminder` context entry.
    PlannerPlanReady {
        /// Markdown / plain-text plan generated by the planner agent.
        plan_text: String,
    },
    /// The Reviewer role (M-COM3) returned a verdict for one executor turn.
    ReviewerVerdict {
        /// Zero-based executor turn index this verdict applies to.
        turn_index: u32,
        /// Verdict body.
        verdict: ReviewerVerdict,
    },
    /// An auto-fix verification attempt completed.
    AutoFixAttempt {
        /// One-based attempt number.
        attempt: u32,
        /// Configured maximum attempts.
        max: u32,
        /// Verification tool that was checked.
        tool_name: String,
        /// Whether the check passed.
        passed: bool,
    },
    /// One non-executor committee role consumed provider tokens. The
    /// executor's per-turn cost is already covered by `UsageUpdated`; this
    /// event only fires for Planner and Reviewer so the TUI can split per
    /// role.
    CommitteeRoleUsage {
        /// Which committee role accumulated the cost.
        role: String,
        /// Estimated cost in USD for this pass.
        cost_usd: f64,
        /// Approximate token count for this pass.
        tokens: u64,
    },
    /// A file-mutating tool (`file_write` / `file_patch`) finished
    /// successfully. Carries the previous and new file content so the TUI
    /// — and any future extension client subscribing to the event stream —
    /// can render a unified before/after diff without re-reading the
    /// workspace or the audit log. `before` is `None` when the tool
    /// created a brand-new file.
    FileDiff(FileDiffPayload),
}

/// Before/after payload emitted alongside [`AgentRunEvent::FileDiff`].
///
/// The harness reads `before` from the on-disk `.peridot/checkpoints/`
/// entry written immediately before the mutation, and reads `after`
/// from the resulting file on disk. Both are full file contents so
/// downstream consumers can run their own hunk algorithm (LCS, Myers,
/// patience, …) without trusting the agent's serialised arguments.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileDiffPayload {
    /// Mutating tool name (`file_write` or `file_patch`).
    pub tool_name: String,
    /// Project-relative path that was mutated.
    pub path: String,
    /// File content before the mutation. `None` when the file did not
    /// exist (e.g. fresh `file_write`).
    pub before: Option<String>,
    /// File content after the mutation. Always populated when the event
    /// fires.
    pub after: String,
}

/// Verdict returned by the reviewer agent after inspecting one executor
/// turn's diff. `Approve` lets the executor continue uninterrupted;
/// `RequestChanges` injects the reviewer's comments into the executor's
/// context for the next turn; `Block` halts the run and prompts the
/// operator for an override.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReviewerVerdict {
    /// The diff is correct and may land. No follow-up.
    Approve,
    /// The diff has fixable issues; `comments` describes what to change.
    RequestChanges {
        /// Actionable comments from the reviewer.
        comments: String,
    },
    /// The diff has a fundamental problem; halt and ask the operator.
    Block {
        /// Reason for blocking the run.
        reason: String,
    },
}

/// One plan step update payload accompanying [`AgentRunEvent::PlanUpdated`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlanStepUpdate {
    /// Step label.
    pub label: String,
    /// Whether the step is marked done.
    pub done: bool,
}

/// One MCP server snapshot accompanying [`AgentRunEvent::McpStatusChanged`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct McpStatusUpdate {
    /// Server display name.
    pub name: String,
    /// Exposed tool count.
    pub tool_count: u32,
    /// Whether the server is currently connected.
    pub connected: bool,
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
    /// Reasoning intensity forwarded to the provider for this turn.
    pub reasoning_effort: peridot_common::ReasoningEffort,
    /// Optional provider service tier forwarded to the provider for this turn.
    pub service_tier: Option<String>,
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
    /// Reasoning intensity forwarded to the provider for every turn.
    /// Sourced from `state.reasoning_effort` (slash-command override) with
    /// `config.models.reasoning_effort` as the persistent fallback. The
    /// agent loop passes this verbatim to each `CompletionRequest`.
    pub reasoning_effort: peridot_common::ReasoningEffort,
    /// Optional provider service tier forwarded to every turn.
    pub service_tier: Option<String>,
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
    /// The run was cancelled by an external interrupt.
    Interrupted,
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
    /// Wall-clock time from the first `run_until_done_with_events` event to
    /// the `Finished` event, in milliseconds. Defaults to `0` for old
    /// serialised summaries that pre-date this field.
    #[serde(default)]
    pub duration_ms: u64,
}
