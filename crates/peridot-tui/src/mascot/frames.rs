//! 8×8 pixel frame data for the Peridot deer mascot.
//!
//! Each [`MascotFrame`] is a row-major grid of [`Pixel`]s. Frames use a tiny
//! 7-entry palette indexed by `Pixel::Index`, which keeps the per-frame data
//! compact and lets us re-skin the mascot by swapping the palette later.

use ratatui::style::Color;

use super::MascotState;

/// Palette index referenced from each pixel slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Pixel {
    /// Transparent — leaves the terminal cell background untouched.
    Empty,
    /// Lookup into [`peridot_palette`] by index.
    Index(u8),
}

/// One frame of mascot animation.
#[derive(Clone, Copy, Debug)]
pub struct MascotFrame {
    /// 8 rows × 8 columns of palette indices.
    pub pixels: [[Pixel; 8]; 8],
}

/// Returns the active palette: peridot greens + warm browns + accent colors.
pub const fn peridot_palette() -> [Color; 7] {
    [
        Color::Rgb(165, 199, 93),  // 0: body green (peridot core)
        Color::Rgb(213, 235, 153), // 1: body highlight
        Color::Rgb(139, 94, 60),   // 2: brown (legs, hooves)
        Color::Rgb(101, 178, 92),  // 3: deep peridot (antler gem tip)
        // 4: eye — a warm dark brown rather than near-black. The previous
        //    `Rgb(28, 28, 32)` blended into the terminal background on most
        //    dark themes so the deer's eyes vanished; this brown stays
        //    visibly distinct against both the light-green body highlight
        //    and a black terminal cell.
        Color::Rgb(60, 35, 20),
        Color::Rgb(255, 255, 255), // 5: eye shine
        Color::Rgb(255, 182, 193), // 6: nose pink
    ]
}

/// Resolves a palette index to a ratatui Color, falling back to body green.
pub fn palette_color(index: u8) -> Color {
    let palette = peridot_palette();
    palette.get(index as usize).copied().unwrap_or(palette[0])
}

const E: Pixel = Pixel::Empty;
const G: Pixel = Pixel::Index(0);
const L: Pixel = Pixel::Index(1);
const B: Pixel = Pixel::Index(2);
const J: Pixel = Pixel::Index(3); // gem
const K: Pixel = Pixel::Index(4); // black
const W: Pixel = Pixel::Index(5); // white
const N: Pixel = Pixel::Index(6); // nose

/// Returns the frame sequence for a given mood. Every state has at least one
/// frame; multi-frame states cycle through `frames_for(state)[current_index]`.
pub fn frames_for(state: MascotState) -> &'static [MascotFrame] {
    match state {
        MascotState::Idle => &IDLE,
        MascotState::Thinking => &THINKING,
        MascotState::ToolRunning => &TOOL_RUNNING,
        MascotState::ApprovalWaiting => &APPROVAL,
        MascotState::AskUser => &ASK_USER,
        MascotState::Done => &DONE,
        MascotState::Failed => &FAILED,
        MascotState::Interrupted => &INTERRUPTED,
    }
}

// Base sprite: antlers + ears + head + body + legs (deer silhouette).
const BASE: MascotFrame = MascotFrame {
    pixels: [
        [E, J, E, E, E, E, J, E],
        [E, B, E, G, G, E, B, E],
        [E, B, G, L, L, G, B, E],
        [E, G, L, K, L, K, G, E],
        [E, G, G, L, N, L, G, E],
        [E, G, G, G, G, G, G, E],
        [E, B, E, E, E, E, B, E],
        [E, B, E, E, E, E, B, E],
    ],
};

