//! Catppuccin Mocha colour palette + paint helpers, shared across the GUI.
//!
//! Kept as one module so colours live in a single place (a future step can make
//! these load from the `[theme]` config / theme TOML files).

use aurea::render::{Color, Paint, PaintStyle};
use ozone_syntax::TokenKind;

// --- editor surface ---
pub(crate) const BG: Color = Color::rgb(30, 30, 46);
pub(crate) const GUTTER_BG: Color = Color::rgb(24, 24, 37);
pub(crate) const GUTTER_FG: Color = Color::rgb(88, 91, 112);
pub(crate) const GUTTER_ACT: Color = Color::rgb(205, 214, 244);
pub(crate) const BORDER: Color = Color::rgb(49, 50, 68);
pub(crate) const CURSOR_BG: Color = Color::rgba(245, 224, 220, 220);
pub(crate) const CURSOR_LINE: Color = Color::rgba(49, 50, 68, 140);
pub(crate) const ACTIVE_PANE_BORDER: Color = Color::rgb(137, 180, 250);
pub(crate) const BRACKET_MATCH: Color = Color::rgba(137, 180, 250, 70);
pub(crate) const SCROLLBAR_THUMB: Color = Color::rgba(88, 91, 112, 180);

// --- status bar ---
pub(crate) const STATUSBAR_BG: Color = Color::rgb(24, 24, 37);
pub(crate) const STATUSBAR_FG: Color = Color::rgb(166, 227, 161);
pub(crate) const STATUSBAR_DIM: Color = Color::rgb(137, 180, 250);
pub(crate) const STATUS_MODE_BG: Color = Color::rgb(49, 50, 68);

// --- overlays (picker / search) ---
pub(crate) const PALETTE_SCRIM: Color = Color::rgba(0, 0, 0, 110);
pub(crate) const PALETTE_BG: Color = Color::rgb(24, 24, 37);
pub(crate) const PALETTE_BORDER: Color = Color::rgb(69, 71, 90);
pub(crate) const PALETTE_INPUT_BG: Color = Color::rgb(17, 17, 27);
pub(crate) const PALETTE_SEL: Color = Color::rgb(49, 50, 68);
pub(crate) const PALETTE_FG: Color = Color::rgb(205, 214, 244);
pub(crate) const PALETTE_DESC: Color = Color::rgb(127, 132, 156);
pub(crate) const PALETTE_PROMPT: Color = Color::rgb(203, 166, 247);
pub(crate) const SEARCH_MATCH: Color = Color::rgba(249, 226, 175, 70); // yellow, all matches
pub(crate) const SEARCH_CURRENT: Color = Color::rgba(250, 179, 135, 150); // peach, current match

/// Catppuccin Mocha syntax token colours.
pub(crate) fn token_color(kind: TokenKind) -> Color {
    match kind {
        TokenKind::Keyword => Color::rgb(203, 166, 247),        // mauve
        TokenKind::KeywordControl => Color::rgb(243, 139, 168), // red
        TokenKind::Type => Color::rgb(137, 180, 250),           // blue
        TokenKind::String => Color::rgb(166, 227, 161),         // green
        TokenKind::Comment => Color::rgb(88, 91, 112),          // overlay0
        TokenKind::Number => Color::rgb(250, 179, 135),         // peach
        TokenKind::Macro => Color::rgb(137, 220, 235),          // sky
        TokenKind::Attribute => Color::rgb(245, 194, 231),      // flamingo
        TokenKind::Lifetime => Color::rgb(245, 194, 231),       // flamingo
        TokenKind::Function => Color::rgb(137, 180, 250),       // blue
        TokenKind::Operator => Color::rgb(137, 220, 235),       // sky
        TokenKind::SectionHeader => Color::rgb(203, 166, 247),  // mauve
        _ => Color::rgb(205, 214, 244),                         // text
    }
}

// --- terminal ---
/// Default terminal foreground/background (match the editor surface).
pub(crate) const TERM_FG: Color = Color::rgb(205, 214, 244);
pub(crate) const TERM_BG: Color = BG;

/// The 16 ANSI colours, Catppuccin Mocha flavour (0-7 normal, 8-15 bright).
const ANSI16: [Color; 16] = [
    Color::rgb(69, 71, 90),    // 0 black   (surface1)
    Color::rgb(243, 139, 168), // 1 red
    Color::rgb(166, 227, 161), // 2 green
    Color::rgb(249, 226, 175), // 3 yellow
    Color::rgb(137, 180, 250), // 4 blue
    Color::rgb(245, 194, 231), // 5 magenta (pink)
    Color::rgb(148, 226, 213), // 6 cyan    (teal)
    Color::rgb(186, 194, 222), // 7 white   (subtext1)
    Color::rgb(88, 91, 112),   // 8 br black (surface2)
    Color::rgb(243, 139, 168), // 9 br red
    Color::rgb(166, 227, 161), // 10 br green
    Color::rgb(249, 226, 175), // 11 br yellow
    Color::rgb(137, 180, 250), // 12 br blue
    Color::rgb(245, 194, 231), // 13 br magenta
    Color::rgb(148, 226, 213), // 14 br cyan
    Color::rgb(166, 173, 200), // 15 br white (subtext0)
];

/// Resolve an xterm 256-colour index to RGB: 0-15 ANSI, 16-231 the 6x6x6 cube,
/// 232-255 the 24-step grayscale ramp.
fn xterm256(idx: u8) -> Color {
    match idx {
        0..=15 => ANSI16[idx as usize],
        16..=231 => {
            let i = idx - 16;
            let r = i / 36;
            let g = (i % 36) / 6;
            let b = i % 6;
            let scale = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Color::rgb(scale(r), scale(g), scale(b))
        }
        _ => {
            let v = 8 + (idx - 232) * 10;
            Color::rgb(v, v, v)
        }
    }
}

/// Resolve a terminal cell colour to a concrete RGB, using `default` for
/// [`ozone_term::Color::Default`].
pub(crate) fn term_color(c: ozone_term::Color, default: Color) -> Color {
    match c {
        ozone_term::Color::Default => default,
        ozone_term::Color::Indexed(i) => xterm256(i),
        ozone_term::Color::Rgb(r, g, b) => Color::rgb(r, g, b),
    }
}

/// A solid fill paint.
pub(crate) fn solid(c: Color) -> Paint {
    Paint::new().color(c).style(PaintStyle::Fill)
}

/// A stroke paint of the given width.
pub(crate) fn stroke(c: Color, w: f32) -> Paint {
    Paint::new().color(c).style(PaintStyle::Stroke).stroke_width(w)
}
