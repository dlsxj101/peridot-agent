use std::time::{SystemTime, UNIX_EPOCH};

use super::*;

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

const MAX_INPUT_HISTORY_ENTRIES: usize = 50;

/// Formats a millisecond duration as a compact human-readable string. Sub-
/// second durations show milliseconds so a 200 ms call doesn't render as
/// "0s"; longer durations promote into `Xm Ys` and then `Xh Ym` so the
/// status bar stays narrow even on multi-hour runs.
pub fn format_duration_ms(ms: u64) -> String {
    if ms < 1000 {
        return format!("{ms} ms");
    }
    let total_seconds = ms / 1000;
    let seconds = total_seconds % 60;
    let total_minutes = total_seconds / 60;
    let minutes = total_minutes % 60;
    let hours = total_minutes / 60;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

/// TUI layout mode selected from terminal size.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutMode {
    /// Header, main panel, side panel, and input.
    Full,
    /// Header, main panel, and input.
    Compact,
    /// Minimal transcript plus input.
    Minimal,
}

/// Header state shown at the top of the TUI.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HeaderState {
    /// Active execution mode.
    pub mode: ExecutionMode,
    /// Active permission mode.
    pub permission: PermissionMode,
    /// Active model name.
    pub model: String,
    /// Total provider tokens observed by the session.
    pub total_tokens: u64,
    /// Prompt-cache hit rate in the range 0.0..=1.0.
    pub cache_hit_rate: f64,
    /// Estimated cost in USD.
    pub cost_usd: f64,
    /// Optional update notice (semver string) surfaced on the right of the header.
    #[serde(default)]
    pub update_available: Option<String>,
    /// Explicit provider override for this session (e.g. "claude-api",
    /// "openai-api", "openrouter-api"). `None` falls back to the project
    /// config's primary auth selection.
    #[serde(default)]
    pub provider: Option<String>,
    /// Display label for the workspace this session targets (typically the
    /// project root's basename). Rendered in the status bar so the operator
    /// can tell which checkout each session is acting against.
    #[serde(default)]
    pub workspace_label: Option<String>,
}

impl HeaderState {
    /// Creates a new header state.
    pub fn new(mode: ExecutionMode, permission: PermissionMode, model: impl Into<String>) -> Self {
        Self {
            mode,
            permission,
            model: model.into(),
            total_tokens: 0,
            cache_hit_rate: 0.0,
            cost_usd: 0.0,
            update_available: None,
            provider: None,
            workspace_label: None,
        }
    }

    /// Records provider usage for header display.
    pub fn record_usage(
        &mut self,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_creation_tokens: u64,
        cost_usd: f64,
    ) {
        self.total_tokens +=
            input_tokens + output_tokens + cache_read_tokens + cache_creation_tokens;
        self.cost_usd += cost_usd;
        let prompt_tokens = input_tokens + cache_read_tokens + cache_creation_tokens;
        if prompt_tokens > 0 {
            self.cache_hit_rate = cache_read_tokens as f64 / prompt_tokens as f64;
        }
    }
}

/// One plan item shown in the side panel.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlanStep {
    /// Step label.
    pub label: String,
    /// Whether the step has completed.
    pub done: bool,
}

/// Session statistics shown in the side panel.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionStats {
    /// Completed tool/model steps.
    pub steps: u32,
    /// Recoverable error count.
    pub errors: u32,
    /// Elapsed seconds.
    pub elapsed_seconds: u64,
}

/// One MCP server summary shown in the side panel.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct McpServerSummary {
    /// Display name.
    pub name: String,
    /// Transport kind, when known.
    #[serde(default)]
    pub transport: Option<String>,
    /// Tool count exposed by the server.
    pub tool_count: u32,
    /// Whether the server is currently connected.
    pub connected: bool,
}

/// Summary of AGENTS.md rules loaded at session start.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgentsSummary {
    /// Total parsed rule count.
    pub rule_count: u32,
    /// Origin file paths in workspace order.
    pub paths: Vec<String>,
}

/// Persisted workspace code-map freshness shown in the side panel.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodeMapSummary {
    /// Whether `.peridot/codemap.json` exists.
    pub index_exists: bool,
    /// Whether source files are newer than the index.
    pub stale: bool,
    /// Number of source files considered by the freshness check.
    pub source_files: usize,
    /// Number of files walked by the persisted index.
    pub walked_files: usize,
    /// Number of indexed symbols.
    pub symbol_count: usize,
    /// Number of indexed TODO/FIXME/HACK markers.
    pub todo_count: usize,
    /// Index creation timestamp, when available.
    pub generated_at_unix: Option<u64>,
    /// Newest source mtime observed by the freshness check, when available.
    pub newest_source_mtime_unix: Option<u64>,
    /// Whether the last command had to refresh the persisted index.
    #[serde(default)]
    pub refreshed: bool,
}

/// Operator-note summary shown in the side panel.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct NoteSummary {
    /// Number of notes known for the active session.
    pub count: usize,
    /// Most recent note text, when known.
    #[serde(default)]
    pub latest: Option<String>,
}

/// Budget and turn guardrail gauge shown in the side panel.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BudgetGauge {
    /// Accumulated cost in USD.
    pub cost_used: f64,
    /// Configured cost limit in USD (None means unbounded).
    pub cost_limit: Option<f64>,
    /// Turns consumed so far.
    pub turns_used: u32,
    /// Maximum allowed turns (None means unbounded).
    pub turns_limit: Option<u32>,
}

/// Right-side panel state.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SidePanelState {
    /// Current plan steps.
    pub plan: Vec<PlanStep>,
    /// Session statistics.
    pub stats: SessionStats,
    /// Active MCP server summaries.
    #[serde(default)]
    pub mcp_status: Vec<McpServerSummary>,
    /// Persisted code-map freshness summary.
    #[serde(default)]
    pub code_map: Option<CodeMapSummary>,
    /// AGENTS.md rule summary loaded at session start.
    #[serde(default)]
    pub agents_md: AgentsSummary,
    /// Approximate context utilization in 0.0..=1.0 (1.0 means at threshold).
    #[serde(default)]
    pub context_pct: f32,
    /// Raw token count of the current context (estimated). Kept alongside
    /// `context_pct` so the status bar can render `used/window` directly
    /// instead of having to back-derive the count from the percentage.
    #[serde(default)]
    pub context_tokens_used: usize,
    /// Context-manager entry estimate before assembling the provider request.
    #[serde(default)]
    pub context_entry_tokens: usize,
    /// Token estimate for the assembled provider messages.
    #[serde(default)]
    pub context_message_tokens: usize,
    /// Token estimate for the active system prompt.
    #[serde(default)]
    pub context_system_tokens: usize,
    /// Token estimate for native tool schemas attached to the request.
    #[serde(default)]
    pub context_tool_schema_tokens: usize,
    /// Estimated provider wire/protocol overhead.
    #[serde(default)]
    pub context_overhead_tokens: usize,
    /// Full active model context window used for display.
    /// Surfaced verbatim in the status line so the operator can confirm
    /// which window peridot resolved for the active model.
    #[serde(default)]
    pub context_tokens_window: usize,
    /// Budget and turn-count gauge.
    #[serde(default)]
    pub budget: BudgetGauge,
}

/// Kind of runtime activity displayed in the TUI side panel.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    /// Model streaming or thinking output.
    Stream,
    /// Tool execution.
    Tool,
    /// Subagent delegation.
    Subagent,
    /// Verification stage.
    Verification,
}

/// Background agent run status for the interactive TUI.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunStatus {
    /// No task is currently running.
    #[default]
    Idle,
    /// A task is running in the background.
    Running,
    /// The last task completed successfully.
    Succeeded,
    /// The last task failed.
    Failed,
    /// The agent is waiting for explicit user approval.
    WaitingApproval,
    /// The last task was interrupted by the user before completion.
    Interrupted,
}

/// One recent runtime activity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeActivity {
    /// Activity kind.
    pub kind: ActivityKind,
    /// Short label such as a tool name or stage name.
    pub label: String,
    /// Human-readable status.
    pub status: String,
}

/// Runtime event accepted from a background agent worker.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TuiRuntimeEvent {
    /// A task started.
    RunStarted {
        /// Task text.
        task: String,
    },
    /// A model/tool turn started.
    TurnStarted {
        /// Zero-based turn index.
        turn_index: u32,
    },
    /// Assistant stream started.
    AssistantStarted {
        /// Stream label.
        label: String,
    },
    /// Assistant stream delta.
    AssistantDelta {
        /// Text delta.
        delta: String,
    },
    /// Assistant stream finished.
    AssistantFinished,
    /// Parsed thinking text.
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
        /// Whether the tool succeeded.
        success: bool,
        /// Short result summary.
        summary: String,
        /// Structured tool output.
        output: serde_json::Value,
    },
    /// A file-mutating tool finished and the harness captured the
    /// before/after content. The TUI computes hunks via
    /// [`crate::diff_hunks::diff_hunks`] and pushes them into the
    /// transcript as `TranscriptKind::Diff` entries so the chat view
    /// shows a real unified diff for both `file_write` and `file_patch`.
    FileDiff(FileDiffPayload),
    /// Tool execution is waiting on explicit user approval.
    ApprovalRequested {
        /// Tool name.
        tool_name: String,
        /// Reason the tool is gated.
        reason: String,
        /// Parameters the tool was about to execute with.
        #[serde(default)]
        parameters: serde_json::Value,
        /// Optional stable tool risk-class label.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        risk_class: Option<String>,
    },
    /// `agent_ask_user` needs a real user answer. The TUI opens its
    /// ask-user panel for `request`; once the operator confirms, the CLI
    /// resolves the matching `request_id` through its `AskUserPort`
    /// registry.
    AskUserRequested {
        /// Correlation id echoed back when the panel resolves.
        request_id: String,
        /// Structured ask-user request.
        request: AskUserRequest,
    },
    /// Turn list response for an open branch picker. Sent by the CLI
    /// after reading the session's context snapshot.
    BranchPickerTurns {
        /// Turns the operator can fork from. May be empty when the
        /// session has no on-disk snapshot.
        turns: Vec<BranchPickerTurn>,
    },
    /// Provider usage changed.
    UsageUpdated {
        /// Total tokens.
        total_tokens: u64,
        /// Cache hit rate.
        cache_hit_rate: f64,
        /// Estimated cost in USD.
        cost_usd: f64,
    },
    /// Recovery warning.
    Recovery {
        /// Recovery message.
        message: String,
    },
    /// Centralized `AgentPhase` transition. Forwarded from the harness's
    /// `PhaseChanged` event so the TUI / daemon can render the live state
    /// machine without polling. `from`/`to` are the formatted phase names
    /// (snake_case) — kept as strings here to avoid pulling
    /// peridot-common into the TUI's runtime-event surface for what's
    /// purely a display concern.
    PhaseChanged {
        /// Previous phase, formatted lower-case (e.g., `"planning"`).
        from: String,
        /// New phase, formatted lower-case (e.g., `"executing"`).
        to: String,
        /// Short stable label describing why this transition happened.
        reason: String,
    },
    /// Context was compacted; carries a compact structured snapshot.
    /// Surfaced as an activity-panel entry; the structured fields are
    /// available to side-panel renderers via the underlying
    /// [`peridot_context::CompactedContext`] but the TUI currently
    /// only renders the narrative.
    ContextCompacted {
        /// LLM-generated short prose summary.
        narrative: String,
        /// Number of distinct files the model has read so far.
        files_read_count: usize,
        /// Number of untrusted-external inputs in the conversation so far.
        untrusted_count: usize,
    },
    /// The host refreshed dynamic auto-skill slash suggestions.
    SkillSuggestionsUpdated {
        /// Active auto-skills exposed as `/skill-name` suggestions.
        skills: Vec<crate::SkillSlashSuggestion>,
    },
    /// Background run finished.
    Finished {
        /// Stop reason.
        stop_reason: String,
        /// Number of turns.
        turns: usize,
        /// Whether the stop reason represents successful completion.
        success: bool,
        /// Wall-clock duration of the run in milliseconds.
        #[serde(default)]
        duration_ms: u64,
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
    /// Background run failed before producing a summary.
    Failed {
        /// Failure message.
        message: String,
    },
    /// A model/tool turn finished.
    TurnEnded {
        /// Zero-based turn index that completed.
        turn_index: u32,
        /// Whether the turn succeeded.
        success: bool,
    },
    /// The active plan was updated.
    PlanUpdated {
        /// Plan step labels with done flags.
        steps: Vec<PlanStepUpdate>,
        /// Current step index, when applicable.
        current: Option<u32>,
    },
    /// Run-level budget guardrail changed.
    BudgetUpdated {
        /// Cost spent.
        cost_used: f64,
        /// Cost ceiling.
        cost_limit: Option<f64>,
        /// Turns used.
        turns_used: u32,
        /// Turn ceiling.
        turns_limit: Option<u32>,
    },
    /// Context window utilization changed.
    ContextUtilizationChanged {
        /// Tokens used.
        tokens_used: u64,
        /// Full model context window.
        threshold: u64,
        /// Context-manager entry estimate.
        #[serde(default)]
        context_tokens: u64,
        /// Provider message estimate.
        #[serde(default)]
        message_tokens: u64,
        /// System prompt estimate.
        #[serde(default)]
        system_tokens: u64,
        /// Tool schema estimate.
        #[serde(default)]
        tool_schema_tokens: u64,
        /// Wire/protocol overhead estimate.
        #[serde(default)]
        overhead_tokens: u64,
    },
    /// MCP server status snapshot.
    McpStatusChanged {
        /// Server entries.
        servers: Vec<McpServerSummary>,
    },
    /// AGENTS.md summary loaded.
    AgentsMdLoaded {
        /// Total rule count.
        rule_count: u32,
        /// Origin paths.
        paths: Vec<String>,
    },
    /// One hook fired.
    HookFired {
        /// Hook name.
        name: String,
        /// Hook category.
        category: String,
        /// Outcome label.
        outcome: String,
    },
    /// External interrupt signal received.
    Interrupted {
        /// Stage name.
        stage: String,
    },
    /// Committee planner produced its task plan (M-COM2).
    PlannerPlanReady {
        /// Plan text dictated by the planner agent.
        plan_text: String,
    },
    /// Committee reviewer returned a verdict after inspecting one executor turn.
    ReviewerVerdict {
        /// Zero-based executor turn index this verdict applies to.
        turn_index: u32,
        /// Verdict label: "approve" | "request_changes" | "block".
        verdict: String,
        /// Reviewer comments or blocking reason (empty on plain approve).
        comments: String,
    },
    /// An auto-fix verification attempt completed.
    AutoFixAttempt {
        /// One-based attempt number.
        attempt: u32,
        /// Configured maximum.
        max: u32,
        /// Verification tool name.
        tool_name: String,
        /// Whether the check passed.
        passed: bool,
    },
    /// One non-executor committee role (planner / reviewer) used tokens.
    CommitteeRoleUsage {
        /// Role label ("planner" or "reviewer").
        role: String,
        /// Estimated cost in USD for this pass.
        cost_usd: f64,
        /// Approximate token count for this pass.
        tokens: u64,
    },
    /// LLM-generated session title ready.
    SessionTitleUpdated {
        /// Session whose title was generated.
        session_id: String,
        /// Generated short title.
        title: String,
    },
}

