//! 16×16 pixel frame data for the Peridot deer mascot.
//!
//! Each [`MascotFrame`] is a row-major 16×16 grid of [`Pixel`]s. The
//! renderer compresses every 2×2 block into a single terminal cell
//! using Unicode quadrant block glyphs (`▘▝▖▗▙▟▛▜▀▄▌▐█ `) — so the
//! sprite shows up at the same 8 columns × 4 rows footprint as the
//! old 8×8 deer but holds 4× the pixel detail.
//!
//! Design rule: keep each 2×2 quadrant down to two distinct colours
//! (one foreground + one background, where `Pixel::Empty` counts as
//! "transparent background"). The renderer picks two colours per
//! cell automatically when the rule is broken, but the result looks
//! cleaner when frames respect it.

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

/// One frame of mascot animation. 16×16 row-major.
#[derive(Clone, Copy, Debug)]
pub struct MascotFrame {
    /// 16 rows × 16 columns of palette indices.
    pub pixels: [[Pixel; 16]; 16],
}

/// Sprite palette tuned to the Peridot deer reference art:
/// deep antler green, mid body green, light body highlight, a
/// 3-step peridot gem (outer / core / sparkle), a near-black eye,
/// a tiny pink nose, and one warm hoof brown.
pub const fn peridot_palette() -> [Color; 9] {
    [
        Color::Rgb(54, 92, 30),    // 0: antler / outline dark green
        Color::Rgb(133, 178, 64),  // 1: body mid green
        Color::Rgb(210, 232, 130), // 2: body highlight (light green)
        Color::Rgb(40, 130, 56),   // 3: gem outline / deep peridot
        Color::Rgb(96, 220, 110),  // 4: gem core (bright green)
        Color::Rgb(225, 255, 215), // 5: gem sparkle / eye-shine highlight
        Color::Rgb(28, 28, 32),    // 6: eye (near-black with a green cast)
        Color::Rgb(255, 182, 193), // 7: nose pink
        Color::Rgb(90, 56, 30),    // 8: hoof brown
    ]
}

/// Resolves a palette index to a ratatui Color, falling back to body green.
pub fn palette_color(index: u8) -> Color {
    let palette = peridot_palette();
    palette.get(index as usize).copied().unwrap_or(palette[1])
}

const E: Pixel = Pixel::Empty;
const D: Pixel = Pixel::Index(0); // antler / outline dark green
const G: Pixel = Pixel::Index(1); // body mid green
const L: Pixel = Pixel::Index(2); // body highlight light
const J: Pixel = Pixel::Index(3); // gem outline
const C: Pixel = Pixel::Index(4); // gem core
const S: Pixel = Pixel::Index(5); // gem sparkle / eye shine
const K: Pixel = Pixel::Index(6); // eye black
const N: Pixel = Pixel::Index(7); // nose pink
const H: Pixel = Pixel::Index(8); // hoof brown

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

// =====================================================================
// BASE — the deer at rest. Tall paired antlers, large round head with
// two black eyes flanking a pink nose, peridot gem at the chest,
// stocky body, two short legs ending in brown hooves. Every other
// frame edits one or two rows of this layout so the operator
// perceives small distinct twitches per mood rather than an entirely
// different sprite each tick.
// =====================================================================
const BASE: MascotFrame = MascotFrame {
    pixels: [
        // Row 0 — antler tips
        [E, E, E, D, E, E, E, E, E, E, E, E, D, E, E, E],
        // Row 1 — antler upper branches
        [E, E, D, D, D, E, E, E, E, E, E, D, D, D, E, E],
        // Row 2 — antler mid
        [E, E, E, D, E, E, E, E, E, E, E, E, D, E, E, E],
        // Row 3 — antler bases meeting the head
        [E, E, E, D, D, E, E, E, E, E, E, D, D, E, E, E],
        // Row 4 — head crown
        [E, E, E, E, D, L, L, L, L, L, L, D, E, E, E, E],
        // Row 5 — forehead
        [E, E, E, D, L, L, L, L, L, L, L, L, D, E, E, E],
        // Row 6 — eyes
        [E, E, E, D, L, K, L, L, L, L, K, L, D, E, E, E],
        // Row 7 — under-eye
        [E, E, E, D, L, L, L, L, L, L, L, L, D, E, E, E],
        // Row 8 — nose
        [E, E, E, D, L, L, L, N, N, L, L, L, D, E, E, E],
        // Row 9 — chin
        [E, E, E, D, L, L, L, L, L, L, L, L, D, E, E, E],
        // Row 10 — head-to-body
        [E, E, D, L, L, L, L, L, L, L, L, L, L, D, E, E],
        // Row 11 — body + gem top
        [E, D, L, L, L, G, G, J, J, G, G, L, L, L, D, E],
        // Row 12 — gem middle
        [E, D, L, L, G, J, C, C, C, C, J, G, L, L, D, E],
        // Row 13 — gem bottom
        [E, D, L, L, G, G, J, C, C, J, G, G, L, L, D, E],
        // Row 14 — lower body
        [E, D, D, L, L, G, G, G, G, G, G, L, L, D, D, E],
        // Row 15 — hooves
        [E, E, H, H, E, E, E, E, E, E, E, E, H, H, E, E],
    ],
};

