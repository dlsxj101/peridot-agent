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

    for cell_y in 0..4 {
        for cell_x in 0..8 {
            let top = frame.pixels[(cell_y * 2) as usize][cell_x as usize];
            let bottom = frame.pixels[(cell_y * 2 + 1) as usize][cell_x as usize];
            if let Some(cell) = buffer.cell_mut((area.x + cell_x, area.y + cell_y)) {
                // Pick glyph + colors so transparent pixels never paint the
                // terminal default fg/bg onto the sprite. Drawing a `▀`
                // half-block with `Color::Reset` as fg makes empty top pixels
                // appear in the terminal's default foreground colour (light
                // grey or white on a dark theme) — which is what the operator
                // sees as a white "background" around the mascot.
                match (top, bottom) {
                    (Pixel::Empty, Pixel::Empty) => {
                        // Fully empty cell — emit a plain space so the
                        // surrounding terminal background shows through.
                        cell.set_symbol(" ");
                        cell.set_fg(Color::Reset);
                        cell.set_bg(Color::Reset);
                    }
                    (Pixel::Empty, Pixel::Index(index)) => {
                        // Only bottom pixel set — lower half block carries
                        // the colour, top half (fg) stays default.
                        cell.set_symbol("\u{2584}");
                        cell.set_fg(palette_color(index));
                        cell.set_bg(Color::Reset);
                    }
                    (Pixel::Index(index), Pixel::Empty) => {
                        // Only top pixel set — upper half block.
                        cell.set_symbol("\u{2580}");
                        cell.set_fg(palette_color(index));
                        cell.set_bg(Color::Reset);
                    }
                    (Pixel::Index(top_idx), Pixel::Index(bottom_idx)) => {
                        cell.set_symbol("\u{2580}");
                        cell.set_fg(palette_color(top_idx));
                        cell.set_bg(palette_color(bottom_idx));
                    }
                }
            }
        }
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
    fn render_paints_sprite_pixels_and_leaves_empty_cells_transparent() {
        let state = fixture();
        let area = Rect::new(0, 0, 16, 8);
        let mut buffer = Buffer::empty(area);
        render_mascot(&state, Rect::new(0, 0, 8, 4), &mut buffer);

        let mut upper = 0;
        let mut lower = 0;
        let mut spaces = 0;
        for y in 0..4 {
            for x in 0..8 {
                let cell = buffer.cell((x, y)).unwrap();
                match cell.symbol() {
                    "\u{2580}" => upper += 1,
                    "\u{2584}" => lower += 1,
                    " " => spaces += 1,
                    other => panic!("unexpected glyph at ({x},{y}): {other:?}"),
                }
            }
        }
        // BASE frame has fully-empty columns 0 and 7 plus an empty row
        // beneath the legs — empty cells must NOT render the half-block
        // glyph or they paint the terminal default fg as a white block.
        assert!(
            spaces > 0,
            "expected at least one transparent cell, got 0 (white-bg regression)"
        );
        assert!(upper > 0, "expected upper-half cells for sprite body");
        assert!(lower > 0, "expected lower-half cells for top-empty pixels");
        assert_eq!(upper + lower + spaces, 32, "every cell must be classified");
    }

    #[test]
    fn empty_cells_use_reset_colors_not_terminal_default_fg() {
        // The regression we're guarding: when a cell is fully transparent
        // we must clear fg/bg to `Color::Reset`, otherwise the terminal
        // inherits an SGR from a neighbouring sprite cell and paints the
        // mascot's background in the wrong colour (operator-visible as a
        // white halo around the deer on dark terminals).
        let state = fixture();
        let area = Rect::new(0, 0, 8, 4);
        let mut buffer = Buffer::empty(area);
        render_mascot(&state, area, &mut buffer);
        let corner = buffer.cell((0, 0)).unwrap();
        assert_eq!(corner.symbol(), " ", "top-left should be transparent");
        assert_eq!(corner.fg, Color::Reset);
        assert_eq!(corner.bg, Color::Reset);
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