/// Plan step payload carried by [`TuiRuntimeEvent::PlanUpdated`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlanStepUpdate {
    /// Step label.
    pub label: String,
    /// Whether the step is marked done.
    pub done: bool,
}

/// One delegated subagent shown in the monitoring panel.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SubagentMonitorItem {
    /// Subagent kind such as fork, worktree, or teammate.
    pub kind: String,
    /// Short task label.
    pub task: String,
    /// Current state.
    pub status: String,
    /// Optional result summary or failure reason.
    pub summary: Option<String>,
    /// Stable subagent identifier used for parent/child wiring.
    #[serde(default)]
    pub id: String,
    /// Parent subagent or session id, when this subagent was spawned from another.
    #[serde(default)]
    pub parent_id: Option<String>,
    /// Tree depth (0 = top-level under the root agent).
    #[serde(default)]
    pub depth: u32,
    /// Wall-clock start time (unix seconds; 0 means unset).
    #[serde(default)]
    pub started_at_unix: u64,
    /// Provider tokens consumed by this subagent so far.
    #[serde(default)]
    pub tokens: u64,
}

/// Active model stream displayed before it is committed to the transcript.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StreamState {
    /// Stream label.
    pub label: String,
    /// Accumulated visible text.
    pub content: String,
    /// Whether the stream has completed.
    pub done: bool,
}

/// Visual category for one transcript entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptKind {
    /// A user-submitted input line.
    User,
    /// A user-facing assistant response.
    Assistant,
    /// A tool started running.
    ToolStart,
    /// A tool finished successfully.
    ToolOk,
    /// A tool finished with failure.
    ToolFail,
    /// A neutral system/info line (mode switch, task start, etc.).
    System,
    /// A soft notice or hint (queued input, etc.).
    Notice,
    /// An error line.
    Error,
    /// Debug content hidden in normal mode.
    Debug,
    /// A turn boundary separator (rendered as a horizontal rule).
    TurnSeparator,
    /// Parsed model thinking surfaced only when debug or show_thinking is on.
    Thinking,
    /// Run-lifecycle bookkeeping ("task: foo", "run: stopped=Done turns=3",
    /// "session: saved session-..."). Pushed by the agent loop and the host
    /// runtime; hidden from the live chat view because the user only wants to
    /// see the conversation itself. Still serialised so headless / snapshot
    /// consumers and session journals retain the trace.
    Meta,
    /// One diff line emitted alongside a `file_patch` (or future `file_edit`)
    /// tool call. Each entry holds a single `- old` / `+ new` line so the
    /// renderer can colour them independently and so long diffs can be
    /// truncated cleanly. Visible in the chat view by default — unlike the
    /// generic indented tool preview lines we hide for other tools.
    Diff,
}

/// One transcript line plus its style classification.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TranscriptEntry {
    /// Visual category.
    pub kind: TranscriptKind,
    /// Plain-text payload (no styling).
    pub text: String,
    /// Unix timestamp for chronological replay across sidecar journals.
    #[serde(default)]
    pub ts: u64,
    /// Optional turn index this entry belongs to (used for grouping).
    #[serde(default)]
    pub parent_turn: Option<u32>,
}

impl TranscriptEntry {
    /// Creates a transcript entry of the given kind.
    pub fn new(kind: TranscriptKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
            ts: current_unix_seconds(),
            parent_turn: None,
        }
    }
}

/// Lifecycle transition captured during an interactive TUI session.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TuiLifecycleEvent {
    /// Hook event name.
    pub event: String,
    /// Previous value.
    pub from: String,
    /// New value.
    pub to: String,
}

/// Floating slash-command picker state (populated in PR4).
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SlashPicker {
    /// Current query prefix.
    pub query: String,
    /// Highlighted suggestion index.
    pub selected: usize,
}

fn default_collapse_threshold() -> usize {
    8
}

fn default_auto_fix_max() -> u32 {
    3
}

/// Main TUI state independent from the terminal backend.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TuiState {
    /// Current layout mode.
    pub layout: LayoutMode,
    /// User-facing TUI settings.
    pub config: TuiConfig,
    /// Header state.
    pub header: HeaderState,
    /// Transcript lines.
    pub transcript: Vec<TranscriptEntry>,
    /// Active streaming model output.
    pub active_stream: Option<StreamState>,
    /// Recent tool, stream, and verification activity.
    pub activities: Vec<RuntimeActivity>,
    /// Recent delegated subagents.
    pub subagents: Vec<SubagentMonitorItem>,
    /// Tool names currently executing in the background.
    pub active_tools: Vec<String>,
    /// Animation counter used for the running-tool spinner.
    pub spinner_tick: u32,
    /// Inputs queued while the agent is busy; flushed when it becomes idle.
    pub input_queue: Vec<String>,
    /// Whether internal/raw assistant output (JSON, thinking) is rendered.
    pub debug_view: bool,
    /// Current background run status.
    pub agent_run_status: AgentRunStatus,
    /// Side panel state.
    pub side_panel: SidePanelState,
    /// Current goal lifecycle status, when a goal is active.
    pub goal_status: Option<GoalStatus>,
    /// Last submitted task, used when a paused approval flow resumes.
    pub last_task: Option<String>,
    /// Current input buffer.
    pub input: String,
    /// Cursor position in input, counted in characters.
    pub input_cursor: usize,
    /// Submitted input history.
    pub input_history: Vec<String>,
    /// Current input-history cursor.
    pub input_history_cursor: Option<usize>,
    /// Active ask-user panel, when the agent is waiting for user guidance.
    pub ask_user: Option<AskUserPanel>,
    /// Active approval panel, when a gated tool needs confirmation.
    pub approval: Option<ApprovalPanel>,
    /// Approval grants remembered for this TUI session.
    #[serde(default)]
    pub approval_grants: Vec<ApprovalGrant>,
    /// Active branch picker overlay, when the operator typed `/branch`
    /// with no args. Populated asynchronously by the CLI handler
    /// after reading the session's context snapshot.
    #[serde(default)]
    pub branch_picker: Option<BranchPickerState>,
    /// Active session picker overlay, opened with Ctrl+T.
    #[serde(default)]
    pub session_picker: Option<SessionPickerState>,
    /// Active Esc menu.
    pub menu: Option<MenuState>,
    /// Lifecycle events recorded from local TUI commands.
    pub lifecycle_events: Vec<TuiLifecycleEvent>,
    /// Scrollback offset (entries skipped from the end). 0 means follow-tail.
    #[serde(default)]
    pub scroll_offset: usize,
    /// Floating slash-command picker, when active.
    #[serde(default)]
    pub slash_picker: Option<SlashPicker>,
    /// Dynamic auto-skills surfaced as `/skill-name` suggestions.
    #[serde(default)]
    pub skill_suggestions: Vec<crate::SkillSlashSuggestion>,
    /// Model names discovered from project config for slash autocomplete.
    #[serde(default, skip)]
    pub model_suggestions: Vec<String>,
    /// Saved branch snapshot names discovered under `.peridot/branches`.
    #[serde(default, skip)]
    pub branch_suggestions: Vec<String>,
    /// Append-only log of parsed thinking text (for debug toggle re-render).
    #[serde(default)]
    pub thinking_log: Vec<String>,
    /// Last successful session save (unix seconds; 0 means unset).
    #[serde(default)]
    pub last_session_save_unix: u64,
    /// Current turn index emitted by the agent loop (rolling counter).
    #[serde(default)]
    pub current_turn: u32,
    /// Multi-session directory: one entry per concurrent agent run.
    #[serde(default)]
    pub sessions: Vec<crate::session_directory::SessionDirectoryItem>,
    /// Id of the session currently in the foreground of the TUI.
    #[serde(default)]
    pub current_session_id: String,
    /// Deferred session-router commands emitted from slash handlers. The host
    /// loop drains these every tick and applies them to its router. Skipped
    /// from serialisation so resumed sessions never re-execute stale commands.
    #[serde(default, skip)]
    pub pending_session_commands: Vec<SessionCommandEvent>,
    /// Free-form operator notes queued by the `/note <text>` slash command.
    /// The host drains the queue every tick and appends each entry to the
    /// current session's `notes.ndjson`. Skipped from serialisation so a
    /// resumed session never replays the queue after the disk write already
    /// landed.
    #[serde(default, skip)]
    pub pending_notes: Vec<String>,
    /// Committee events (planner / reviewer / role-usage) queued by
    /// `apply_runtime_event`. The host drains the queue every tick and
    /// appends each entry to `<sessions>/<id>/committee.ndjson`.
    #[serde(default, skip)]
    pub pending_committee_events: Vec<serde_json::Value>,
    /// Active committee mode for the foreground session (M-COM4). Mirrors
    /// `[committee].mode` from project config, with `/committee <mode>`
    /// switching it per-session at runtime.
    #[serde(default)]
    pub committee_mode: peridot_common::CommitteeMode,
    /// Estimated USD spent by the Planner role this session (M-COM5).
    #[serde(default)]
    pub committee_planner_cost: f64,
    /// Token total consumed by the Planner role this session (M-COM5).
    #[serde(default)]
    pub committee_planner_tokens: u64,
    /// Estimated USD spent by the Reviewer role this session (M-COM5).
    #[serde(default)]
    pub committee_reviewer_cost: f64,
    /// Token total consumed by the Reviewer role this session (M-COM5).
    #[serde(default)]
    pub committee_reviewer_tokens: u64,
    /// Wall-clock start of the currently running task (unix seconds). `None`
    /// while idle / between runs. `tick_spinner` recomputes
    /// `side_panel.stats.elapsed_seconds` from this anchor on every frame so
    /// the status bar advances second-by-second without the host loop having
    /// to push periodic events.
    #[serde(default)]
    pub task_started_at_unix: Option<u64>,
    /// Runtime override for the model spawned sub-agents (`/fork`,
    /// `/teammate`, `/worktree`, `agent_delegate`) should use. `None` means
    /// "fall back to `config.subagents.default_model` from the toml, and if
    /// that's also unset, inherit the caller's main model." Mutated via the
    /// `/subagent model <name|reset>` slash command; serialised so a resumed
    /// session keeps the operator's runtime preference.
    #[serde(default)]
    pub subagent_default_model: Option<String>,
    /// Active goal-mode objective text (the verbatim string the operator
    /// passed to `/goal <objective>`). `None` while no goal is active. Lets
    /// the side panel show the actual target instead of just "running".
    #[serde(default)]
    pub goal_text: Option<String>,
    /// Wall-clock start of the current goal in unix seconds. Used to render
    /// "goal age" in the side panel. Set on `/goal <objective>`, refreshed
    /// when paused goals are resumed, cleared on `/goal clear`.
    #[serde(default)]
    pub goal_started_at_unix: Option<u64>,
    /// Runtime reasoning-intensity dial applied to outgoing model requests.
    /// Initialised from `config.models.reasoning_effort`; mutated via the
    /// `/reasoning <off|low|medium|high|xhigh>` slash command. Persisted so a
    /// resumed session keeps the operator's preference.
    #[serde(default)]
    pub reasoning_effort: peridot_common::ReasoningEffort,
    /// Runtime service-tier override. `Some("fast")` requests the provider's
    /// fast/priority lane where supported; `None` uses provider defaults.
    #[serde(default)]
    pub service_tier: Option<String>,
    /// Active `@file` picker, when the input cursor sits inside an
    /// `@<token>` mention. Cleared as soon as the cursor leaves the
    /// token. Tab / Enter inserts the highlighted suggestion in place of
    /// the partial mention.
    #[serde(default, skip)]
    pub at_picker: Option<crate::at_picker::AtPicker>,
    /// Cached project file index used by the `@file` picker. Built on
    /// first open via [`at_picker::build_file_index`] and reused for the
    /// rest of the session — skipped from serialisation because the disk
    /// scan happens lazily on a freshly resumed session anyway.
    #[serde(default, skip)]
    pub at_picker_index: Vec<String>,
    /// Files currently known to be attached to the active session context.
    /// Updated by `/attach`, `/attachments`, and `/detach`; persisted as a
    /// small convenience cache so resumed sessions can still complete
    /// `/detach <path>` before the operator reloads the inventory.
    #[serde(default)]
    pub attachment_paths: Vec<String>,
    /// Operator-note summary for the active session.
    #[serde(default)]
    pub note_summary: NoteSummary,
    /// Whether the auto-fix loop is enabled for this session.
    #[serde(default)]
    pub auto_fix_enabled: bool,
    /// Maximum identical-failure attempts before circuit-breaker fires.
    #[serde(default = "default_auto_fix_max")]
    pub auto_fix_max_attempts: u32,
    /// Indices of transcript block headers the user explicitly toggled.
    #[serde(default)]
    pub collapsed_blocks: std::collections::HashSet<usize>,
    /// Global toggle: when true all tool/diff blocks are collapsed.
    #[serde(default)]
    pub collapse_all_tool_blocks: bool,
    /// Line-count threshold above which a tool/diff block auto-collapses.
    #[serde(default = "default_collapse_threshold")]
    pub collapse_threshold: usize,
}

