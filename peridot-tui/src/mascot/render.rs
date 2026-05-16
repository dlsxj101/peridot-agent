//! Half-block rendering for the Peridot deer mascot.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use super::frames::{Pixel, palette_color};
use super::{MascotState, current_frame_index, frames_for, mascot_state_from};
use crate::state::TuiState;

/// Draws the mascot for the current TUI mood at `area`. The sprite is 8 cells
/// wide × 4 cells tall (each cell holds two stacked pixels via the `▀` glyph).
///
/// When `area` is too small the call is a no-op, so this is safe to call from
/// any layout slot.
pub fn render_mascot(state: &TuiState, area: Rect, buffer: &mut Buffer) {
    if area.width < 8 || area.height < 4 {
        return;
    }
    let mood = mascot_state_from(state);
    let frames = frames_for(mood);
    let frame = &frames[current_frame_index(state.spinner_tick, frames.len())];
    let symbol = "\u{2580}";

    for cell_y in 0..4 {
        for cell_x in 0..8 {
            let top = frame.pixels[(cell_y * 2) as usize][cell_x as usize];
            let bottom = frame.pixels[(cell_y * 2 + 1) as usize][cell_x as usize];
            if let Some(cell) = buffer.cell_mut((area.x + cell_x, area.y + cell_y)) {
                cell.set_symbol(symbol);
                cell.set_fg(pixel_color(top));
                cell.set_bg(pixel_color(bottom));
            }
        }
    }
}

fn pixel_color(pixel: Pixel) -> Color {
    match pixel {
        Pixel::Empty => Color::Reset,
        Pixel::Index(index) => palette_color(index),
    }
}

/// Returns a one-line ASCII summary of the active mood — used by the text
/// snapshot path so headless previews stay deterministic when truecolor isn't
/// available.
pub fn mascot_text_summary(state: &TuiState) -> String {
    let mood = mascot_state_from(state);
    let glyph = match mood {
        MascotState::Idle => "\u{1F98C}",
        MascotState::Thinking => "?",
        MascotState::ToolRunning => "\u{2699}",
        MascotState::ApprovalWaiting => "!",
        MascotState::AskUser => "?",
        MascotState::Done => "\u{2713}",
        MascotState::Failed => "x",
        MascotState::Interrupted => "*",
    };
    format!("{glyph} {mood:?}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{AgentRunStatus, HeaderState};
    use peridot_common::{ExecutionMode, PermissionMode};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    fn fixture() -> TuiState {
        TuiState::new(HeaderState::new(
            ExecutionMode::Execute,
            PermissionMode::Auto,
            "mock",
        ))
    }

    #[test]
    fn render_writes_halfblock_glyphs_in_8x4_window() {
        let state = fixture();
        let area = Rect::new(0, 0, 16, 8);
        let mut buffer = Buffer::empty(area);
        render_mascot(&state, Rect::new(0, 0, 8, 4), &mut buffer);
        let mut filled = 0;
        for y in 0..4 {
            for x in 0..8 {
                if let Some(cell) = buffer.cell((x, y))
                    && cell.symbol() == "\u{2580}"
                {
                    filled += 1;
                }
            }
        }
        assert_eq!(
            filled, 32,
            "all 8×4 cells should carry the half-block glyph"
        );
    }

    #[test]
    fn render_is_no_op_when_area_too_small() {
        let state = fixture();
        let area = Rect::new(0, 0, 4, 2);
        let mut buffer = Buffer::empty(area);
        render_mascot(&state, area, &mut buffer);
        // Buffer starts empty (no symbol); after no-op render, still empty.
        let cell = buffer.cell((0, 0)).unwrap();
        assert_eq!(cell.symbol(), " ");
    }

    #[test]
    fn text_summary_reflects_interrupted_state() {
        let mut state = fixture();
        state.agent_run_status = AgentRunStatus::Interrupted;
        let text = mascot_text_summary(&state);
        assert!(text.contains("Interrupted"));
    }
}
