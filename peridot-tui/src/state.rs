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
    pub transcript: Vec<String>,
    /// Active streaming model output.
    pub active_stream: Option<StreamState>,
    /// Recent tool, stream, and verification activity.
    pub activities: Vec<RuntimeActivity>,
    /// Recent delegated subagents.
    pub subagents: Vec<SubagentMonitorItem>,
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

    /// Appends a transcript line.
    pub fn push_transcript(&mut self, line: impl Into<String>) {
        self.transcript.push(line.into());
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
        self.push_transcript(format!("task: {task}"));
    }

    /// Marks the active agent task as completed.
    pub fn mark_agent_succeeded(&mut self, summary: impl Into<String>) {
        self.agent_run_status = AgentRunStatus::Succeeded;
        self.push_activity(ActivityKind::Stream, "run", "done");
        self.push_transcript(format!("run: {}", summary.into()));
    }

    /// Marks the active agent task as failed.
    pub fn mark_agent_failed(&mut self, message: impl Into<String>) {
        self.agent_run_status = AgentRunStatus::Failed;
        self.side_panel.stats.errors += 1;
        self.push_activity(ActivityKind::Stream, "run", "failed");
        self.push_transcript(format!("run failed: {}", message.into()));
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
            self.push_transcript(format!("{}: <empty>", stream.label));
        } else {
            self.push_transcript(format!("{}: {content}", stream.label));
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
        self.push_activity(ActivityKind::Tool, tool_name.clone(), "running");
        self.push_transcript(format!("tool {tool_name}: running"));
        for line in tool_parameter_preview(&tool_name, &parameters) {
            self.push_transcript(line);
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
        self.record_tool_activity(tool_name.clone(), success, summary.clone());
        let marker = if success { "ok" } else { "failed" };
        self.push_transcript(format!("tool {tool_name}: {marker}: {summary}"));
        for line in tool_output_preview(&tool_name, &output) {
            self.push_transcript(line);
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
        self.push_transcript(format!(
            "approval: {} {label} ({})",
            panel.tool_name, panel.reason
        ));
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
                if self.config.show_thinking {
                    self.push_activity(ActivityKind::Stream, "thinking", "parsed");
                    self.push_transcript(format!("thinking: {text}"));
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
                    self.push_transcript(format!("run: stopped={stop_reason} turns={turns}"));
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
                self.push_transcript(format!("session: saved {session_id}"));
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
                self.push_transcript(format!("session: failed to save {session_id}: {message}"));
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