/// A session-router intent emitted by a slash command. The TUI itself does not
/// own the router; instead it pushes one of these onto
/// [`TuiState::pending_session_commands`] and the CLI drains them every tick.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionCommandEvent {
    /// `/session new [task]` — open a new session, optionally with an initial task.
    SessionNew(Option<String>),
    /// `/session switch <id|title>` — make a different session the foreground one.
    SessionSwitch(String),
    /// `/session close <id|title>` — cancel and remove a session.
    SessionClose(String),
    /// `/session delete <id|title>` — cancel and remove a session and persisted data.
    SessionDelete(String),
    /// `/session rename <id|title> <new title>` — update a session display title.
    SessionRename {
        /// Session id, title, or index to rename.
        target: String,
        /// New display title.
        title: String,
    },
    /// `/session count` — show persisted lifecycle counts.
    SessionCount,
    /// `/session list --status <state>` — show persisted sessions matching one lifecycle state.
    SessionListStatus(String),
    /// `/session prune [filters]` — remove persisted sessions matching filters.
    SessionPrune {
        /// Optional lifecycle filter.
        status: Option<String>,
        /// Optional updated-at age filter.
        older_than_days: Option<u64>,
        /// Preview matching sessions without deleting them.
        dry_run: bool,
    },
    /// `/session search <query>` — search persisted transcripts.
    SessionSearch(String),
    /// `/session show <id|title>` — show persisted session details.
    SessionShow(String),
    /// `/session locate <id|title>` — show the session directory path.
    SessionLocate(String),
    /// `/session resume <id|title>` — start a task from a saved session summary.
    SessionResume(String),
    /// `/session replay <id|title> [--last N]` — replay a persisted session timeline.
    SessionReplay {
        /// Session id, title, or index to replay.
        target: String,
        /// Optional cap for the most recent timeline entries.
        last: Option<usize>,
    },
    /// `/session export <id|title> [artifacts]` — export a persisted session.
    SessionExport {
        /// Session id, title, or index to export.
        target: String,
        /// Artifact classes to export. Empty means full copy.
        artifacts: Vec<peridot_core::ExportArtifact>,
    },
    /// `/session import <dir> [--id <id>] [--force]` — import a persisted session directory.
    SessionImport {
        /// Source directory to import.
        from: String,
        /// Optional imported session id override.
        id: Option<String>,
        /// Overwrite an existing persisted session with the same id.
        force: bool,
    },
    /// `/fork <task>` — spawn a single-turn Fork subagent inline.
    Fork(String),
    /// `/teammate <task>` — spawn a worktree-isolated Teammate subagent.
    Teammate(String),
    /// `/worktree <branch> <task>` — explicit worktree-isolated fork.
    Worktree {
        /// Branch name to materialise as a git worktree.
        branch: String,
        /// Initial task text.
        task: String,
    },
    /// `/mcp list` — read current `config.mcp` entries from disk and
    /// render them in the transcript.
    McpList,
    /// `/mcp add <name> <transport> <target>` — append a new MCP server
    /// entry to `config.toml`. The host loop validates the transport,
    /// writes the file atomically, and posts a notice telling the user to
    /// restart the session for the change to take effect.
    McpAdd {
        /// Unique server name.
        name: String,
        /// `stdio` or `http`.
        transport: String,
        /// Stdio command (with optional args) or HTTP endpoint URL.
        target: String,
    },
    /// `/mcp remove <name>` — delete the named entry from `config.toml`.
    McpRemove(String),
    /// `/mcp test <name>` — run a one-shot connect-and-list-tools probe
    /// against the named server, reporting tool count or failure.
    McpTest(String),
    /// `/todos` — walk the project for TODO / FIXME / HACK / XXX / BUG
    /// markers and dump the hit list (path:line: text) to the transcript.
    ScanTodos,
    /// `/codemap` — scan source files for public symbols and TODO markers.
    CodeMap,
    /// `/codemap status` — report whether the persisted code map index is stale.
    CodeMapStatus,
    /// `/codemap refresh` — rebuild the persisted code map index.
    CodeMapRefresh,
    /// `/codemap find <query>` — search the persisted code map index.
    CodeMapFind(String),
    /// `/codemap locate <symbol>` — locate symbol definitions from the persisted code map index.
    CodeMapLocate(String),
    /// `/codemap outline <path>` — list indexed symbols in one workspace file.
    CodeMapOutline(String),
    /// `/codemap refs <symbol>` — find textual references to a workspace symbol.
    CodeMapRefs(String),
    /// `/attachments` — list files attached to the active session context.
    Attachments,
    /// `/attach <path>` — read a workspace file into the active session
    /// context as an operator-provided attachment.
    Attach(String),
    /// `/detach <path>` — remove matching attachment entries from context.
    Detach(String),
    /// `/export [attachments|notes|timeline|full]` — write session artifacts.
    Export(Vec<peridot_core::ExportArtifact>),
    /// `/notes [last N]` — list operator notes for the active session.
    Notes(Option<usize>),
    /// `/notes clear` — remove every operator note from the active session.
    NotesClear,
    /// `/rewind` — remove the last user turn from the context snapshot.
    RewindContext,
    /// `/branch save <name>` — copy the active session's context
    /// snapshot under `.peridot/branches/<name>/` for later restore.
    BranchSave(String),
    /// `/branch restore <name>` — overwrite the working context snapshot
    /// with the named branch. Caller is expected to refuse the operation
    /// when the agent is currently running.
    BranchRestore(String),
    /// `/branch list` — list every named branch saved under
    /// `.peridot/branches/` with its creation time.
    BranchList,
    /// `/branch turn <id>` — fork the conversation at a specific past
    /// turn id. Truncates context to turns `<= id` and records lineage
    /// so subsequent turns carry `parent_turn_id = id`. Refused while
    /// the agent is busy.
    BranchTurn(u64),
    /// `/branch tree` — print the DAG journal.
    BranchTree,
    /// `/branch switch <index>` — swap the active path with a journal limb.
    BranchSwitch(usize),
    /// `/branch` (no args) — open the branch picker. The CLI handler
    /// reads the session snapshot, builds the turn list, and feeds it
    /// back via `TuiRuntimeEvent::BranchPickerTurns`.
    BranchPickerOpen,
    /// `/compact` — request an LLM recap of the older portion of the
    /// conversation on the next agent turn boundary, bypassing the
    /// auto threshold. Fire-and-forget; the agent loop consumes the
    /// flag and resets it.
    CompactContext,
    /// `/skill-name [args]` — load a stored skill into the active
    /// session context so the next model turn can use it.
    Skill {
        /// Skill name without the leading slash.
        name: String,
        /// Free-form trailing args supplied by the operator.
        args: String,
    },
    /// `/skills` — list active stored skills available to slash invocation.
    SkillList,
    /// `/skills show <name>` — show details for one stored skill.
    SkillShow(String),
    /// `/skills search <query>` — search active stored skills.
    SkillSearch(String),
    /// `/skills pin <name>` — protect an active stored skill from curation.
    SkillPin(String),
    /// `/skills unpin <name>` — remove the curation protection marker.
    SkillUnpin(String),
    /// `/skills archive <name>` — hide an active stored skill from inventory.
    SkillArchive(String),
    /// `/skills restore <name>` — restore an archived stored skill.
    SkillRestore(String),
    /// `/skills archived [query]` — list archived stored skills.
    SkillArchived(String),
    /// `/context top` — render the largest entries from the persisted
    /// context snapshot so operators can see what is consuming the window.
    ContextTop,
    /// `/undo` — restore the latest file checkpoint.
    UndoLastCheckpoint,
    /// `/clear` — cancel the current session and open a fresh one in
    /// the same workspace. TUI side already wiped its visible state
    /// via `TuiState::reset_for_clear`; the host's job here is to
    /// signal the running agent to stop, close out the old
    /// `SessionHandle`, and register a new one so the next user
    /// message starts a brand-new conversation (empty context,
    /// zeroed usage counters).
    ClearAndRestart,
}

/// Result produced when an interactive TUI session exits.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TuiExit {
    /// Final TUI state.
    pub state: TuiState,
    /// Submitted task, when the user pressed Enter on non-command input.
    pub submitted: Option<String>,
}

/// Outcome of handling one terminal input event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TuiEventOutcome {
    /// Keep rendering the current TUI session.
    Continue,
    /// Exit without submitting a task.
    Quit,
    /// Exit and submit the contained task text.
    Submit(String),
    /// Continue after a tool approval decision.
    Approval {
        /// Decision.
        decision: ApprovalDecision,
        /// Scope at which the approval should be remembered.
        scope: ApprovalScope,
        /// Tool name.
        tool_name: String,
        /// Approval reason.
        reason: String,
        /// Original tool parameters that triggered approval.
        parameters: serde_json::Value,
        /// Optional partial-patch parameters synthesised from the
        /// operator's per-hunk selection. When `Some`, the caller
        /// should re-execute the tool with these parameters instead
        /// of the original ones (a subset of hunks was rejected); when
        /// `None`, the original full-patch parameters still apply.
        synthesised_parameters: Option<serde_json::Value>,
    },
    /// The user pressed Esc while the agent was busy; the run should be cancelled.
    Interrupt,
    /// The operator committed (or cancelled) an `agent_ask_user` panel.
    /// The CLI fulfils the matching oneshot inside its `AskUserPort`
    /// registry so the in-flight tool call can resume.
    AskUserResolved {
        /// Correlation id originally supplied with `open_ask_user_with_id`.
        request_id: String,
        /// Structured answer. `AskUserAnswer::Cancelled` when the
        /// operator dismissed the panel without picking.
        answer: AskUserAnswer,
    },
}

