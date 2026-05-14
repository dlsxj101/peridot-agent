//! Terminal UI state and rendering boundary.

use std::fmt::Write;

use peridot_common::{ExecutionMode, PermissionMode};
use peridot_core::{SlashCommand, parse_slash_command};
use serde::{Deserialize, Serialize};

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
            cost_usd: 0.0,
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

/// Main TUI state independent from the terminal backend.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TuiState {
    /// Current layout mode.
    pub layout: LayoutMode,
    /// Header state.
    pub header: HeaderState,
    /// Transcript lines.
    pub transcript: Vec<String>,
    /// Side panel state.
    pub side_panel: SidePanelState,
    /// Current input buffer.
    pub input: String,
}

impl TuiState {
    /// Creates a new TUI state.
    pub fn new(header: HeaderState) -> Self {
        Self {
            layout: LayoutMode::Full,
            header,
            transcript: Vec::new(),
            side_panel: SidePanelState::default(),
            input: String::new(),
        }
    }

    /// Selects a layout mode from terminal dimensions.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.layout = select_layout(width, height);
    }

    /// Appends a transcript line.
    pub fn push_transcript(&mut self, line: impl Into<String>) {
        self.transcript.push(line.into());
    }

    /// Parses the current input as a slash command when possible.
    pub fn current_slash_command(&self) -> Option<SlashCommand> {
        parse_slash_command(&self.input)
    }
}

/// Renders a deterministic text snapshot for tests and headless previews.
pub fn render_text_snapshot(state: &TuiState) -> String {
    let mut output = String::new();
    let _ = writeln!(
        output,
        "PERIDOT | {}.{} | {} | ${:.4}",
        state.header.mode, state.header.permission, state.header.model, state.header.cost_usd
    );
    let _ = writeln!(output, "layout: {:?}", state.layout);
    let _ = writeln!(output);
    for line in state.transcript.iter().rev().take(20).rev() {
        let _ = writeln!(output, "{line}");
    }
    if state.layout == LayoutMode::Full {
        let done = state
            .side_panel
            .plan
            .iter()
            .filter(|step| step.done)
            .count();
        let _ = writeln!(output);
        let _ = writeln!(output, "Plan {done}/{}", state.side_panel.plan.len());
        for step in &state.side_panel.plan {
            let marker = if step.done { "[x]" } else { "[ ]" };
            let _ = writeln!(output, "{marker} {}", step.label);
        }
        let _ = writeln!(
            output,
            "Session steps={} errors={} elapsed={}s",
            state.side_panel.stats.steps,
            state.side_panel.stats.errors,
            state.side_panel.stats.elapsed_seconds
        );
    }
    let _ = write!(output, "> {}", state.input);
    output
}

/// Selects a layout mode from terminal dimensions.
pub fn select_layout(width: u16, height: u16) -> LayoutMode {
    if width >= 120 && height >= 32 {
        LayoutMode::Full
    } else if width >= 80 && height >= 20 {
        LayoutMode::Compact
    } else {
        LayoutMode::Minimal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_layout_from_terminal_size() {
        assert_eq!(select_layout(140, 40), LayoutMode::Full);
        assert_eq!(select_layout(90, 24), LayoutMode::Compact);
        assert_eq!(select_layout(60, 12), LayoutMode::Minimal);
    }

    #[test]
    fn parses_input_slash_command() {
        let mut state = TuiState::new(HeaderState::new(
            ExecutionMode::Execute,
            PermissionMode::Auto,
            "mock",
        ));
        state.input = "/goal fix tests".to_string();

        assert_eq!(
            state.current_slash_command(),
            Some(SlashCommand::GoalStart("fix tests".to_string()))
        );
    }

    #[test]
    fn renders_text_snapshot() {
        let mut state = TuiState::new(HeaderState::new(
            ExecutionMode::Execute,
            PermissionMode::Auto,
            "mock",
        ));
        state.push_transcript("tool file_write ok");
        state.side_panel.plan.push(PlanStep {
            label: "Implement hooks".to_string(),
            done: true,
        });

        let snapshot = render_text_snapshot(&state);

        assert!(snapshot.contains("PERIDOT | execute.auto | mock"));
        assert!(snapshot.contains("[x] Implement hooks"));
        assert!(snapshot.contains("tool file_write ok"));
    }
}
