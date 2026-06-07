//! Generic UI component layer.
//!
//! The base widgets every overlay is built from — panels, pills — plus the
//! shared [`Style`] they read colours from. Overlays (`picker`, `minibuffer`,
//! `search`, `notify`, `whichkey`, the status bar) compose these instead of
//! reaching into the theme directly, so the look is defined once here and a
//! theme change flows through uniformly.
//!
//! Components are stateless drawing helpers: they take a `DrawingContext` + a
//! `Rect` and paint. State (selection, query text, lifetimes) stays in the
//! overlay that owns it.

pub(crate) mod field;
pub(crate) mod list;
pub(crate) mod panel;
pub(crate) mod pill;

pub(crate) use field::*;
pub(crate) use list::*;
pub(crate) use panel::*;
pub(crate) use pill::*;

use aurea::render::Color;

use crate::theme::palette;

/// A snapshot of the colours components paint with, derived from the active
/// [`crate::theme`] palette. One call site (`style()`) maps theme fields to
/// component-facing roles, so widgets never name raw palette fields.
pub(crate) struct Style {
    /// Panel fill + 1px border ring.
    pub panel_bg: Color,
    pub panel_border: Color,
    /// Dim scrim painted behind a modal panel.
    pub scrim: Color,
    /// Primary / secondary text.
    pub fg: Color,
    pub dim: Color,
    /// Accent (prompts, highlighted keys).
    pub accent: Color,
    /// Selected-row background.
    pub selection: Color,
    /// Inset input-field background.
    pub input_bg: Color,
    /// Pill / badge fill.
    pub chip_bg: Color,
}

/// The current component style, mapped from the active theme palette.
pub(crate) fn style() -> Style {
    let p = palette();
    Style {
        panel_bg: p.picker_bg,
        panel_border: p.picker_border,
        scrim: p.scrim,
        fg: p.picker_fg,
        dim: p.picker_detail,
        accent: p.picker_prompt,
        selection: p.picker_selection,
        input_bg: p.picker_input_bg,
        chip_bg: p.status_mode_bg,
    }
}