// =====================================================================
// IDLE — base + a blink (eyes close to highlight tone for one frame).
// =====================================================================
const BLINK: MascotFrame = MascotFrame {
    pixels: [
        BASE.pixels[0],
        BASE.pixels[1],
        BASE.pixels[2],
        BASE.pixels[3],
        BASE.pixels[4],
        BASE.pixels[5],
        // Closed eyes — `K` becomes `L`.
        [E, E, E, D, L, L, L, L, L, L, L, L, D, E, E, E],
        BASE.pixels[7],
        BASE.pixels[8],
        BASE.pixels[9],
        BASE.pixels[10],
        BASE.pixels[11],
        BASE.pixels[12],
        BASE.pixels[13],
        BASE.pixels[14],
        BASE.pixels[15],
    ],
};

const IDLE: [MascotFrame; 2] = [BASE, BLINK];

// =====================================================================
// THINKING — gentle right-antler twitch (the right tip shifts inward
// by one cell to suggest a head tilt).
// =====================================================================
const THINKING_TWITCH: MascotFrame = MascotFrame {
    pixels: [
        [E, E, E, D, E, E, E, E, E, E, E, D, E, E, E, E],
        BASE.pixels[1],
        BASE.pixels[2],
        BASE.pixels[3],
        BASE.pixels[4],
        BASE.pixels[5],
        BASE.pixels[6],
        BASE.pixels[7],
        BASE.pixels[8],
        BASE.pixels[9],
        BASE.pixels[10],
        BASE.pixels[11],
        BASE.pixels[12],
        BASE.pixels[13],
        BASE.pixels[14],
        BASE.pixels[15],
    ],
};

const THINKING: [MascotFrame; 2] = [BASE, THINKING_TWITCH];

// =====================================================================
// TOOL_RUNNING — chest gem pulses dim → mid → bright over three
// frames. Body and head unchanged so the eye is drawn to the gem.
// =====================================================================
const TOOL_GLOW_DIM: MascotFrame = MascotFrame {
    pixels: [
        BASE.pixels[0],
        BASE.pixels[1],
        BASE.pixels[2],
        BASE.pixels[3],
        BASE.pixels[4],
        BASE.pixels[5],
        BASE.pixels[6],
        BASE.pixels[7],
        BASE.pixels[8],
        BASE.pixels[9],
        BASE.pixels[10],
        // Gem outline becomes core green (J → C on row 11/13).
        [E, D, L, L, L, G, G, C, C, G, G, L, L, L, D, E],
        BASE.pixels[12],
        [E, D, L, L, G, G, C, C, C, C, G, G, L, L, D, E],
        BASE.pixels[14],
        BASE.pixels[15],
    ],
};
const TOOL_GLOW_BRIGHT: MascotFrame = MascotFrame {
    pixels: [
        BASE.pixels[0],
        BASE.pixels[1],
        BASE.pixels[2],
        BASE.pixels[3],
        BASE.pixels[4],
        BASE.pixels[5],
        BASE.pixels[6],
        BASE.pixels[7],
        BASE.pixels[8],
        BASE.pixels[9],
        BASE.pixels[10],
        // Whole gem brightens — entire facets become sparkle white.
        [E, D, L, L, L, C, C, S, S, C, C, L, L, L, D, E],
        [E, D, L, L, C, S, S, S, S, S, S, C, L, L, D, E],
        [E, D, L, L, C, C, S, S, S, S, C, C, L, L, D, E],
        BASE.pixels[14],
        BASE.pixels[15],
    ],
};

const TOOL_RUNNING: [MascotFrame; 3] = [BASE, TOOL_GLOW_DIM, TOOL_GLOW_BRIGHT];