// Idle frame 2 — eyes closed (blink). The eye row swaps from `K` (open
// pupil) to `L` (light cheek-tone) so the eye actually disappears for a
// frame; the original BLINK kept K at row 3 which made the animation
// effectively a no-op visually.
const BLINK: MascotFrame = MascotFrame {
    pixels: [
        [E, J, E, E, E, E, J, E],
        [E, B, E, G, G, E, B, E],
        [E, B, G, L, L, G, B, E],
        [E, G, L, L, L, L, G, E],
        [E, G, G, L, N, L, G, E],
        [E, G, G, G, G, G, G, E],
        [E, B, E, E, E, E, B, E],
        [E, B, E, E, E, E, B, E],
    ],
};

const IDLE: [MascotFrame; 2] = [BASE, BLINK];

// Thinking — small white "shine" floating to the right of the head.
const THINK_A: MascotFrame = MascotFrame {
    pixels: [
        [E, J, E, E, E, E, J, W],
        [E, B, E, G, G, E, B, E],
        [E, B, G, L, L, G, B, E],
        [E, G, L, K, L, K, G, E],
        [E, G, G, L, N, L, G, E],
        [E, G, G, G, G, G, G, E],
        [E, B, E, E, E, E, B, E],
        [E, B, E, E, E, E, B, E],
    ],
};
const THINK_B: MascotFrame = MascotFrame {
    pixels: [
        [E, J, E, E, E, E, J, E],
        [E, B, E, G, G, E, B, W],
        [E, B, G, L, L, G, B, E],
        [E, G, L, K, L, K, G, E],
        [E, G, G, L, N, L, G, E],
        [E, G, G, G, G, G, G, E],
        [E, B, E, E, E, E, B, E],
        [E, B, E, E, E, E, B, E],
    ],
};
const THINKING: [MascotFrame; 3] = [THINK_A, BASE, THINK_B];

// Tool running — gems on antlers light up (extra highlight).
const TOOL_A: MascotFrame = MascotFrame {
    pixels: [
        [J, J, E, E, E, E, J, J],
        [E, B, E, G, G, E, B, E],
        [E, B, G, L, L, G, B, E],
        [E, G, L, K, L, K, G, E],
        [E, G, G, L, N, L, G, E],
        [E, G, G, G, G, G, G, E],
        [E, B, E, E, E, E, B, E],
        [E, B, E, E, E, E, B, E],
    ],
};
const TOOL_B: MascotFrame = MascotFrame {
    pixels: [
        [E, J, J, E, E, J, J, E],
        [E, B, E, G, G, E, B, E],
        [E, B, G, L, L, G, B, E],
        [E, G, L, K, L, K, G, E],
        [E, G, G, L, N, L, G, E],
        [E, G, G, G, G, G, G, E],
        [E, B, E, E, E, E, B, E],
        [E, B, E, E, E, E, B, E],
    ],
};
const TOOL_RUNNING: [MascotFrame; 3] = [BASE, TOOL_A, TOOL_B];

// Approval — head slightly tilted (asymmetric antlers, wide eyes).
const APPROVAL_FRAME: MascotFrame = MascotFrame {
    pixels: [
        [J, E, E, E, E, E, E, J],
        [B, E, E, G, G, E, B, E],
        [B, G, G, L, L, G, B, E],
        [E, G, W, K, L, K, G, E],
        [E, G, G, L, N, L, G, E],
        [E, G, G, G, G, G, G, E],
        [E, B, E, E, E, E, B, E],
        [E, B, E, E, E, E, B, E],
    ],
};
const APPROVAL: [MascotFrame; 1] = [APPROVAL_FRAME];

// AskUser — pupils larger (curious).
const ASK_FRAME: MascotFrame = MascotFrame {
    pixels: [
        [E, J, E, E, E, E, J, E],
        [E, B, E, G, G, E, B, E],
        [E, B, G, L, L, G, B, E],
        [E, G, K, K, L, K, K, E],
        [E, G, G, L, N, L, G, E],
        [E, G, G, G, G, G, G, E],
        [E, B, E, E, E, E, B, E],
        [E, B, E, E, E, E, B, E],
    ],
};
const ASK_USER: [MascotFrame; 1] = [ASK_FRAME];

