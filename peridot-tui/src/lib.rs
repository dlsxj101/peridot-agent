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
    layout::{Constraint, Direction, Layout, Position},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use serde::{Deserialize, Serialize};

mod ask_user;
mod input;
mod render;
mod state;
mod terminal;
#[cfg(test)]
mod tests;

pub use ask_user::{ApprovalDecision, ApprovalPanel, AskUserPanel, MenuState};
pub use input::{handle_key_event, run_interactive, run_interactive_with_events};
use render::goal_status_label;
pub use render::{draw, render_text_snapshot, select_layout};
pub use state::{
    ActivityKind, AgentRunStatus, HeaderState, LayoutMode, PlanStep, RuntimeActivity, SessionStats,
    SidePanelState, StreamState, SubagentMonitorItem, TuiEventOutcome, TuiExit, TuiLifecycleEvent,
    TuiRuntimeEvent, TuiState,
};
use terminal::TerminalGuard;