/// Approval remembered for the current interactive session.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ApprovalGrant {
    /// Tool the grant came from.
    pub tool_name: String,
    /// Human-readable approval reason.
    pub reason: String,
    /// Scope selected by the operator.
    pub scope: ApprovalScope,
    /// Exact shell command, when available.
    #[serde(default)]
    pub command: Option<String>,
    /// Project-relative path scope, when available.
    #[serde(default)]
    pub path: Option<String>,
}

impl TuiState {
    /// Creates a new TUI state.
    pub fn new(header: HeaderState) -> Self {
        Self {
            layout: LayoutMode::Full,
            config: TuiConfig::default(),
            header,
            transcript: Vec::new(),
            active_stream: None,
            activities: Vec::new(),
            subagents: Vec::new(),
            active_tools: Vec::new(),
            spinner_tick: 0,
            input_queue: Vec::new(),
            debug_view: false,
            agent_run_status: AgentRunStatus::Idle,
            side_panel: SidePanelState::default(),
            goal_status: None,
            last_task: None,
            input: String::new(),
            input_cursor: 0,
            input_history: Vec::new(),
            input_history_cursor: None,
            ask_user: None,
            approval: None,
            approval_grants: Vec::new(),
            branch_picker: None,
            session_picker: None,
            menu: None,
            lifecycle_events: Vec::new(),
            scroll_offset: 0,
            slash_picker: None,
            skill_suggestions: Vec::new(),
            model_suggestions: Vec::new(),
            branch_suggestions: Vec::new(),
            thinking_log: Vec::new(),
            last_session_save_unix: 0,
            current_turn: 0,
            sessions: Vec::new(),
            current_session_id: String::new(),
            pending_session_commands: Vec::new(),
            pending_notes: Vec::new(),
            note_summary: NoteSummary::default(),
            committee_mode: peridot_common::CommitteeMode::Off,
            committee_planner_cost: 0.0,
            committee_planner_tokens: 0,
            committee_reviewer_cost: 0.0,
            committee_reviewer_tokens: 0,
            pending_committee_events: Vec::new(),
            task_started_at_unix: None,
            subagent_default_model: None,
            goal_text: None,
            goal_started_at_unix: None,
            reasoning_effort: peridot_common::ReasoningEffort::default(),
            service_tier: None,
            at_picker: None,
            at_picker_index: Vec::new(),
            attachment_paths: Vec::new(),
            auto_fix_enabled: false,
            auto_fix_max_attempts: default_auto_fix_max(),
            collapsed_blocks: std::collections::HashSet::new(),
            collapse_all_tool_blocks: false,
            collapse_threshold: default_collapse_threshold(),
        }
    }

    /// Removes and returns every queued committee event in FIFO order. The
    /// host loop drains these and appends them to the session's
    /// `committee.ndjson` journal.
    pub fn drain_pending_committee_events(&mut self) -> Vec<serde_json::Value> {
        std::mem::take(&mut self.pending_committee_events)
    }

    /// Applies configured TUI display settings.
    pub fn with_config(mut self, config: TuiConfig) -> Self {
        self.config = config;
        self
    }

