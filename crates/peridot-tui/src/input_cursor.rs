//! Multi-line input cursor navigation.
//!
//! Up/Down arrow handling for the multi-line composer: map the flat
//! character cursor into a (line, column) position, and move it to the
//! adjacent logical line while snapping the column to that line's length.
//! Split out of `input.rs` so the cursor arithmetic lives in one place; the
//! key handler calls [`try_move_cursor_up`] / [`try_move_cursor_down`] and
//! falls back to history navigation when they return `false`.

use crate::state::TuiState;

/// The (line, column) of the input cursor, both 0-based, derived from the flat
/// character offset `state.input_cursor` into `state.input`.
fn input_cursor_line_col(state: &TuiState) -> (usize, usize) {
    let prefix: String = state.input.chars().take(state.input_cursor).collect();
    let line = prefix.matches('\n').count();
    let col = prefix
        .rsplit('\n')
        .next()
        .map(|tail| tail.chars().count())
        .unwrap_or(0);
    (line, col)
}

/// Returns the character offset of the start of `target_line` (0-based)
/// inside `input`. `target_line` past the end clamps to the last
/// line so callers can dead-reckon a position even on overflow.
fn line_start_char_offset(input: &str, target_line: usize) -> usize {
    let mut count = 0usize;
    let mut offset = 0usize;
    for ch in input.chars() {
        if count == target_line {
            break;
        }
        offset += 1;
        if ch == '\n' {
            count += 1;
        }
    }
    offset
}

/// Tries to move the cursor to the previous logical line, snapping the
/// column to the line's length if it would overshoot. Returns `false`
/// when the cursor is already on line 0 — callers fall back to history
/// in that case.
pub(crate) fn try_move_cursor_up(state: &mut TuiState) -> bool {
    let (line, col) = input_cursor_line_col(state);
    if line == 0 {
        return false;
    }
    let lines: Vec<&str> = state.input.split('\n').collect();
    let target_line = line - 1;
    let target_col = col.min(lines[target_line].chars().count());
    state.input_cursor = line_start_char_offset(&state.input, target_line) + target_col;
    true
}

/// Mirror of [`try_move_cursor_up`] for the Down arrow.
pub(crate) fn try_move_cursor_down(state: &mut TuiState) -> bool {
    let (line, col) = input_cursor_line_col(state);
    let lines: Vec<&str> = state.input.split('\n').collect();
    if line + 1 >= lines.len() {
        return false;
    }
    let target_line = line + 1;
    let target_col = col.min(lines[target_line].chars().count());
    state.input_cursor = line_start_char_offset(&state.input, target_line) + target_col;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::HeaderState;
    use peridot_common::{ExecutionMode, PermissionMode};

    fn state_with_input(input: &str, cursor: usize) -> TuiState {
        let mut state = TuiState::new(HeaderState::new(
            ExecutionMode::Execute,
            PermissionMode::Auto,
            "mock",
        ));
        state.input = input.to_string();
        state.input_cursor = cursor;
        state
    }

    #[test]
    fn line_start_char_offset_clamps_past_the_end() {
        // "ab\ncd" → line 0 starts at 0, line 1 starts at 3; past-end clamps.
        assert_eq!(line_start_char_offset("ab\ncd", 0), 0);
        assert_eq!(line_start_char_offset("ab\ncd", 1), 3);
        assert_eq!(line_start_char_offset("ab\ncd", 9), 5);
    }

    #[test]
    fn move_up_keeps_column_and_reports_edges() {
        // Cursor on 'd' (line 1, col 1) in "ab\ncd".
        let mut state = state_with_input("ab\ncd", 4);
        assert!(try_move_cursor_up(&mut state));
        // Moves to line 0, col 1 → offset 1.
        assert_eq!(state.input_cursor, 1);
        // Already on line 0 → no move, returns false (caller falls back).
        assert!(!try_move_cursor_up(&mut state));
        assert_eq!(state.input_cursor, 1);
    }

    #[test]
    fn move_down_snaps_column_to_shorter_line() {
        // Cursor at end of "abcd" (line 0, col 4); next line "xy" is shorter.
        let mut state = state_with_input("abcd\nxy", 4);
        assert!(try_move_cursor_down(&mut state));
        // line 1 starts at offset 5, col snaps to 2 → 7.
        assert_eq!(state.input_cursor, 7);
        // Already on last line → false.
        assert!(!try_move_cursor_down(&mut state));
    }
}
