//! Terminal UI state and rendering boundary.

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
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HeaderState {
    /// Active execution mode.
    pub mode: ExecutionMode,
    /// Active permission mode.
    pub permission: PermissionMode,
    /// Active model name.
    pub model: String,
}

impl HeaderState {
    /// Creates a new header state.
    pub fn new(mode: ExecutionMode, permission: PermissionMode, model: impl Into<String>) -> Self {
        Self {
            mode,
            permission,
            model: model.into(),
        }
    }
}

/// Main TUI state independent from the terminal backend.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TuiState {
    /// Current layout mode.
    pub layout: LayoutMode,
    /// Header state.
    pub header: HeaderState,
    /// Transcript lines.
    pub transcript: Vec<String>,
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
}