// Done — visible ^_^ eyes (closed upward arcs) + small bounce. The
// previous Done frames left the eye row as plain body green, so the
// deer's face went blank when a task completed. The K (brown) pixels
// at row 2 cols 3 & 5 now render as the top of a closed-smiling eye;
// the bottom half of the eye-cell is body green (no K below) so the
// shape reads as `^^`.
const DONE_A: MascotFrame = MascotFrame {
    pixels: [
        [E, J, E, E, E, E, J, E],
        [E, B, E, G, G, E, B, E],
        [E, B, G, K, L, K, B, E],
        [E, G, L, G, L, G, G, E],
        [E, G, G, L, N, L, G, E],
        [E, G, G, G, G, G, G, E],
        [E, B, E, E, E, E, B, E],
        [E, B, E, E, E, E, B, E],
    ],
};
const DONE_B: MascotFrame = MascotFrame {
    pixels: [
        [E, J, E, E, E, E, J, E],
        [E, B, G, K, L, K, B, E],
        [E, G, L, G, L, G, G, E],
        [E, G, G, L, N, L, G, E],
        [E, G, G, G, G, G, G, E],
        [E, B, E, E, E, E, B, E],
        [E, B, E, E, E, E, B, E],
        [E, E, E, E, E, E, E, E],
    ],
};
const DONE: [MascotFrame; 2] = [DONE_A, DONE_B];

// Failed — ears drooping, eyes shut as `x_x`. The previous frame left
// the eye row as plain body green so the deer's face vanished on
// failure; now K pixels sit at row 3 cols 3 & 5 so the closed eyes
// remain readable while the rest of the face still reads as defeated.
const FAILED_FRAME: MascotFrame = MascotFrame {
    pixels: [
        [E, J, E, E, E, E, J, E],
        [E, B, E, E, E, E, B, E],
        [E, E, G, G, G, G, E, E],
        [E, G, L, K, L, K, G, E],
        [E, G, G, L, N, L, G, E],
        [E, G, G, G, G, G, G, E],
        [E, B, E, E, E, E, B, E],
        [E, B, E, E, E, E, B, E],
    ],
};
const FAILED: [MascotFrame; 1] = [FAILED_FRAME];

// Interrupted — startled, ears straight up, wide eyes.
const INTERRUPTED_FRAME: MascotFrame = MascotFrame {
    pixels: [
        [E, J, J, E, E, J, J, E],
        [E, B, B, G, G, B, B, E],
        [E, B, G, L, L, G, B, E],
        [E, G, W, K, W, K, G, E],
        [E, G, G, L, N, L, G, E],
        [E, G, G, G, G, G, G, E],
        [E, B, E, E, E, E, B, E],
        [E, B, E, E, E, E, B, E],
    ],
};
const INTERRUPTED: [MascotFrame; 1] = [INTERRUPTED_FRAME];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_state_has_at_least_one_frame() {
        for state in [
            MascotState::Idle,
            MascotState::Thinking,
            MascotState::ToolRunning,
            MascotState::ApprovalWaiting,
            MascotState::AskUser,
            MascotState::Done,
            MascotState::Failed,
            MascotState::Interrupted,
        ] {
            assert!(!frames_for(state).is_empty(), "no frames for {state:?}");
        }
    }

    #[test]
    fn palette_has_seven_entries() {
        let palette = peridot_palette();
        assert_eq!(palette.len(), 7);
        assert_eq!(palette_color(0), Color::Rgb(165, 199, 93));
        assert_eq!(palette_color(99), Color::Rgb(165, 199, 93));
    }

    #[test]
    fn frames_are_eight_by_eight() {
        for state in [MascotState::Idle, MascotState::ToolRunning] {
            for frame in frames_for(state) {
                assert_eq!(frame.pixels.len(), 8);
                for row in &frame.pixels {
                    assert_eq!(row.len(), 8);
                }
            }
        }
    }
}
