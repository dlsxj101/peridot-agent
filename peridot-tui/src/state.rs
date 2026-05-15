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
    /// Side panel state.
    pub side_panel: SidePanelState,
    /// Current goal lifecycle status, when a goal is active.
    pub goal_status: Option<GoalStatus>,
    /// Current input buffer.
    pub input: String,
    /// Active ask-user panel, when the agent is waiting for user guidance.
    pub ask_user: Option<AskUserPanel>,
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
            side_panel: SidePanelState::default(),
            goal_status: None,
            input: String::new(),
            ask_user: None,
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