    /// Selects a layout mode from terminal dimensions.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.layout = select_layout(width, height);
    }

    /// Appends a system-kind transcript line.
    pub fn push_transcript(&mut self, line: impl Into<String>) {
        self.push_transcript_entry(TranscriptKind::System, line);
    }

    /// Appends a transcript line of the given kind. If the user has scrolled up
    /// (`scroll_offset > 0`) we grow the offset by the number of `\n`-separated
    /// rows the new entry contributes so the visible window stays anchored
    /// instead of sliding forward when the agent emits output below them.
    /// `scroll_offset` is measured in visual rows above the tail; the render
    /// pass clamps it against the actual wrapped row count, so an
    /// over-estimate here is harmless.
    pub fn push_transcript_entry(&mut self, kind: TranscriptKind, line: impl Into<String>) {
        let entry = TranscriptEntry::new(kind, line);
        let row_count = entry.text.lines().count().max(1);
        self.transcript.push(entry);
        if self.scroll_offset > 0 {
            self.scroll_offset = self.scroll_offset.saturating_add(row_count);
        }
    }

    /// Scrolls the transcript view up by `amount` rows. The render pass
    /// clamps the offset against the total wrapped row count, so a generous
    /// overshoot here is safe.
    pub fn scroll_up(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(amount);
    }

    /// Scrolls the transcript view down by `amount` rows, saturating at the
    /// tail (`scroll_offset == 0`).
    pub fn scroll_down(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
    }

    /// Resets the transcript view to follow the tail. Called automatically when
    /// the user submits new input so they always see their own message land.
    pub fn scroll_to_tail(&mut self) {
        self.scroll_offset = 0;
    }

    /// Returns true when the user has scrolled away from the tail.
    pub fn is_scrolled_back(&self) -> bool {
        self.scroll_offset > 0
    }

    /// Appends a user input line.
    pub fn push_user(&mut self, text: impl Into<String>) {
        self.push_transcript_entry(TranscriptKind::User, text);
    }

    /// Appends a user-facing assistant response.
    pub fn push_assistant(&mut self, text: impl Into<String>) {
        self.push_transcript_entry(TranscriptKind::Assistant, text);
    }

    /// Appends a soft notice (queued input, hint, etc.).
    pub fn push_notice(&mut self, text: impl Into<String>) {
        self.push_transcript_entry(TranscriptKind::Notice, text);
    }

    /// Appends an error line.
    pub fn push_error(&mut self, text: impl Into<String>) {
        self.push_transcript_entry(TranscriptKind::Error, text);
    }

    /// Appends a debug-only line (hidden unless debug_view is enabled).
    pub fn push_debug(&mut self, text: impl Into<String>) {
        self.push_transcript_entry(TranscriptKind::Debug, text);
    }

    /// Records a session-router intent that the host loop will pick up next tick.
    pub fn push_pending_session_command(&mut self, command: SessionCommandEvent) {
        self.pending_session_commands.push(command);
    }

    /// Replaces dynamic auto-skill slash suggestions.
    pub fn set_skill_suggestions(&mut self, skills: Vec<crate::SkillSlashSuggestion>) {
        self.skill_suggestions = skills;
        self.refresh_slash_picker();
    }

    /// Replaces model-name slash suggestions.
    pub fn set_model_suggestions(&mut self, models: Vec<String>) {
        self.model_suggestions = dedupe_sorted_nonempty(models);
        self.refresh_slash_picker();
    }

    /// Replaces branch snapshot slash suggestions.
    pub fn set_branch_suggestions(&mut self, branches: Vec<String>) {
        self.branch_suggestions = dedupe_sorted_nonempty(branches);
        self.refresh_slash_picker();
    }

    /// Adds one branch snapshot slash suggestion when it is not already present.
    pub fn add_branch_suggestion(&mut self, branch: &str) {
        let branch = branch.trim();
        if branch.is_empty()
            || self
                .branch_suggestions
                .iter()
                .any(|entry| entry.eq_ignore_ascii_case(branch))
        {
            return;
        }
        self.branch_suggestions.push(branch.to_string());
        self.branch_suggestions.sort();
    }

    /// Adds one model-name slash suggestion when it is not already present.
    pub fn add_model_suggestion(&mut self, model: &str) {
        let model = model.trim();
        if model.is_empty()
            || self
                .model_suggestions
                .iter()
                .any(|entry| entry.eq_ignore_ascii_case(model))
        {
            return;
        }
        self.model_suggestions.push(model.to_string());
        self.model_suggestions.sort();
    }

    /// Queues a free-form note that the host loop will append to the current
    /// session's `notes.ndjson` on the next tick.
    pub fn push_pending_note(&mut self, text: String) {
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        self.note_summary.count = self.note_summary.count.saturating_add(1);
        self.note_summary.latest = Some(text.clone());
        self.sync_current_session_note_summary();
        self.pending_notes.push(text);
    }

    /// Replaces the active-session note summary.
    pub fn set_note_summary(&mut self, count: usize, latest: Option<String>) {
        self.note_summary = NoteSummary {
            count,
            latest: latest.and_then(|value| {
                let value = value.trim().to_string();
                if value.is_empty() { None } else { Some(value) }
            }),
        };
        self.sync_current_session_note_summary();
    }

    /// Clears the active-session note summary.
    pub fn clear_note_summary(&mut self) {
        self.note_summary = NoteSummary::default();
        self.sync_current_session_note_summary();
    }

    /// Replaces the active note summary from the session directory, if known.
    pub fn hydrate_note_summary_from_directory(&mut self) {
        let Some(item) = self
            .sessions
            .iter()
            .find(|item| item.id == self.current_session_id)
        else {
            return;
        };
        self.note_summary = NoteSummary {
            count: item.notes_count,
            latest: item.last_note.clone(),
        };
    }

    fn sync_current_session_note_summary(&mut self) {
        if self.current_session_id.is_empty() {
            return;
        }
        if let Some(item) = self
            .sessions
            .iter_mut()
            .find(|item| item.id == self.current_session_id)
        {
            item.notes_count = self.note_summary.count;
            item.last_note = self.note_summary.latest.clone();
        }
    }

    /// Removes and returns every queued note in FIFO order.
    pub fn drain_pending_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_notes)
    }

    /// Counts background sessions (i.e. anything except the current foreground
    /// session) that currently have a pending approval / ask_user flag set.
    /// The status bar uses this to surface a `⚠ N sessions need attention`
    /// indicator.
    pub fn pending_attention_count(&self) -> usize {
        self.sessions
            .iter()
            .filter(|item| item.id != self.current_session_id && item.pending_attention)
            .count()
    }

    /// Sum of `cost_usd` across all tracked sessions.  For the live
    /// foreground session the header value is used (it updates on every
    /// `UsageUpdated` event) while directory items may lag.
    pub fn aggregate_cost_usd(&self) -> f64 {
        if self.sessions.is_empty() {
            return self.header.cost_usd;
        }
        self.sessions
            .iter()
            .map(|item| {
                if item.id == self.current_session_id {
                    self.header.cost_usd.max(item.cost_usd)
                } else {
                    item.cost_usd
                }
            })
            .sum()
    }

    /// Sum of `tokens` across all tracked sessions (same live-vs-directory
    /// strategy as `aggregate_cost_usd`).
    pub fn aggregate_tokens(&self) -> u64 {
        if self.sessions.is_empty() {
            return self.header.total_tokens;
        }
        self.sessions
            .iter()
            .map(|item| {
                if item.id == self.current_session_id {
                    self.header.total_tokens.max(item.tokens)
                } else {
                    item.tokens
                }
            })
            .sum()
    }

    /// Removes and returns every queued session-router intent in FIFO order.
    pub fn drain_pending_session_commands(&mut self) -> Vec<SessionCommandEvent> {
        std::mem::take(&mut self.pending_session_commands)
    }

    /// Updates the [`SessionDirectoryItem`](crate::SessionDirectoryItem) entry
    /// for a background session in response to a [`TuiRuntimeEvent`]. Foreground
    /// sessions should consume the event through
    /// [`apply_runtime_event`](Self::apply_runtime_event) instead — this method
    /// only tracks counters and attention flags for sessions the user is not
    /// currently watching.
    ///
    /// When the background session is a subagent of the foreground session
    /// (matching `parent_id`), a [`SubagentMonitorItem`] is also created or
    /// updated so the side panel tree reflects child progress inline.
    pub fn record_background_event(&mut self, session_id: &str, event: &TuiRuntimeEvent) {
        let mut subagent_update: Option<(String, Option<String>, String, String, u64)> = None;
        if let Some(item) = self.sessions.iter_mut().find(|item| item.id == session_id) {
            match event {
                TuiRuntimeEvent::RunStarted { .. } | TuiRuntimeEvent::TurnStarted { .. } => {
                    item.status = AgentRunStatus::Running;
                }
                TuiRuntimeEvent::Finished {
                    success,
                    stop_reason,
                    ..
                } => {
                    item.status = if *success {
                        AgentRunStatus::Succeeded
                    } else if stop_reason == "Interrupted" {
                        AgentRunStatus::Interrupted
                    } else {
                        AgentRunStatus::Failed
                    };
                }
                TuiRuntimeEvent::Interrupted { .. } => {
                    item.status = AgentRunStatus::Interrupted;
                }
                TuiRuntimeEvent::Failed { .. } => {
                    item.status = AgentRunStatus::Failed;
                }
                TuiRuntimeEvent::UsageUpdated {
                    total_tokens,
                    cost_usd,
                    ..
                } => {
                    item.tokens = *total_tokens;
                    item.cost_usd = *cost_usd;
                }
                TuiRuntimeEvent::ApprovalRequested { .. } => {
                    item.pending_attention = true;
                }
                TuiRuntimeEvent::SessionTitleUpdated { title, .. } => {
                    item.title = title.clone();
                    item.title_generated = true;
                }
                _ => {}
            }
            item.last_event_at_unix = current_unix_seconds();
            if item.parent_id.as_deref() == Some(self.current_session_id.as_str()) {
                let status = format!("{:?}", item.status).to_ascii_lowercase();
                subagent_update = Some((
                    item.id.clone(),
                    item.parent_id.clone(),
                    item.kind.clone().unwrap_or_else(|| "subagent".to_string()),
                    item.title.clone(),
                    item.tokens,
                ));
                let _ = status;
            }
        }
        if let Some((id, parent_id, kind, task, tokens)) = subagent_update {
            self.upsert_subagent_monitor(id, parent_id, kind, task, tokens);
        }
    }

    fn upsert_subagent_monitor(
        &mut self,
        id: String,
        parent_id: Option<String>,
        kind: String,
        task: String,
        tokens: u64,
    ) {
        let status_from_directory = self
            .sessions
            .iter()
            .find(|item| item.id == id)
            .map(|item| format!("{:?}", item.status).to_ascii_lowercase())
            .unwrap_or_else(|| "running".to_string());
        if let Some(monitor) = self.subagents.iter_mut().find(|item| item.id == id) {
            monitor.parent_id = parent_id;
            monitor.kind = kind;
            monitor.task = task;
            monitor.status = status_from_directory;
            monitor.tokens = tokens;
        } else {
            self.subagents.push(SubagentMonitorItem {
                kind,
                task,
                status: status_from_directory,
                summary: None,
                id,
                parent_id,
                depth: 1,
                started_at_unix: current_unix_seconds(),
                tokens,
            });
        }
    }

    /// Advances the spinner animation by one frame. While a task is running
    /// we also refresh `side_panel.stats.elapsed_seconds` from the
    /// `task_started_at_unix` anchor so the elapsed counter ticks in real time
    /// without the host loop having to push periodic events.
    pub fn tick_spinner(&mut self) {
        self.spinner_tick = self.spinner_tick.wrapping_add(1);
        if let Some(started) = self.task_started_at_unix {
            let now = current_unix_seconds();
            self.side_panel.stats.elapsed_seconds = now.saturating_sub(started);
        }
    }

    /// Returns the current braille spinner glyph.
    pub fn spinner_frame(&self) -> &'static str {
        const FRAMES: [&str; 10] = [
            "\u{280B}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283C}", "\u{2834}", "\u{2826}",
            "\u{2827}", "\u{2807}", "\u{280F}",
        ];
        FRAMES[(self.spinner_tick as usize) % FRAMES.len()]
    }

    /// Returns true when the agent is in a non-idle state that blocks new input.
    pub fn is_agent_busy(&self) -> bool {
        matches!(
            self.agent_run_status,
            AgentRunStatus::Running | AgentRunStatus::WaitingApproval
        )
    }

    /// Removes a tool name from the active list (matches the last occurrence).
    fn finish_active_tool(&mut self, tool_name: &str) {
        if let Some(pos) = self.active_tools.iter().rposition(|name| name == tool_name) {
            self.active_tools.remove(pos);
        }
    }

    /// Records a submitted input line for history navigation.
    pub fn record_input_history(&mut self, input: &str) {
        let input = input.trim();
        if input.is_empty() {
            return;
        }
        if let Some(index) = self.input_history.iter().position(|entry| entry == input) {
            self.input_history.remove(index);
        }
        self.input_history.push(input.to_string());
        if self.input_history.len() > MAX_INPUT_HISTORY_ENTRIES {
            let overflow = self.input_history.len() - MAX_INPUT_HISTORY_ENTRIES;
            self.input_history.drain(0..overflow);
        }
        self.input_history_cursor = None;
    }

    /// Replaces input with the previous history entry.
    pub fn previous_input_history(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        let index = self
            .input_history_cursor
            .map(|index| index.saturating_sub(1))
            .unwrap_or_else(|| self.input_history.len() - 1);
        self.input_history_cursor = Some(index);
        self.input = self.input_history[index].clone();
        self.input_cursor = self.input.chars().count();
        self.refresh_input_pickers();
    }

    /// Replaces input with the next history entry or clears it after the newest entry.
    pub fn next_input_history(&mut self) {
        let Some(index) = self.input_history_cursor else {
            return;
        };
        if index + 1 >= self.input_history.len() {
            self.input_history_cursor = None;
            self.input.clear();
            self.input_cursor = 0;
        } else {
            let next = index + 1;
            self.input_history_cursor = Some(next);
            self.input = self.input_history[next].clone();
            self.input_cursor = self.input.chars().count();
        }
        self.refresh_input_pickers();
    }

    /// Clears the input buffer and cursor.
    pub fn clear_input(&mut self) {
        self.input.clear();
        self.input_cursor = 0;
        self.input_history_cursor = None;
        self.refresh_input_pickers();
    }

    /// Wipes everything `/clear` should clear: transcript, side-panel
    /// counters, header totals, plan, panels, spinner state. The
    /// agent's conversation context lives in `peridot-core` and has
    /// to be cleared by the host via a `SessionCommandEvent` — but
    /// every UI surface the operator can SEE is reset here, so the
    /// post-clear screen looks like a fresh boot.
    pub fn reset_for_clear(&mut self) {
        self.transcript.clear();
        self.activities.clear();
        self.side_panel.stats = Default::default();
        self.side_panel.plan.clear();
        self.subagents.clear();
        self.header.total_tokens = 0;
        self.header.cache_hit_rate = 0.0;
        self.header.cost_usd = 0.0;
        self.active_tools.clear();
        self.active_stream = None;
        self.approval = None;
        self.ask_user = None;
        self.spinner_tick = 0;
        self.agent_run_status = AgentRunStatus::Idle;
        self.input_queue.clear();
    }

    /// Inserts one character at the current input cursor.
    pub fn insert_input_char(&mut self, character: char) {
        let byte_index = input_byte_index(&self.input, self.input_cursor);
        self.input.insert(byte_index, character);
        self.input_cursor += 1;
        self.input_history_cursor = None;
        self.refresh_input_pickers();
    }

    /// Refreshes every picker derived from the current input buffer.
    fn refresh_input_pickers(&mut self) {
        self.refresh_at_picker();
        self.refresh_slash_picker();
    }

    /// Examines the current input + cursor to decide whether the `@file`
    /// picker should be open. Called after any input mutation so the
    /// picker tracks the user's caret naturally — opening when the
    /// caret enters an `@token`, closing when it leaves.
    pub fn refresh_at_picker(&mut self) {
        let cursor = self.input_cursor;
        let token = crate::at_picker::current_at_token(&self.input, cursor);
        let Some((start, query)) = token else {
            self.at_picker = None;
            return;
        };
        match self.at_picker.as_mut() {
            Some(picker) => {
                picker.query = query;
                picker.token_start = start;
                picker.selected = 0;
            }
            None => {
                self.at_picker = Some(crate::at_picker::AtPicker {
                    query,
                    selected: 0,
                    token_start: start,
                });
            }
        }
    }

    /// Refreshes the slash-command picker from the current input buffer.
    ///
    /// Slash commands are line commands: the picker opens only while the
    /// draft begins with `/` and remains a single logical line. Once the
    /// operator adds arguments that no catalog entry can match, the picker
    /// closes naturally and Enter submits the command text.
    pub fn refresh_slash_picker(&mut self) {
        let query = self.input.clone();
        if !query.starts_with('/') || query.contains('\n') {
            self.slash_picker = None;
            return;
        }
        let len = crate::slash_picker::picker_len_with_dynamic_and_files(
            &query,
            crate::slash_picker::SlashDynamicSources {
                skills: &self.skill_suggestions,
                sessions: &self.sessions,
                mcp_servers: &self.side_panel.mcp_status,
                models: &self.model_suggestions,
                branches: &self.branch_suggestions,
                files: &self.at_picker_index,
                attachment_paths: &self.attachment_paths,
            },
        );
        if len == 0 {
            self.slash_picker = None;
            return;
        }
        match self.slash_picker.as_mut() {
            Some(picker) => {
                picker.query = query;
                picker.selected = picker.selected.min(len.saturating_sub(1));
            }
            None => {
                self.slash_picker = Some(SlashPicker { query, selected: 0 });
            }
        }
    }

    /// Moves the slash picker selection by one row.
    pub fn move_slash_picker_selection(&mut self, delta: isize) {
        let Some(picker) = self.slash_picker.as_mut() else {
            return;
        };
        let len = crate::slash_picker::picker_len_with_dynamic_and_files(
            &picker.query,
            crate::slash_picker::SlashDynamicSources {
                skills: &self.skill_suggestions,
                sessions: &self.sessions,
                mcp_servers: &self.side_panel.mcp_status,
                models: &self.model_suggestions,
                branches: &self.branch_suggestions,
                files: &self.at_picker_index,
                attachment_paths: &self.attachment_paths,
            },
        );
        if len == 0 {
            picker.selected = 0;
            return;
        }
        let current = picker.selected.min(len - 1) as isize;
        picker.selected = (current + delta).clamp(0, len as isize - 1) as usize;
    }

    /// Completes the current input from the highlighted slash command.
    pub fn accept_slash_picker(&mut self) {
        let Some(picker) = self.slash_picker.as_ref() else {
            return;
        };
        let query = picker.query.clone();
        let selected = picker.selected;
        if let Some(context) = crate::slash_picker::slash_argument_context_with_dynamic_and_files(
            &query,
            crate::slash_picker::SlashDynamicSources {
                skills: &self.skill_suggestions,
                sessions: &self.sessions,
                mcp_servers: &self.side_panel.mcp_status,
                models: &self.model_suggestions,
                branches: &self.branch_suggestions,
                files: &self.at_picker_index,
                attachment_paths: &self.attachment_paths,
            },
        ) {
            let Some(option) = context
                .options
                .get(selected.min(context.options.len().saturating_sub(1)))
            else {
                return;
            };
            self.input = format!(
                "{} {option}{}",
                context.command_name,
                if context.append_space { " " } else { "" }
            );
            self.input_cursor = self.input.chars().count();
            self.refresh_input_pickers();
            return;
        }

        let matches = crate::slash_picker::filtered_suggestions(&query, &self.skill_suggestions);
        let Some(spec) = matches.get(picker.selected) else {
            return;
        };
        self.input =
            crate::slash_picker::accepted_command_text(&spec.name, spec.arg_hint.as_deref());
        self.input_cursor = self.input.chars().count();
        self.refresh_input_pickers();
    }

    /// Returns true when Enter should submit rather than complete the picker.
    pub fn slash_picker_exact_selection_is_runnable(&self) -> bool {
        let Some(picker) = self.slash_picker.as_ref() else {
            return false;
        };
        if crate::slash_picker::slash_argument_context_with_dynamic_and_files(
            &picker.query,
            crate::slash_picker::SlashDynamicSources {
                skills: &self.skill_suggestions,
                sessions: &self.sessions,
                mcp_servers: &self.side_panel.mcp_status,
                models: &self.model_suggestions,
                branches: &self.branch_suggestions,
                files: &self.at_picker_index,
                attachment_paths: &self.attachment_paths,
            },
        )
        .is_some()
        {
            return false;
        }
        let matches =
            crate::slash_picker::filtered_suggestions(&picker.query, &self.skill_suggestions);
        let Some(spec) = matches.get(picker.selected) else {
            return false;
        };
        let input = self.input.trim();
        input == spec.name
            && spec
                .arg_hint
                .as_deref()
                .is_none_or(|hint| hint.starts_with('['))
    }

    /// Refreshes the cached file index that powers the `@file` picker.
    /// Called lazily the first time the picker needs to render so we
    /// don't pay the scan cost on startup for users who never invoke it.
    pub fn ensure_at_picker_index(&mut self, project_root: &std::path::Path) {
        if self.at_picker_index.is_empty() {
            self.refresh_at_picker_index(project_root);
        }
    }

    /// Rebuilds the cached file index that powers the `@file` picker.
    /// This is intentionally separate from [`Self::ensure_at_picker_index`]
    /// so hosts can keep a long-lived TUI session fresh after files are
    /// created or deleted.
    pub fn refresh_at_picker_index(&mut self, project_root: &std::path::Path) {
        self.at_picker_index = crate::at_picker::build_file_index(project_root, 5_000);
    }

    /// Replaces the cached attachment path list used by `/detach` autocomplete.
    pub fn set_attachment_paths(&mut self, paths: Vec<String>) {
        self.attachment_paths = dedupe_sorted_nonempty(paths);
        self.refresh_slash_picker();
    }

    /// Adds one attached path to the `/detach` autocomplete cache.
    pub fn add_attachment_path(&mut self, path: impl Into<String>) {
        let mut paths = self.attachment_paths.clone();
        paths.push(path.into());
        self.set_attachment_paths(paths);
    }

    /// Removes paths from the `/detach` autocomplete cache.
    pub fn remove_attachment_paths(&mut self, paths: &[String]) {
        if paths.is_empty() {
            return;
        }
        self.attachment_paths.retain(|candidate| {
            !paths
                .iter()
                .any(|removed| candidate.eq_ignore_ascii_case(removed))
        });
        self.refresh_slash_picker();
    }

    /// Replaces the active `@token` with the picker's currently-highlighted
    /// suggestion. No-op when the picker is closed or has no matches.
    pub fn accept_at_picker(&mut self) {
        let Some(picker) = self.at_picker.take() else {
            return;
        };
        let matches = crate::at_picker::filter_paths(&self.at_picker_index, &picker.query);
        let Some(choice) = matches.get(picker.selected) else {
            return;
        };
        let chosen = (*choice).clone();
        let cursor_byte = input_byte_index(&self.input, self.input_cursor);
        self.input
            .replace_range(picker.token_start..cursor_byte, &format!("@{chosen} "));
        let inserted_chars = chosen.chars().count() + 2; // '@' + path + ' '
        let token_start_chars = self.input[..picker.token_start].chars().count();
        self.input_cursor = token_start_chars + inserted_chars;
        self.refresh_input_pickers();
    }

    /// Removes the character before the current input cursor.
    pub fn backspace_input(&mut self) {
        let cursor_before = self.input_cursor;
        if cursor_before == 0 {
            return;
        }
        let start = input_byte_index(&self.input, self.input_cursor - 1);
        let end = input_byte_index(&self.input, self.input_cursor);
        self.input.replace_range(start..end, "");
        self.input_cursor -= 1;
        self.input_history_cursor = None;
        self.refresh_input_pickers();
    }

    /// Removes the character at the current input cursor.
    pub fn delete_input_char(&mut self) {
        let input_len = self.input.chars().count();
        if self.input_cursor >= input_len {
            return;
        }
        let start = input_byte_index(&self.input, self.input_cursor);
        let end = input_byte_index(&self.input, self.input_cursor + 1);
        self.input.replace_range(start..end, "");
        self.input_history_cursor = None;
        self.refresh_input_pickers();
    }

    /// Moves the input cursor one character left.
    pub fn move_input_cursor_left(&mut self) {
        self.input_cursor = self.input_cursor.saturating_sub(1);
        self.refresh_input_pickers();
    }

    /// Moves the input cursor one character right.
    pub fn move_input_cursor_right(&mut self) {
        self.input_cursor = (self.input_cursor + 1).min(self.input.chars().count());
        self.refresh_input_pickers();
    }

    /// Moves the input cursor to the start.
    pub fn move_input_cursor_home(&mut self) {
        self.input_cursor = 0;
        self.refresh_input_pickers();
    }

    /// Moves the input cursor to the end.
    pub fn move_input_cursor_end(&mut self) {
        self.input_cursor = self.input.chars().count();
        self.refresh_input_pickers();
    }

    /// Marks an agent task as running. Resets the elapsed counter and stamps
    /// the wall-clock anchor so the status bar can tick second-by-second
    /// without external events.
    pub fn mark_agent_running(&mut self, task: impl Into<String>) {
        let task = task.into();
        self.agent_run_status = AgentRunStatus::Running;
        self.last_task = Some(task.clone());
        self.task_started_at_unix = Some(current_unix_seconds());
        self.side_panel.stats.elapsed_seconds = 0;
        self.begin_stream("assistant");
        self.push_activity(ActivityKind::Stream, "run", format!("running: {task}"));
        self.push_transcript_entry(TranscriptKind::Meta, format!("task: {task}"));
    }

    /// Marks the active agent task as completed. Freezes the elapsed counter
    /// on whatever value the run finished at (i.e. clears the running anchor
    /// so `tick_spinner` stops bumping it forward).
    pub fn mark_agent_succeeded(&mut self, summary: impl Into<String>) {
        self.agent_run_status = AgentRunStatus::Succeeded;
        self.active_tools.clear();
        self.task_started_at_unix = None;
        self.push_activity(ActivityKind::Stream, "run", "done");
        self.push_transcript_entry(TranscriptKind::Meta, format!("run: {}", summary.into()));
    }

    /// Marks the active agent task as failed. Also freezes the elapsed
    /// counter so the failure timing stays visible.
    pub fn mark_agent_failed(&mut self, message: impl Into<String>) {
        self.agent_run_status = AgentRunStatus::Failed;
        self.active_tools.clear();
        self.task_started_at_unix = None;
        self.side_panel.stats.errors += 1;
        self.push_activity(ActivityKind::Stream, "run", "failed");
        self.push_transcript_entry(
            TranscriptKind::Error,
            format!("run failed: {}", message.into()),
        );
    }

    /// Starts or replaces the active model stream.
    pub fn begin_stream(&mut self, label: impl Into<String>) {
        let label = label.into();
        self.active_stream = Some(StreamState {
            label: label.clone(),
            content: String::new(),
            done: false,
        });
        self.push_activity(ActivityKind::Stream, label, "streaming");
    }

    /// Appends a model stream delta to the active stream.
    pub fn push_stream_delta(&mut self, delta: &str) {
        if self.active_stream.is_none() {
            self.begin_stream("assistant");
        }
        if let Some(stream) = self.active_stream.as_mut() {
            stream.content.push_str(delta);
        }
    }

    /// Finishes the active stream and appends it to the transcript.
    pub fn finish_stream(&mut self) {
        let Some(mut stream) = self.active_stream.take() else {
            return;
        };
        stream.done = true;
        let content = stream.content.trim();
        if content.is_empty() {
            self.push_transcript_entry(TranscriptKind::Debug, format!("{}: <empty>", stream.label));
        } else {
            let parsed = parse_assistant_content(content);
            if let Some(visible) = parsed.display.as_ref() {
                self.push_transcript_entry(TranscriptKind::Assistant, visible.clone());
            }
            if self.debug_view {
                let summary = truncate_for_debug(content, 80);
                self.push_transcript_entry(
                    TranscriptKind::Debug,
                    format!("{} raw: {summary}", stream.label),
                );
            }
        }
        self.push_activity(ActivityKind::Stream, stream.label, "done");
        self.side_panel.stats.steps += 1;
    }

    /// Records one tool execution in the visible activity list.
    pub fn record_tool_started(
        &mut self,
        tool_name: impl Into<String>,
        parameters: serde_json::Value,
    ) {
        let tool_name = tool_name.into();
        self.active_tools.push(tool_name.clone());
        self.push_activity(ActivityKind::Tool, tool_name.clone(), "running");
        self.push_transcript_entry(TranscriptKind::ToolStart, format!("{tool_name}  running"));
        for line in tool_parameter_preview(&tool_name, &parameters) {
            self.push_transcript_entry(TranscriptKind::ToolStart, line);
        }
        // Per-line diff rendering used to fire from `parameters` here for
        // `file_patch` only — `file_write` couldn't participate because the
        // tool only carries the new content. The harness now emits a
        // dedicated `FileDiff` event after the mutation completes, so both
        // tools render through `record_file_diff` and the inline path is
        // gone.
    }

    /// Materialises a [`FileDiffPayload`] into per-line transcript entries
    /// so the chat view shows a real unified diff for both `file_write`
    /// and `file_patch`.
    ///
    /// Each `Diff` entry holds a single `- old` / `+ new` line so the
    /// renderer can colour them independently (red / green) and clip a
    /// long patch cleanly. The cap matches the legacy inline path so
    /// large mass-rewrites don't drown the transcript.
    pub fn record_file_diff(&mut self, payload: FileDiffPayload) {
        const DIFF_RENDER_LIMIT: usize = 40;
        let header = match payload.before.as_deref() {
            None => format!("diff: {} (new file)", payload.path),
            Some(_) => format!("diff: {}", payload.path),
        };
        self.push_transcript_entry(TranscriptKind::Diff, header);
        let before = payload.before.as_deref().unwrap_or("");
        let hunks = crate::diff_hunks::diff_hunks(before, &payload.after);
        let mut emitted = 0usize;
        let mut clipped = 0usize;
        for hunk in &hunks {
            for line in &hunk.old_lines {
                if emitted >= DIFF_RENDER_LIMIT {
                    clipped += 1;
                    continue;
                }
                self.push_transcript_entry(TranscriptKind::Diff, format!("- {line}"));
                emitted += 1;
            }
            for line in &hunk.new_lines {
                if emitted >= DIFF_RENDER_LIMIT {
                    clipped += 1;
                    continue;
                }
                self.push_transcript_entry(TranscriptKind::Diff, format!("+ {line}"));
                emitted += 1;
            }
        }
        if clipped > 0 {
            self.push_transcript_entry(
                TranscriptKind::Diff,
                format!("... +{clipped} more diff lines"),
            );
        }
    }

    /// Records one tool execution in the visible activity list.
    pub fn record_tool_activity(
        &mut self,
        tool_name: impl Into<String>,
        success: bool,
        summary: impl Into<String>,
    ) {
        let status = if success { "ok" } else { "failed" };
        if !success {
            self.side_panel.stats.errors += 1;
        }
        self.side_panel.stats.steps += 1;
        self.push_activity(
            ActivityKind::Tool,
            tool_name.into(),
            format!("{status}: {}", summary.into()),
        );
    }

    /// Records one completed tool execution in activity and transcript.
    pub fn record_tool_result(
        &mut self,
        tool_name: impl Into<String>,
        success: bool,
        summary: impl Into<String>,
        output: serde_json::Value,
    ) {
        let tool_name = tool_name.into();
        let summary = summary.into();
        self.finish_active_tool(&tool_name);
        self.record_tool_activity(tool_name.clone(), success, summary.clone());
        // `agent_done` is a synthetic call: when the model replies with plain text we
        // promote the reply to an `agent_done(summary=text)` invocation so the loop
        // can complete. The assistant text was already pushed as a chat entry by
        // `finish_stream`, so echoing it again under a green check would be visual
        // noise. Only show the `agent_done` line when its summary differs from the
        // most recent assistant entry (e.g. an explicit completion summary).
        let kind = if success {
            TranscriptKind::ToolOk
        } else {
            TranscriptKind::ToolFail
        };
        let suppress_duplicate = tool_name == "agent_done"
            && success
            && self.last_assistant_text_matches(summary.trim());
        if !suppress_duplicate {
            if tool_name == "agent_done" && success {
                self.push_transcript_entry(TranscriptKind::Assistant, summary);
                return;
            }
            self.push_transcript_entry(kind, format!("{tool_name}  {summary}"));
            for line in tool_output_preview(&tool_name, &output) {
                self.push_transcript_entry(kind, line);
            }
        }
    }

    /// Returns true when the most recent transcript entry is an assistant message
    /// whose trimmed body equals `candidate`. Used to suppress redundant
    /// `agent_done` echoes of the chat text already shown above.
    fn last_assistant_text_matches(&self, candidate: &str) -> bool {
        self.transcript
            .iter()
            .rev()
            .find(|entry| {
                !matches!(
                    entry.kind,
                    TranscriptKind::Debug
                        | TranscriptKind::Thinking
                        | TranscriptKind::TurnSeparator
                )
            })
            .map(|entry| entry.kind == TranscriptKind::Assistant && entry.text.trim() == candidate)
            .unwrap_or(false)
    }

    /// Records one verification stage in the visible activity list.
    pub fn record_verification_activity(
        &mut self,
        stage: impl Into<String>,
        success: bool,
        summary: impl Into<String>,
    ) {
        let status = if success { "passed" } else { "failed" };
        if !success {
            self.side_panel.stats.errors += 1;
        }
        self.side_panel.stats.steps += 1;
        self.push_activity(
            ActivityKind::Verification,
            stage.into(),
            format!("{status}: {}", summary.into()),
        );
    }

    /// Records a subagent delegation as running.
    pub fn record_subagent_started(&mut self, kind: impl Into<String>, task: impl Into<String>) {
        let kind = kind.into();
        let task = task.into();
        self.subagents.push(SubagentMonitorItem {
            kind: kind.clone(),
            task: task.clone(),
            status: "running".to_string(),
            summary: None,
            id: String::new(),
            parent_id: None,
            depth: 0,
            started_at_unix: 0,
            tokens: 0,
        });
        self.push_activity(ActivityKind::Subagent, kind, format!("running: {task}"));
        self.trim_subagents();
    }

    /// Marks a subagent delegation as completed.
    pub fn record_subagent_completed(
        &mut self,
        kind: impl Into<String>,
        task: impl Into<String>,
        summary: impl Into<String>,
    ) {
        self.record_subagent_finished(kind, task, "done", summary);
    }

    /// Marks a subagent delegation as failed.
    pub fn record_subagent_failed(
        &mut self,
        kind: impl Into<String>,
        task: impl Into<String>,
        summary: impl Into<String>,
    ) {
        self.record_subagent_finished(kind, task, "failed", summary);
        self.side_panel.stats.errors += 1;
    }

    /// Parses the current input as a slash command when possible.
    pub fn current_slash_command(&self) -> Option<SlashCommand> {
        parse_slash_command(&self.input)
    }

    /// Opens an ask-user panel.
    pub fn open_ask_user(&mut self, request: AskUserRequest) {
        self.ask_user = Some(AskUserPanel::from_request(request));
    }

    /// Opens an ask-user panel tied to an `AskUserPort` correlation id.
    /// The panel records the id and surfaces it back through
    /// `TuiEventOutcome::AskUserResolved` when the operator commits, so
    /// the CLI can fulfil the matching oneshot.
    pub fn open_ask_user_with_id(
        &mut self,
        request_id: impl Into<String>,
        request: AskUserRequest,
    ) {
        self.ask_user = Some(AskUserPanel::from_request(request).with_request_id(request_id));
    }

    /// Opens a tool approval panel.
    pub fn open_approval(
        &mut self,
        tool_name: impl Into<String>,
        reason: impl Into<String>,
        parameters: serde_json::Value,
        risk_class: Option<String>,
    ) {
        let tool_name = tool_name.into();
        let reason = reason.into();
        self.agent_run_status = AgentRunStatus::WaitingApproval;
        self.push_activity(
            ActivityKind::Tool,
            tool_name.clone(),
            format!("approval required: {reason}"),
        );
        let diff = compute_approval_diff_preview(&tool_name, &parameters);
        self.approval = Some(
            ApprovalPanel::new(tool_name, reason)
                .with_parameters(parameters)
                .with_risk_class(risk_class)
                .with_diff_preview(diff),
        );
    }

    /// Records a user approval decision and closes the approval panel.
    pub fn record_approval_decision(&mut self, decision: ApprovalDecision) {
        let Some(panel) = self.approval.take() else {
            return;
        };
        let label = match decision {
            ApprovalDecision::Approve => "approved",
            ApprovalDecision::Deny => "denied",
        };
        self.push_activity(ActivityKind::Tool, panel.tool_name.clone(), label);
        let kind = match decision {
            ApprovalDecision::Approve => TranscriptKind::Notice,
            ApprovalDecision::Deny => TranscriptKind::Error,
        };
        self.push_transcript_entry(
            kind,
            format!("approval: {} {label} ({})", panel.tool_name, panel.reason),
        );
        if self.agent_run_status == AgentRunStatus::WaitingApproval {
            self.agent_run_status = AgentRunStatus::Running;
        }
    }

    /// Applies an event from the background agent worker.
    pub fn apply_runtime_event(&mut self, event: TuiRuntimeEvent) {
        match event {
            TuiRuntimeEvent::RunStarted { task } => self.mark_agent_running(task),
            TuiRuntimeEvent::TurnStarted { turn_index } => {
                self.current_turn = turn_index;
                self.push_activity(ActivityKind::Stream, "turn", format!("#{}", turn_index + 1));
                if !self.transcript.is_empty() {
                    self.push_transcript_entry(
                        TranscriptKind::TurnSeparator,
                        format!("turn {}", turn_index + 1),
                    );
                }
            }
            TuiRuntimeEvent::AssistantStarted { label } => self.begin_stream(label),
            TuiRuntimeEvent::AssistantDelta { delta } => self.push_stream_delta(&delta),
            TuiRuntimeEvent::AssistantFinished => self.finish_stream(),
            TuiRuntimeEvent::Thinking { text } => {
                self.push_activity(ActivityKind::Stream, "thinking", "parsed");
                self.thinking_log.push(text.clone());
                self.push_transcript_entry(TranscriptKind::Thinking, format!("thinking: {text}"));
            }
            TuiRuntimeEvent::ToolStarted { name, parameters } => {
                self.record_tool_started(name, parameters);
            }
            TuiRuntimeEvent::ToolFinished {
                name,
                success,
                summary,
                output,
            } => self.record_tool_result(name, success, summary, output),
            TuiRuntimeEvent::FileDiff(payload) => {
                self.record_file_diff(payload);
            }
            TuiRuntimeEvent::AskUserRequested {
                request_id,
                request,
            } => {
                self.open_ask_user_with_id(request_id, request);
            }
            TuiRuntimeEvent::ApprovalRequested {
                tool_name,
                reason,
                parameters,
                risk_class,
            } => {
                self.open_approval(tool_name, reason, parameters, risk_class);
            }
            TuiRuntimeEvent::BranchPickerTurns { turns } => {
                if let Some(picker) = self.branch_picker.as_mut() {
                    picker.populate(turns);
                }
            }
            TuiRuntimeEvent::UsageUpdated {
                total_tokens,
                cache_hit_rate,
                cost_usd,
            } => {
                self.header.total_tokens = total_tokens;
                self.header.cache_hit_rate = cache_hit_rate;
                self.header.cost_usd = cost_usd;
            }
            TuiRuntimeEvent::Recovery { message } => {
                self.side_panel.stats.errors += 1;
                self.push_activity(ActivityKind::Verification, "recovery", message);
            }
            TuiRuntimeEvent::PhaseChanged { from, to, reason } => {
                // Phase transitions are observational — surface them in the
                // activity panel so the operator can see the harness's
                // state-machine progression, but don't push them as
                // transcript entries (they'd be too chatty).
                self.push_activity(
                    ActivityKind::Verification,
                    "phase",
                    format!(
                        "{} → {} ({reason})",
                        phase_display_label(&from),
                        phase_display_label(&to)
                    ),
                );
            }
            TuiRuntimeEvent::ContextCompacted {
                narrative,
                files_read_count,
                untrusted_count,
            } => {
                // Compactions are infrequent and high-signal — surface
                // them in the activity panel with a concise structured
                // marker (`compact: N files read, M untrusted`) plus
                // the LLM narrative when present.
                let label =
                    format!("compact: {files_read_count} files read, {untrusted_count} untrusted");
                let detail = if narrative.is_empty() {
                    label.clone()
                } else {
                    format!("{label}\n{narrative}")
                };
                self.push_activity(ActivityKind::Verification, "compact", detail);
            }
            TuiRuntimeEvent::SkillSuggestionsUpdated { skills } => {
                self.set_skill_suggestions(skills);
            }
            TuiRuntimeEvent::Finished {
                stop_reason,
                turns,
                success,
                duration_ms,
            } => {
                // Sync the side-panel elapsed counter to the agent's reported
                // duration so the `⏱ completed in 17s` stamp matches the
                // `elapsed: 17s` Status block. The TUI's view starts a beat
                // earlier (at `mark_agent_running`, before provider setup),
                // so without this they'd disagree by a few seconds.
                if duration_ms > 0 {
                    self.side_panel.stats.elapsed_seconds = duration_ms / 1000;
                }
                let duration = format_duration_ms(duration_ms);
                if self.approval.is_some() {
                    self.push_transcript_entry(
                        TranscriptKind::Meta,
                        format!("run: stopped={stop_reason} turns={turns} duration={duration}"),
                    );
                    self.agent_run_status = AgentRunStatus::WaitingApproval;
                    return;
                }
                if self.agent_run_status == AgentRunStatus::Interrupted {
                    self.push_transcript_entry(
                        TranscriptKind::Meta,
                        format!("run: stopped={stop_reason} turns={turns} duration={duration}"),
                    );
                    return;
                }
                // Use the Assistant kind for the visible completion stamp so
                // it stays in the chat view (System entries are now hidden).
                // No glyph prefix — `⏱` (U+23F1) is half-width in many WSL
                // fonts and clipped the duration digits that followed it.
                let stamp = if success {
                    format!(
                        "completed in {duration} ({turns} turn{})",
                        if turns == 1 { "" } else { "s" }
                    )
                } else {
                    format!(
                        "stopped: {stop_reason} after {duration} ({turns} turn{})",
                        if turns == 1 { "" } else { "s" }
                    )
                };
                self.push_transcript_entry(TranscriptKind::Assistant, stamp);
                if success {
                    self.mark_agent_succeeded(format!(
                        "stopped={stop_reason} turns={turns} duration={duration}"
                    ));
                } else {
                    self.mark_agent_failed(format!(
                        "stopped={stop_reason} turns={turns} duration={duration}"
                    ));
                }
            }
            TuiRuntimeEvent::SessionSaved { session_id } => {
                self.push_activity(
                    ActivityKind::Stream,
                    "session",
                    format!("saved: {session_id}"),
                );
                self.push_transcript_entry(
                    TranscriptKind::Meta,
                    format!(
                        "session: saved {session_id}  ·  resume with: peridot session resume {session_id}"
                    ),
                );
            }
            TuiRuntimeEvent::SessionSaveFailed {
                session_id,
                message,
            } => {
                self.side_panel.stats.errors += 1;
                self.push_activity(
                    ActivityKind::Stream,
                    "session",
                    format!("save failed: {session_id}"),
                );
                self.push_transcript_entry(
                    TranscriptKind::Error,
                    format!("session: failed to save {session_id}: {message}"),
                );
            }
            TuiRuntimeEvent::Failed { message } => self.mark_agent_failed(message),
            TuiRuntimeEvent::TurnEnded {
                turn_index,
                success,
            } => {
                let status = if success { "done" } else { "failed" };
                self.push_activity(
                    ActivityKind::Stream,
                    "turn",
                    format!("end #{} ({status})", turn_index + 1),
                );
            }
            TuiRuntimeEvent::PlanUpdated { steps, current } => {
                self.side_panel.plan = steps
                    .into_iter()
                    .map(|step| PlanStep {
                        label: step.label,
                        done: step.done,
                    })
                    .collect();
                if let Some(idx) = current {
                    self.push_activity(ActivityKind::Stream, "plan", format!("step {}", idx + 1));
                } else {
                    self.push_activity(ActivityKind::Stream, "plan", "updated");
                }
            }
            TuiRuntimeEvent::BudgetUpdated {
                cost_used,
                cost_limit,
                turns_used,
                turns_limit,
            } => {
                self.side_panel.budget = BudgetGauge {
                    cost_used,
                    cost_limit,
                    turns_used,
                    turns_limit,
                };
            }
            TuiRuntimeEvent::ContextUtilizationChanged {
                tokens_used,
                threshold,
                context_tokens,
                message_tokens,
                system_tokens,
                tool_schema_tokens,
                overhead_tokens,
            } => {
                self.side_panel.context_tokens_used = tokens_used as usize;
                self.side_panel.context_tokens_window = threshold as usize;
                self.side_panel.context_entry_tokens = context_tokens as usize;
                self.side_panel.context_message_tokens = message_tokens as usize;
                self.side_panel.context_system_tokens = system_tokens as usize;
                self.side_panel.context_tool_schema_tokens = tool_schema_tokens as usize;
                self.side_panel.context_overhead_tokens = overhead_tokens as usize;
                if threshold > 0 {
                    self.side_panel.context_pct =
                        (tokens_used as f32 / threshold as f32).clamp(0.0, 1.0);
                }
            }
            TuiRuntimeEvent::McpStatusChanged { servers } => {
                self.side_panel.mcp_status = servers;
            }
            TuiRuntimeEvent::AgentsMdLoaded { rule_count, paths } => {
                self.side_panel.agents_md = AgentsSummary { rule_count, paths };
            }
            TuiRuntimeEvent::HookFired {
                name,
                category,
                outcome,
            } => {
                self.push_activity(
                    ActivityKind::Stream,
                    format!("hook:{name}"),
                    format!("{category}: {outcome}"),
                );
            }
            TuiRuntimeEvent::Interrupted { stage } => {
                self.agent_run_status = AgentRunStatus::Interrupted;
                self.active_tools.clear();
                self.push_transcript_entry(
                    TranscriptKind::Notice,
                    format!("interrupted during {stage}"),
                );
                self.push_activity(ActivityKind::Stream, "run", "interrupted");
            }
            TuiRuntimeEvent::PlannerPlanReady { plan_text } => {
                self.push_transcript_entry(
                    TranscriptKind::System,
                    format!("committee planner ready:\n{plan_text}"),
                );
                self.push_activity(ActivityKind::Stream, "committee planner", "plan ready");
                let ts = current_unix_seconds();
                self.pending_committee_events.push(serde_json::json!({
                    "ts": ts,
                    "kind": "planner_plan_ready",
                    "plan_text": plan_text,
                }));
            }
            TuiRuntimeEvent::AutoFixAttempt {
                attempt,
                max,
                tool_name,
                passed,
            } => {
                let status = if passed { "passed" } else { "FAILED" };
                self.push_transcript_entry(
                    if passed {
                        TranscriptKind::System
                    } else {
                        TranscriptKind::Notice
                    },
                    format!("autofix: {tool_name} {status} (attempt {attempt}/{max})"),
                );
                self.push_activity(
                    ActivityKind::Verification,
                    "autofix",
                    format!("{attempt}/{max} {status}"),
                );
            }
            TuiRuntimeEvent::CommitteeRoleUsage {
                role,
                cost_usd,
                tokens,
            } => {
                match role.as_str() {
                    "planner" => {
                        self.committee_planner_cost += cost_usd;
                        self.committee_planner_tokens += tokens;
                    }
                    "reviewer" => {
                        self.committee_reviewer_cost += cost_usd;
                        self.committee_reviewer_tokens += tokens;
                    }
                    _ => {}
                }
                self.push_activity(
                    ActivityKind::Stream,
                    format!("committee {role}"),
                    format!("+${cost_usd:.4} / +{tokens} tok"),
                );
                let ts = current_unix_seconds();
                self.pending_committee_events.push(serde_json::json!({
                    "ts": ts,
                    "kind": "role_usage",
                    "role": role,
                    "cost_usd": cost_usd,
                    "tokens": tokens,
                }));
            }
            TuiRuntimeEvent::ReviewerVerdict {
                turn_index,
                verdict,
                comments,
            } => {
                let summary = if comments.is_empty() {
                    format!("committee reviewer (turn {turn_index}): {verdict}")
                } else {
                    format!("committee reviewer (turn {turn_index}): {verdict} — {comments}")
                };
                let kind = match verdict.as_str() {
                    "approve" => TranscriptKind::System,
                    "request_changes" => TranscriptKind::Notice,
                    "block" => TranscriptKind::Error,
                    _ => TranscriptKind::Notice,
                };
                self.push_transcript_entry(kind, summary);
                self.push_activity(ActivityKind::Stream, "committee reviewer", &verdict);
                let ts = current_unix_seconds();
                self.pending_committee_events.push(serde_json::json!({
                    "ts": ts,
                    "kind": "reviewer_verdict",
                    "turn_index": turn_index,
                    "verdict": verdict,
                    "comments": comments,
                }));
            }
            TuiRuntimeEvent::SessionTitleUpdated { session_id, title } => {
                if let Some(item) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                    item.title = title;
                    item.title_generated = true;
                }
            }
        }
    }

    fn push_activity(
        &mut self,
        kind: ActivityKind,
        label: impl Into<String>,
        status: impl Into<String>,
    ) {
        self.activities.push(RuntimeActivity {
            kind,
            label: label.into(),
            status: status.into(),
        });
        if self.activities.len() > 8 {
            let overflow = self.activities.len() - 8;
            self.activities.drain(0..overflow);
        }
    }

    fn record_subagent_finished(
        &mut self,
        kind: impl Into<String>,
        task: impl Into<String>,
        status: &str,
        summary: impl Into<String>,
    ) {
        let kind = kind.into();
        let task = task.into();
        let summary = summary.into();
        if let Some(item) = self
            .subagents
            .iter_mut()
            .rev()
            .find(|item| item.kind == kind && item.task == task)
        {
            item.status = status.to_string();
            item.summary = Some(summary.clone());
        } else {
            self.subagents.push(SubagentMonitorItem {
                kind: kind.clone(),
                task: task.clone(),
                status: status.to_string(),
                summary: Some(summary.clone()),
                id: String::new(),
                parent_id: None,
                depth: 0,
                started_at_unix: 0,
                tokens: 0,
            });
        }
        self.side_panel.stats.steps += 1;
        self.push_activity(ActivityKind::Subagent, kind, format!("{status}: {summary}"));
        self.trim_subagents();
    }

    fn trim_subagents(&mut self) {
        if self.subagents.len() > 6 {
            let overflow = self.subagents.len() - 6;
            self.subagents.drain(0..overflow);
        }
    }
}

