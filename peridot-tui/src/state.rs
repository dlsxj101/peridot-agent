use super::*;

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

/// Right-side panel state.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SidePanelState {
    /// Current plan steps.
    pub plan: Vec<PlanStep>,
    /// Session statistics.
    pub stats: SessionStats,
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
    /// Tool execution is waiting on explicit user approval.
    ApprovalRequested {
        /// Tool name.
        tool_name: String,
        /// Reason the tool is gated.
        reason: String,
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
    /// Background run finished.
    Finished {
        /// Stop reason.
        stop_reason: String,
        /// Number of turns.
        turns: usize,
        /// Whether the stop reason represents successful completion.
        success: bool,
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
}

/// One transcript line plus its style classification.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TranscriptEntry {
    /// Visual category.
    pub kind: TranscriptKind,
    /// Plain-text payload (no styling).
    pub text: String,
}

impl TranscriptEntry {
    /// Creates a transcript entry of the given kind.
    pub fn new(kind: TranscriptKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
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
    /// Active Esc menu.
    pub menu: Option<MenuState>,
    /// Lifecycle events recorded from local TUI commands.
    pub lifecycle_events: Vec<TuiLifecycleEvent>,
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
        /// Tool name.
        tool_name: String,
        /// Approval reason.
        reason: String,
    },
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
            menu: None,
            lifecycle_events: Vec::new(),
        }
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

    /// Appends a transcript line of the given kind.
    pub fn push_transcript_entry(&mut self, kind: TranscriptKind, line: impl Into<String>) {
        self.transcript.push(TranscriptEntry::new(kind, line));
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

    /// Advances the spinner animation by one frame.
    pub fn tick_spinner(&mut self) {
        self.spinner_tick = self.spinner_tick.wrapping_add(1);
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
        if input.trim().is_empty() {
            return;
        }
        if self.input_history.last().is_none_or(|last| last != input) {
            self.input_history.push(input.to_string());
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
    }

    /// Clears the input buffer and cursor.
    pub fn clear_input(&mut self) {
        self.input.clear();
        self.input_cursor = 0;
        self.input_history_cursor = None;
    }

    /// Inserts one character at the current input cursor.
    pub fn insert_input_char(&mut self, character: char) {
        let byte_index = input_byte_index(&self.input, self.input_cursor);
        self.input.insert(byte_index, character);
        self.input_cursor += 1;
        self.input_history_cursor = None;
    }

    /// Removes the character before the current input cursor.
    pub fn backspace_input(&mut self) {
        if self.input_cursor == 0 {
            return;
        }
        let start = input_byte_index(&self.input, self.input_cursor - 1);
        let end = input_byte_index(&self.input, self.input_cursor);
        self.input.replace_range(start..end, "");
        self.input_cursor -= 1;
        self.input_history_cursor = None;
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
    }

    /// Moves the input cursor one character left.
    pub fn move_input_cursor_left(&mut self) {
        self.input_cursor = self.input_cursor.saturating_sub(1);
    }

    /// Moves the input cursor one character right.
    pub fn move_input_cursor_right(&mut self) {
        self.input_cursor = (self.input_cursor + 1).min(self.input.chars().count());
    }

    /// Moves the input cursor to the start.
    pub fn move_input_cursor_home(&mut self) {
        self.input_cursor = 0;
    }

    /// Moves the input cursor to the end.
    pub fn move_input_cursor_end(&mut self) {
        self.input_cursor = self.input.chars().count();
    }

    /// Marks an agent task as running.
    pub fn mark_agent_running(&mut self, task: impl Into<String>) {
        let task = task.into();
        self.agent_run_status = AgentRunStatus::Running;
        self.last_task = Some(task.clone());
        self.begin_stream("assistant");
        self.push_activity(ActivityKind::Stream, "run", format!("running: {task}"));
        self.push_transcript_entry(TranscriptKind::System, format!("task: {task}"));
    }

    /// Marks the active agent task as completed.
    pub fn mark_agent_succeeded(&mut self, summary: impl Into<String>) {
        self.agent_run_status = AgentRunStatus::Succeeded;
        self.active_tools.clear();
        self.push_activity(ActivityKind::Stream, "run", "done");
        self.push_transcript_entry(TranscriptKind::System, format!("run: {}", summary.into()));
    }

    /// Marks the active agent task as failed.
    pub fn mark_agent_failed(&mut self, message: impl Into<String>) {
        self.agent_run_status = AgentRunStatus::Failed;
        self.active_tools.clear();
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
        let kind = if success {
            TranscriptKind::ToolOk
        } else {
            TranscriptKind::ToolFail
        };
        self.push_transcript_entry(kind, format!("{tool_name}  {summary}"));
        for line in tool_output_preview(&tool_name, &output) {
            self.push_transcript_entry(kind, line);
        }
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

    /// Opens a tool approval panel.
    pub fn open_approval(&mut self, tool_name: impl Into<String>, reason: impl Into<String>) {
        let tool_name = tool_name.into();
        let reason = reason.into();
        self.agent_run_status = AgentRunStatus::WaitingApproval;
        self.push_activity(
            ActivityKind::Tool,
            tool_name.clone(),
            format!("approval required: {reason}"),
        );
        self.approval = Some(ApprovalPanel::new(tool_name, reason));
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
                self.push_activity(ActivityKind::Stream, "turn", format!("#{}", turn_index + 1));
            }
            TuiRuntimeEvent::AssistantStarted { label } => self.begin_stream(label),
            TuiRuntimeEvent::AssistantDelta { delta } => self.push_stream_delta(&delta),
            TuiRuntimeEvent::AssistantFinished => self.finish_stream(),
            TuiRuntimeEvent::Thinking { text } => {
                self.push_activity(ActivityKind::Stream, "thinking", "parsed");
                if self.config.show_thinking || self.debug_view {
                    self.push_transcript_entry(TranscriptKind::Debug, format!("thinking: {text}"));
                }
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
            TuiRuntimeEvent::ApprovalRequested { tool_name, reason } => {
                self.open_approval(tool_name, reason);
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
            TuiRuntimeEvent::Finished {
                stop_reason,
                turns,
                success,
            } => {
                if self.approval.is_some() {
                    self.push_transcript_entry(
                        TranscriptKind::System,
                        format!("run: stopped={stop_reason} turns={turns}"),
                    );
                    self.agent_run_status = AgentRunStatus::WaitingApproval;
                    return;
                }
                if success {
                    self.mark_agent_succeeded(format!("stopped={stop_reason} turns={turns}"));
                } else {
                    self.mark_agent_failed(format!("stopped={stop_reason} turns={turns}"));
                }
            }
            TuiRuntimeEvent::SessionSaved { session_id } => {
                self.push_activity(
                    ActivityKind::Stream,
                    "session",
                    format!("saved: {session_id}"),
                );
                self.push_transcript_entry(
                    TranscriptKind::Notice,
                    format!("session: saved {session_id}"),
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
        "shell_exec" => shell_output_preview(output),
        "file_write" | "file_patch" | "file_read" => file_output_preview(tool_name, output),
        _ => Vec::new(),
    }
}

fn tool_parameter_preview(tool_name: &str, parameters: &serde_json::Value) -> Vec<String> {
    match tool_name {
        "shell_exec" => parameters
            .get("command")
            .and_then(serde_json::Value::as_str)
            .map(|command| vec![format!("  command: {command}")])
            .unwrap_or_default(),
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
    if let Some(old_text) = parameters
        .get("old_text")
        .and_then(serde_json::Value::as_str)
    {
        lines.push("  diff:".to_string());
        lines.extend(
            preview_lines(old_text, 4)
                .into_iter()
                .map(|line| format!("    - {line}")),
        );
    }
    if let Some(new_text) = parameters
        .get("new_text")
        .and_then(serde_json::Value::as_str)
    {
        if !lines.iter().any(|line| line == "  diff:") {
            lines.push("  diff:".to_string());
        }
        lines.extend(
            preview_lines(new_text, 4)
                .into_iter()
                .map(|line| format!("    + {line}")),
        );
    }
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

fn shell_output_preview(output: &serde_json::Value) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(status) = output.get("status") {
        lines.push(format!("  status: {status}"));
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
