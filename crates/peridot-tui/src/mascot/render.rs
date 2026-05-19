//! Quadrant-block rendering for the 16×16 Peridot deer mascot.
//!
//! Each 2×2 block of sprite pixels maps to one terminal cell: the
//! renderer picks a Unicode quadrant-block glyph that fills the
//! "filled" sub-pixels with a foreground colour and leaves the
//! "empty" sub-pixels in a background colour. So the sprite is
//! drawn at the same 8 cols × 4 rows footprint as the old half-
//! block 8×8 deer while carrying 4× the pixel detail.
//!
//! Two-colour-per-cell rule: every quadrant in the source frames is
//! designed with at most two distinct colours (where `Pixel::Empty`
//! counts as transparent). If a frame breaks the rule, the renderer
//! picks the two most common colours and snaps the rest to the
//! nearest of them — no crash, just a slightly less faithful pixel.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use super::frames::{Pixel, palette_color};
use super::{MascotState, current_frame_index, frames_for, mascot_state_from};
use crate::state::TuiState;

/// Width / height of the rendered mascot in terminal cells.
const CELL_COLS: u16 = 8;
const CELL_ROWS: u16 = 4;

/// Draws the mascot for the current TUI mood at `area`. Uses 8 cells
/// wide × 4 cells tall (each cell encodes a 2×2 block of sprite
/// pixels). When `area` is too small the call is a no-op.
pub fn render_mascot(state: &TuiState, area: Rect, buffer: &mut Buffer) {
    if area.width < CELL_COLS || area.height < CELL_ROWS {
        return;
    }
    let mood = mascot_state_from(state);
    let frames = frames_for(mood);
    let frame = &frames[current_frame_index(state.spinner_tick, frames.len())];

    for cell_y in 0..CELL_ROWS {
        for cell_x in 0..CELL_COLS {
            // Pull the 2×2 sub-pixels that make up this cell. Top-left,
            // top-right, bottom-left, bottom-right ordering matches the
            // bit positions used by `quadrant_glyph`.
            let px = (cell_x as usize) * 2;
            let py = (cell_y as usize) * 2;
            let tl = frame.pixels[py][px];
            let tr = frame.pixels[py][px + 1];
            let bl = frame.pixels[py + 1][px];
            let br = frame.pixels[py + 1][px + 1];

            let (glyph, fg, bg) = quadrant_cell(tl, tr, bl, br);
            if let Some(cell) = buffer.cell_mut((area.x + cell_x, area.y + cell_y)) {
                cell.set_symbol(glyph);
                cell.set_fg(fg);
                cell.set_bg(bg);
            }
        }
    }
}

/// Compresses a 2×2 sub-pixel block into one terminal cell. Returns
/// the glyph plus the foreground / background colours the renderer
/// should draw with. `Color::Reset` is used for transparent slots so
/// the terminal's own background bleeds through.
fn quadrant_cell(tl: Pixel, tr: Pixel, bl: Pixel, br: Pixel) -> (&'static str, Color, Color) {
    // Fast path 1: all four sub-pixels are `Empty` — a fully
    // transparent cell. Emit a space so neighbouring cells' bg
    // doesn't leak into ours.
    if matches!(
        (tl, tr, bl, br),
        (Pixel::Empty, Pixel::Empty, Pixel::Empty, Pixel::Empty)
    ) {
        return (" ", Color::Reset, Color::Reset);
    }

    // Collect the up-to-two opaque colours present in this cell.
    // The `(fg, bg)` choice then maps every sub-pixel into a
    // 4-bit mask: 1 = sub-pixel matches `fg`, 0 = matches `bg`
    // (which may be `Color::Reset` when an `Empty` is in the mix).
    let pixels = [tl, tr, bl, br];
    let mut palette: Vec<u8> = Vec::with_capacity(4);
    let mut has_empty = false;
    for px in &pixels {
        match px {
            Pixel::Empty => has_empty = true,
            Pixel::Index(i) => {
                if !palette.contains(i) {
                    palette.push(*i);
                }
            }
        }
    }
    // Choose fg / bg. Designs respecting the two-colour-per-cell
    // rule end up here with `palette.len() <= 2`. If a frame breaks
    // the rule we keep the first two indices and approximate.
    let (fg_color, bg_color) = match (palette.as_slice(), has_empty) {
        ([], _) => return (" ", Color::Reset, Color::Reset),
        ([only], true) => (palette_color(*only), Color::Reset),
        ([only], false) => {
            // Solid block — every sub-pixel is the same colour. The
            // glyph is irrelevant; pick `█` for crispness.
            return ("\u{2588}", palette_color(*only), Color::Reset);
        }
        ([fg, bg], _) => (palette_color(*fg), palette_color(*bg)),
        ([fg, bg, ..], _) => (palette_color(*fg), palette_color(*bg)),
    };

    // Build the 4-bit mask: bit 3 = TL, bit 2 = TR, bit 1 = BL,
    // bit 0 = BR — set when the sub-pixel matches `fg_color`,
    // cleared when it matches `bg_color` or is `Empty`.
    let primary_index = palette.first().copied();
    let bit = |px: Pixel| -> u8 {
        match (px, primary_index) {
            (Pixel::Index(i), Some(p)) if i == p => 1,
            _ => 0,
        }
    };
    let mask = (bit(tl) << 3) | (bit(tr) << 2) | (bit(bl) << 1) | bit(br);
    let glyph = quadrant_glyph(mask);
    (glyph, fg_color, bg_color)
}