/// Builds a short diff preview for tools whose approval payload contains old/new text.
pub(super) fn compute_approval_diff_preview(
    tool_name: &str,
    parameters: &serde_json::Value,
) -> Option<String> {
    if tool_name != "file_patch" {
        return None;
    }
    let old_text = parameters.get("old_text").and_then(|v| v.as_str())?;
    let new_text = parameters.get("new_text").and_then(|v| v.as_str())?;
    let mut lines = Vec::new();
    for line in old_text.lines().take(6) {
        lines.push(format!("- {line}"));
    }
    for line in new_text.lines().take(6) {
        lines.push(format!("+ {line}"));
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

/// Truncates raw debug content to `max_chars` characters with an ellipsis suffix when shortened.
fn truncate_for_debug(text: &str, max_chars: usize) -> String {
    let collapsed: String = text.lines().collect::<Vec<_>>().join(" \u{21B5} ");
    let mut chars = collapsed.chars();
    let head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{head}...")
    } else {
        head
    }
}

/// Parsed view of an assistant message split into a user-visible line and the raw payload.
pub(super) struct ParsedAssistant {
    pub display: Option<String>,
}

/// Extracts the user-visible portion of an assistant message.
///
/// If the message ends in a JSON action block, the action drives what (if anything) is shown:
/// `agent_ask_user` surfaces the question, `agent_done` the summary, and tool-call actions
/// produce no visible line because the tool events already report them. Free-form text without
/// a JSON action is shown as-is.
pub(super) fn parse_assistant_content(content: &str) -> ParsedAssistant {
    if let Some(json_str) = last_balanced_json_object(content)
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&json_str)
        && let Some(action) = value.get("action").and_then(serde_json::Value::as_str)
    {
        let params = value.get("parameters");
        let display = match action {
            "agent_ask_user" => params
                .and_then(|p| {
                    p.get("question")
                        .or_else(|| p.get("prompt"))
                        .or_else(|| p.get("message"))
                })
                .and_then(serde_json::Value::as_str)
                .map(|text| format!("ask: {text}")),
            "agent_done" => Some(
                params
                    .and_then(|p| p.get("summary").or_else(|| p.get("message")))
                    .and_then(serde_json::Value::as_str)
                    .map(|text| format!("done: {text}"))
                    .unwrap_or_else(|| "done".to_string()),
            ),
            "agent_message" | "respond" | "reply" => params
                .and_then(|p| {
                    p.get("message")
                        .or_else(|| p.get("text"))
                        .or_else(|| p.get("content"))
                })
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            _ => None,
        };
        return ParsedAssistant { display };
    }
    ParsedAssistant {
        display: Some(content.to_string()),
    }
}

