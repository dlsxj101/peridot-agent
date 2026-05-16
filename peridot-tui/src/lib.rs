//! Terminal UI state and rendering boundary.

use std::fmt::Write as FmtWrite;
use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
        size as terminal_size,
    },
};
use peridot_common::{AskUserRequest, ExecutionMode, PermissionMode, TuiConfig};
use peridot_core::{GoalStatus, SlashCommand, parse_slash_command};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use serde::{Deserialize, Serialize};

mod ask_user;
mod i18n;
mod input;
mod mascot;
mod render;
mod session_directory;
mod slash_picker;
mod state;
mod terminal;
#[cfg(test)]
mod tests;

pub use i18n::{PhraseKey, tr};
pub use mascot::{
    MascotFrame, MascotState, Pixel, mascot_state_from, peridot_palette, render_mascot,
};
pub use session_directory::{
    SessionDirectoryItem, cycle_foreground, foreground_index, render_tab_bar, render_tab_bar_text,
    tab_bar_height, trim_directory,
};
pub use slash_picker::{SlashCommandSpec, filtered_specs, first_match, slash_command_catalog};

pub use ask_user::{ApprovalDecision, ApprovalPanel, ApprovalScope, AskUserPanel, MenuState};
pub use input::{handle_key_event, run_interactive, run_interactive_with_events};
use render::goal_status_label;
pub use render::{draw, render_text_snapshot, select_layout};
pub use state::{
    ActivityKind, AgentRunStatus, AgentsSummary, BudgetGauge, HeaderState, LayoutMode,
    McpServerSummary, PlanStep, PlanStepUpdate, RuntimeActivity, SessionStats, SidePanelState,
    SlashPicker, StreamState, SubagentMonitorItem, TranscriptEntry, TranscriptKind,
    TuiEventOutcome, TuiExit, TuiLifecycleEvent, TuiRuntimeEvent, TuiState,
};
use terminal::TerminalGuard;