/// Maps a 4-bit TL/TR/BL/BR mask to its Unicode quadrant glyph.
fn quadrant_glyph(mask: u8) -> &'static str {
    match mask & 0b1111 {
        0b0000 => " ",
        0b0001 => "\u{2597}", // ▗ BR
        0b0010 => "\u{2596}", // ▖ BL
        0b0011 => "\u{2584}", // ▄ BL+BR
        0b0100 => "\u{259D}", // ▝ TR
        0b0101 => "\u{2590}", // ▐ TR+BR
        0b0110 => "\u{259E}", // ▞ TR+BL
        0b0111 => "\u{259F}", // ▟ TR+BL+BR
        0b1000 => "\u{2598}", // ▘ TL
        0b1001 => "\u{259A}", // ▚ TL+BR
        0b1010 => "\u{258C}", // ▌ TL+BL
        0b1011 => "\u{2599}", // ▙ TL+BL+BR
        0b1100 => "\u{2580}", // ▀ TL+TR
        0b1101 => "\u{259C}", // ▜ TL+TR+BR
        0b1110 => "\u{259B}", // ▛ TL+TR+BL
        0b1111 => "\u{2588}", // █ full
        _ => " ",
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
    fn render_fills_8x4_cells_without_panicking() {
        let state = fixture();
        let area = Rect::new(0, 0, CELL_COLS, CELL_ROWS);
        let mut buffer = Buffer::empty(area);
        render_mascot(&state, area, &mut buffer);
        // Every cell must have been touched — at minimum a space.
        for y in 0..CELL_ROWS {
            for x in 0..CELL_COLS {
                let cell = buffer.cell((x, y)).unwrap();
                assert!(!cell.symbol().is_empty());
            }
        }
    }

    #[test]
    fn fully_transparent_cell_uses_reset_colors() {
        let (glyph, fg, bg) = quadrant_cell(Pixel::Empty, Pixel::Empty, Pixel::Empty, Pixel::Empty);
        assert_eq!(glyph, " ");
        assert_eq!(fg, Color::Reset);
        assert_eq!(bg, Color::Reset);
    }

    #[test]
    fn solid_cell_uses_full_block() {
        let (glyph, fg, bg) = quadrant_cell(
            Pixel::Index(0),
            Pixel::Index(0),
            Pixel::Index(0),
            Pixel::Index(0),
        );
        assert_eq!(glyph, "\u{2588}");
        assert_eq!(fg, palette_color(0));
        assert_eq!(bg, Color::Reset);
    }

    #[test]
    fn mixed_two_color_cell_chooses_quadrant_glyph() {
        // TL filled with palette 0, rest empty → upper-left block ▘
        let (glyph, _, bg) =
            quadrant_cell(Pixel::Index(0), Pixel::Empty, Pixel::Empty, Pixel::Empty);
        assert_eq!(glyph, "\u{2598}");
        assert_eq!(bg, Color::Reset);
    }

    #[test]
    fn render_is_no_op_when_area_too_small() {
        let state = fixture();
        let area = Rect::new(0, 0, 4, 2);
        let mut buffer = Buffer::empty(area);
        render_mascot(&state, area, &mut buffer);
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