/// Returns the textual representation of the last balanced top-level JSON object in `text`.
fn last_balanced_json_object(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut end: Option<usize> = None;
    let mut in_string = false;
    let mut escaped = false;
    for (idx, &byte) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'}' => end = Some(idx),
            _ => {}
        }
    }
    let end = end?;
    let mut depth: usize = 0;
    let mut in_string = false;
    let mut start: Option<usize> = None;
    for idx in (0..=end).rev() {
        let byte = bytes[idx];
        if in_string {
            if byte == b'"' && !is_escaped_quote(bytes, idx) {
                in_string = false;
            }
            continue;
        }
        if byte == b'"' && !is_escaped_quote(bytes, idx) {
            in_string = true;
            continue;
        }
        match byte {
            b'}' => depth += 1,
            b'{' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
                if depth == 0 {
                    start = Some(idx);
                    break;
                }
            }
            _ => {}
        }
    }
    let start = start?;
    Some(text[start..=end].to_string())
}

fn is_escaped_quote(bytes: &[u8], idx: usize) -> bool {
    let mut backslashes = 0usize;
    let mut cursor = idx;
    while cursor > 0 && bytes[cursor - 1] == b'\\' {
        backslashes += 1;
        cursor -= 1;
    }
    backslashes % 2 == 1
}

