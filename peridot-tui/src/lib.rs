//! Terminal UI state and rendering boundary.
//
// Clippy's `doc_lazy_continuation` lint currently ICEs when a doc comment
// contains certain multi-byte characters (em-dashes, CJK glyphs) — a
// reported upstream bug. We disable the lint at the crate level until the
// fix lands so the workspace clippy gate stays usable.
#![allow(clippy::doc_lazy_continuation)]
// Clippy 0.1.95's `needless_borrows_for_generic_args` lint ICEs while
// rendering a diagnostic against expressions in this crate ("slice index
// starts at 45 but ends at 44"). Disable until upstream clippy ships a fix.
#![allow(clippy::needless_borrows_for_generic_args)]

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
mod at_picker;
mod branch_picker;
mod diff_hunks;
#[cfg(test)]
mod fixtures;
mod i18n;
mod input;
mod mascot;
mod render;
mod session_directory;
mod settings_screen;
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
pub use branch_picker::{BranchPickerState, BranchPickerTurn};
pub use input::{handle_key_event, run_interactive, run_interactive_with_events};
use render::goal_status_label;
pub use render::{draw, render_text_snapshot, select_layout};
pub use settings_screen::{SettingItem, SettingValue, SettingsOutcome, run_settings_screen};
pub use state::{
    ActivityKind, AgentRunStatus, AgentsSummary, BudgetGauge, HeaderState, LayoutMode,
    McpServerSummary, PlanStep, PlanStepUpdate, RuntimeActivity, SessionCommandEvent, SessionStats,
    SidePanelState, SlashPicker, StreamState, SubagentMonitorItem, TranscriptEntry, TranscriptKind,
    TuiEventOutcome, TuiExit, TuiLifecycleEvent, TuiRuntimeEvent, TuiState,
};
use terminal::TerminalGuard;
