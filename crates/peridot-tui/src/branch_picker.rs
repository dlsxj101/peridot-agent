//! Branch picker — list view for selecting a turn to fork from.
//!
//! The operator types `/branch` with no args and the harness pushes
//! `SessionCommandEvent::BranchPickerOpen`. The CLI handler loads
//! `context.bin`, walks the turn ids, and feeds the resulting list
//! back as `TuiRuntimeEvent::BranchPickerTurns`. The picker overlay
//! renders the list; on Enter the chosen turn id flows through the
//! same `/branch turn <id>` slash command, so all the picker does is
//! pick — the actual fork lives in the existing handler.

use serde::{Deserialize, Serialize};

/// One row in the branch picker — a single past turn the operator can
/// fork from.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BranchPickerTurn {
    /// Stable per-session turn id, stamped by the harness when the
    /// turn was first executed.
    pub turn_id: u64,
    /// Label describing who produced the turn ("user", "assistant",
    /// "tool", etc.) — drives the colour of the row.
    pub source: String,
    /// One-line preview of the turn's contents (already truncated to
    /// fit on a list row).
    pub preview: String,
}

/// Live state for the branch picker overlay. Mirrors the lifecycle
/// of [`crate::ApprovalPanel`]: created when the operator opens the
/// picker, populated asynchronously when the CLI hands back the turn
/// list, dropped when the operator commits or cancels.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct BranchPickerState {
    /// Turns the operator can fork from. Empty until the CLI sends
    /// `TuiRuntimeEvent::BranchPickerTurns`.
    pub turns: Vec<BranchPickerTurn>,
    /// Index of the highlighted row.
    pub selected: usize,
    /// `true` once the turn list has been populated. Used by the
    /// render path to distinguish "loading" from "no turns".
    pub loaded: bool,
}

impl BranchPickerState {
    /// Builds an empty picker in the "loading" state.
    pub fn opening() -> Self {
        Self::default()
    }

    /// Replaces the turn list with `turns` and marks the picker as
    /// loaded. Resets the selection to the latest turn so the
    /// operator can roll back one step with a single Enter — the
    /// most common case.
    pub fn populate(&mut self, mut turns: Vec<BranchPickerTurn>) {
        // Render newest at the bottom; selection starts on the last
        // entry so Enter forks at the most recent turn.
        turns.sort_by_key(|t| t.turn_id);
        self.selected = turns.len().saturating_sub(1);
        self.turns = turns;
        self.loaded = true;
    }

    /// Moves the selection by `delta` rows, wrapping at the edges.
    /// No-op when the list is empty.
    pub fn move_selection(&mut self, delta: i32) {
        if self.turns.is_empty() {
            return;
        }
        let len = self.turns.len() as i32;
        let current = self.selected as i32;
        self.selected = ((current + delta).rem_euclid(len)) as usize;
    }

    /// Returns the highlighted turn id when one exists.
    pub fn selected_turn_id(&self) -> Option<u64> {
        self.turns.get(self.selected).map(|t| t.turn_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(id: u64, source: &str) -> BranchPickerTurn {
        BranchPickerTurn {
            turn_id: id,
            source: source.to_string(),
            preview: format!("turn-{id}"),
        }
    }

    #[test]
    fn populate_sorts_and_selects_latest() {
        let mut state = BranchPickerState::opening();
        state.populate(vec![turn(2, "user"), turn(5, "tool"), turn(1, "user")]);
        assert!(state.loaded);
        assert_eq!(
            state.turns.iter().map(|t| t.turn_id).collect::<Vec<_>>(),
            vec![1, 2, 5]
        );
        assert_eq!(state.selected, 2);
        assert_eq!(state.selected_turn_id(), Some(5));
    }

    #[test]
    fn move_selection_wraps_at_edges() {
        let mut state = BranchPickerState::opening();
        state.populate(vec![turn(1, "user"), turn(2, "user"), turn(3, "user")]);
        state.selected = 0;
        state.move_selection(-1);
        assert_eq!(state.selected, 2, "wrap from 0 to last");
        state.move_selection(1);
        assert_eq!(state.selected, 0, "wrap from last to 0");
    }

    #[test]
    fn empty_list_is_inert() {
        let mut state = BranchPickerState::opening();
        state.move_selection(1);
        assert_eq!(state.selected, 0);
        assert!(state.selected_turn_id().is_none());
    }
}
