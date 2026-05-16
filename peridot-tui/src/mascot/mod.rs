//! Peridot deer pixel mascot — 8×8 sprite rendered with Unicode half-blocks.
//!
//! The mascot lives in the upper four rows × eight columns of the side panel.
//! It picks a [`MascotState`] from the TUI state machine, looks up its frames
//! in [`frames::frames_for`], and draws the active frame each tick using the
//! `▀` glyph: each terminal cell encodes two stacked pixels (top via `fg`,
//! bottom via `bg`). Frame cycling is driven by `state.spinner_tick` so the
//! existing 10 Hz tick budget is reused.

mod frames;
mod render;

use crate::state::{AgentRunStatus, TuiState};

pub use frames::{MascotFrame, Pixel, frames_for, peridot_palette};
pub use render::{mascot_text_summary, render_mascot};

/// Mood the mascot should display, derived from the TUI state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum MascotState {
    /// Default idle pose; blinks every few seconds.
    Idle,
    /// Agent is streaming (thinking) — slight ear twitch.
    Thinking,
    /// One or more tools are running — antler gem glows.
    ToolRunning,
    /// Approval panel is open — head tilted with eyes wide.
    ApprovalWaiting,
    /// AskUser panel is open — curious upright posture.
    AskUser,
    /// Last run finished successfully — happy bounce.
    Done,
    /// Last run failed — ears drooping.
    Failed,
    /// Last run was interrupted — startled, ears straight up.
    Interrupted,
}

/// Picks the right mood from the TUI state.
pub fn mascot_state_from(state: &TuiState) -> MascotState {
    if state.agent_run_status == AgentRunStatus::Interrupted {
        return MascotState::Interrupted;
    }
    if state.ask_user.is_some() {
        return MascotState::AskUser;
    }
    if state.approval.is_some() || state.agent_run_status == AgentRunStatus::WaitingApproval {
        return MascotState::ApprovalWaiting;
    }
    if !state.active_tools.is_empty() {
        return MascotState::ToolRunning;
    }
    if state.active_stream.is_some() || state.agent_run_status == AgentRunStatus::Running {
        return MascotState::Thinking;
    }
    match state.agent_run_status {
        AgentRunStatus::Succeeded => MascotState::Done,
        AgentRunStatus::Failed => MascotState::Failed,
        _ => MascotState::Idle,
    }
}

/// Returns the active frame index for `state`, derived from `spinner_tick`.
/// Each frame holds for 5 ticks (≈500 ms at 10 Hz) so animations stay readable.
pub fn current_frame_index(spinner_tick: u32, frame_count: usize) -> usize {
    if frame_count <= 1 {
        return 0;
    }
    ((spinner_tick / 5) as usize) % frame_count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::HeaderState;
    use peridot_common::{ExecutionMode, PermissionMode};

    fn fixture() -> TuiState {
        TuiState::new(HeaderState::new(
            ExecutionMode::Execute,
            PermissionMode::Auto,
            "mock",
        ))
    }

    #[test]
    fn idle_is_default_mood() {
        let state = fixture();
        assert_eq!(mascot_state_from(&state), MascotState::Idle);
    }

    #[test]
    fn interrupted_status_wins_over_other_panels() {
        let mut state = fixture();
        state.agent_run_status = AgentRunStatus::Interrupted;
        assert_eq!(mascot_state_from(&state), MascotState::Interrupted);
    }

    #[test]
    fn frame_index_cycles_with_spinner_tick() {
        assert_eq!(current_frame_index(0, 3), 0);
        assert_eq!(current_frame_index(5, 3), 1);
        assert_eq!(current_frame_index(10, 3), 2);
        assert_eq!(current_frame_index(15, 3), 0);
        // Single-frame animations always return 0.
        assert_eq!(current_frame_index(99, 1), 0);
    }
}
