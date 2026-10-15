//! Thin adapters from Ozone's theme and modal state to Aurea's canvas UI.
//! Rendering lives in Aurea; editor actions and overlay placement stay here.

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

// These adapters only paint: they register no hit targets and receive no
// input. Reuse Aurea's text measurement cache on each drawing thread without
// taking keyboard focus away from Ozone's modal event routing.
std::thread_local! {
    static PAINT_STATE: std::cell::RefCell<aurea::ui::UiState<u8>> =
        std::cell::RefCell::new(aurea::ui::UiState::new());
}

pub(crate) fn with_ui<R>(
    ctx: &mut dyn aurea::render::DrawingContext,
    paint: impl FnOnce(&mut aurea::ui::Ui<'_, u8>) -> aurea::AureaResult<R>,
) -> aurea::AureaResult<R> {
    PAINT_STATE.with(|state| {
        let mut state = state.borrow_mut();
        let theme = aurea::ui::Theme::dark();
        let mut ui = state.frame(ctx, &theme);
        paint(&mut ui)
    })
}