fn input_byte_index(input: &str, char_index: usize) -> usize {
    input
        .char_indices()
        .map(|(byte_index, _)| byte_index)
        .nth(char_index)
        .unwrap_or(input.len())
}

fn tool_output_preview(tool_name: &str, output: &serde_json::Value) -> Vec<String> {
    match tool_name {
        "shell_exec" | "shell_readonly" => shell_output_preview(output),
        "ripgrep_search" => ripgrep_output_preview(output),
        "file_write" | "file_patch" | "file_read" => file_output_preview(tool_name, output),
        _ => Vec::new(),
    }
}

fn tool_parameter_preview(tool_name: &str, parameters: &serde_json::Value) -> Vec<String> {
    match tool_name {
        "shell_exec" | "shell_readonly" => parameters
            .get("command")
            .and_then(serde_json::Value::as_str)
            .map(|command| vec![format!("  command: {command}")])
            .unwrap_or_default(),
        "ripgrep_search" => ripgrep_parameter_preview(parameters),
        "file_patch" => file_patch_parameter_preview(parameters),
        "file_write" => file_write_parameter_preview(parameters),
        _ => parameters
            .get("path")
            .and_then(serde_json::Value::as_str)
            .map(|path| vec![format!("  path: {path}")])
            .unwrap_or_default(),
    }
}

fn file_patch_parameter_preview(parameters: &serde_json::Value) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(path) = parameters.get("path").and_then(serde_json::Value::as_str) {
        lines.push(format!("  path: {path}"));
    }
    // The diff bodies themselves arrive as a dedicated `FileDiff` event
    // after the tool finishes (see `record_file_diff`), so the ToolStart
    // preview only carries the path. Anything else here would
    // double-render in the chat alongside the post-execution diff.
    lines
}

fn file_write_parameter_preview(parameters: &serde_json::Value) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(path) = parameters.get("path").and_then(serde_json::Value::as_str) {
        lines.push(format!("  path: {path}"));
    }
    if let Some(content) = parameters
        .get("content")
        .and_then(serde_json::Value::as_str)
    {
        lines.push("  content:".to_string());
        lines.extend(
            preview_lines(content, 4)
                .into_iter()
                .map(|line| format!("    {line}")),
        );
    }
    lines
}

fn ripgrep_parameter_preview(parameters: &serde_json::Value) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(query) = parameters.get("query").and_then(serde_json::Value::as_str) {
        lines.push(format!("  query: {query}"));
    }
    if let Some(path) = parameters.get("path").and_then(serde_json::Value::as_str) {
        lines.push(format!("  path: {path}"));
    }
    lines
}

fn shell_output_preview(output: &serde_json::Value) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(status) = output.get("status") {
        lines.push(format!("  status: {status}"));
    }
    if let Some(mutated) = output
        .get("workspace_mutated")
        .and_then(serde_json::Value::as_bool)
    {
        lines.push(format!("  mutated: {mutated}"));
    }
    for key in ["stdout", "stderr"] {
        let Some(text) = output.get(key).and_then(serde_json::Value::as_str) else {
            continue;
        };
        let preview = preview_lines(text, 3);
        if !preview.is_empty() {
            lines.push(format!("  {key}:"));
            lines.extend(preview.into_iter().map(|line| format!("    {line}")));
        }
    }
    lines
}

fn ripgrep_output_preview(output: &serde_json::Value) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(backend) = output.get("backend").and_then(serde_json::Value::as_str) {
        lines.push(format!("  backend: {backend}"));
    }
    if let Some(matches) = output.get("matches").and_then(serde_json::Value::as_array) {
        lines.push(format!("  matches: {}", matches.len()));
        for item in matches.iter().take(3) {
            let path = item
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("<unknown>");
            let line = item
                .get("line")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default();
            let text = item
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .trim();
            lines.push(format!("    {path}:{line}: {text}"));
        }
    }
    lines
}

fn file_output_preview(tool_name: &str, output: &serde_json::Value) -> Vec<String> {
    if tool_name == "file_read" {
        let Some(content) = output.as_str() else {
            return Vec::new();
        };
        let preview = preview_lines(content, 4);
        if preview.is_empty() {
            return Vec::new();
        }
        let mut lines = vec!["  preview:".to_string()];
        lines.extend(preview.into_iter().map(|line| format!("    {line}")));
        return lines;
    }
    output
        .get("path")
        .map(|path| vec![format!("  path: {path}")])
        .unwrap_or_default()
}

fn dedupe_sorted_nonempty(values: Vec<String>) -> Vec<String> {
    let mut values: Vec<String> = values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect();
    values.sort();
    values.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    values
}

fn preview_lines(text: &str, limit: usize) -> Vec<String> {
    let mut lines = text
        .lines()
        .take(limit)
        .map(|line| {
            if line.chars().count() > 120 {
                format!("{}...", line.chars().take(117).collect::<String>())
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>();
    if text.lines().count() > limit {
        lines.push("...".to_string());
    }
    lines
}

fn phase_display_label(phase: &str) -> &str {
    if phase.eq_ignore_ascii_case("verifying") {
        "checking"
    } else {
        phase
    }
}