// =====================================================================
// APPROVAL_WAITING — eyes widen: a sparkle pixel sits next to each
// pupil so the deer looks alert and asking.
// =====================================================================
const APPROVAL_FRAME: MascotFrame = MascotFrame {
    pixels: [
        BASE.pixels[0],
        BASE.pixels[1],
        BASE.pixels[2],
        BASE.pixels[3],
        BASE.pixels[4],
        BASE.pixels[5],
        // Pupils flanked by sparkle highlights for the wide-eyed look.
        [E, E, E, D, L, K, S, L, L, S, K, L, D, E, E, E],
        BASE.pixels[7],
        BASE.pixels[8],
        BASE.pixels[9],
        BASE.pixels[10],
        BASE.pixels[11],
        BASE.pixels[12],
        BASE.pixels[13],
        BASE.pixels[14],
        BASE.pixels[15],
    ],
};

const APPROVAL: [MascotFrame; 1] = [APPROVAL_FRAME];

// =====================================================================
// ASK_USER — curious upright. Sparkle pixels on the head crown stand
// in for raised ears.
// =====================================================================
const ASK_USER_FRAME: MascotFrame = MascotFrame {
    pixels: [
        BASE.pixels[0],
        BASE.pixels[1],
        BASE.pixels[2],
        BASE.pixels[3],
        BASE.pixels[4],
        [E, E, E, D, S, L, L, L, L, L, L, S, D, E, E, E],
        BASE.pixels[6],
        BASE.pixels[7],
        BASE.pixels[8],
        BASE.pixels[9],
        BASE.pixels[10],
        BASE.pixels[11],
        BASE.pixels[12],
        BASE.pixels[13],
        BASE.pixels[14],
        BASE.pixels[15],
    ],
};

const ASK_USER: [MascotFrame; 1] = [ASK_USER_FRAME];

// =====================================================================
// DONE — happy bounce. Hooves visibly lift one row so the deer
// reads as airborne for one frame.
// =====================================================================
const DONE_BOUNCE: MascotFrame = MascotFrame {
    pixels: [
        BASE.pixels[0],
        BASE.pixels[1],
        BASE.pixels[2],
        BASE.pixels[3],
        BASE.pixels[4],
        BASE.pixels[5],
        BASE.pixels[6],
        BASE.pixels[7],
        BASE.pixels[8],
        BASE.pixels[9],
        BASE.pixels[10],
        BASE.pixels[11],
        BASE.pixels[12],
        BASE.pixels[13],
        BASE.pixels[14],
        // Hooves lifted — only the inside columns hit the ground.
        [E, E, E, H, E, E, E, E, E, E, E, E, H, E, E, E],
    ],
};

const DONE: [MascotFrame; 2] = [BASE, DONE_BOUNCE];

// =====================================================================
// FAILED — ears drooping. Antler upper branches collapse downward,
// eyes close (sad / disappointed).
// =====================================================================
const FAILED_FRAME: MascotFrame = MascotFrame {
    pixels: [
        [E; 16],
        [E, E, E, D, E, E, E, E, E, E, E, E, D, E, E, E],
        [E, E, E, D, D, E, E, E, E, E, E, D, D, E, E, E],
        [E, E, E, E, D, E, E, E, E, E, E, D, E, E, E, E],
        BASE.pixels[4],
        BASE.pixels[5],
        [E, E, E, D, L, L, L, L, L, L, L, L, D, E, E, E],
        BASE.pixels[7],
        BASE.pixels[8],
        BASE.pixels[9],
        BASE.pixels[10],
        BASE.pixels[11],
        BASE.pixels[12],
        BASE.pixels[13],
        BASE.pixels[14],
        BASE.pixels[15],
    ],
};

const FAILED: [MascotFrame; 1] = [FAILED_FRAME];

// =====================================================================
// INTERRUPTED — startled. Antlers stand straight (every row 0-3 has
// a single vertical antler pixel), pupils enlarge into a 2-cell stare.
// =====================================================================
const INTERRUPTED_FRAME: MascotFrame = MascotFrame {
    pixels: [
        [E, E, E, D, E, E, E, E, E, E, E, E, D, E, E, E],
        [E, E, E, D, E, E, E, E, E, E, E, E, D, E, E, E],
        [E, E, E, D, E, E, E, E, E, E, E, E, D, E, E, E],
        [E, E, E, D, D, E, E, E, E, E, E, D, D, E, E, E],
        BASE.pixels[4],
        BASE.pixels[5],
        // Big startled pupils — two K pixels each side.
        [E, E, E, D, L, K, K, L, L, K, K, L, D, E, E, E],
        BASE.pixels[7],
        BASE.pixels[8],
        BASE.pixels[9],
        BASE.pixels[10],
        BASE.pixels[11],
        BASE.pixels[12],
        BASE.pixels[13],
        BASE.pixels[14],
        BASE.pixels[15],
    ],
};

const INTERRUPTED: [MascotFrame; 1] = [INTERRUPTED_FRAME];
