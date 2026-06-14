use aurea::AureaResult;
use aurea::render::{DrawingContext, Rect};

use ozone_config::Config;
use ozone_editor::Workspace;

use crate::input::ActiveMods;
use crate::layout::{STATUS_H, pane_layout};
use crate::lsp::LspStatus;
use crate::overlay::search::SearchState;
use crate::statusbar::draw_status_bar;
use crate::theme::{palette, solid};
use crate::{ImageCache, SyntaxCache, TermCells, editor_font};

mod decorations;
mod image;
mod text;
mod view;

use view::draw_view;

#[derive(Debug, Clone, Copy)]
pub(super) struct TextMetrics {
    pub char_w: f32,
    pub text_ascent: f32,
    pub text_descent: f32,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_editor(
    ctx: &mut dyn DrawingContext,
    ws: &mut Workspace,
    config: &Config,
    welcome_bindings: &[(String, String)],
    search: Option<&SearchState>,
    term_cells: &TermCells,
    images: &ImageCache,
    syntax_cache: &mut SyntaxCache,
    mods: ActiveMods,
    cursor_visible: bool,
    lsp_status: LspStatus,
    char_w_out: &mut f32,
) -> AureaResult<()> {
    let width = ctx.width() as f32;
    let height = ctx.height() as f32;

    ctx.clear(palette().background)?;

    let font = editor_font(config);
    let metrics = ctx.measure_text("M", &font).ok();
    let char_w = metrics
        .as_ref()
        .map(|m| m.advance)
        .unwrap_or(font.size * 0.6);
    let text_ascent = metrics
        .as_ref()
        .map(|m| m.ascent)
        .unwrap_or(font.size * 0.8);
    let text_descent = metrics
        .as_ref()
        .map(|m| m.descent)
        .unwrap_or(font.size * 0.2);
    *char_w_out = char_w;

    let editor_rect = Rect::new(0.0, 0.0, width, (height - STATUS_H).max(0.0));
    let metrics = TextMetrics {
        char_w,
        text_ascent,
        text_descent,
    };

    if let Some(panes) = &ws.panes {
        let (leaves, dividers) = pane_layout(panes, editor_rect);
        for (view_id, rect) in leaves {
            draw_view(
                ctx,
                ws,
                config,
                view_id,
                rect,
                &font,
                metrics,
                welcome_bindings,
                term_cells,
                images,
                syntax_cache,
                cursor_visible,
            )?;
        }
        for divider in dividers {
            ctx.draw_rect(divider, &solid(palette().border))?;
        }
    } else if let Some(view_id) = ws.active_view().map(|view| view.id) {
        draw_view(
            ctx,
            ws,
            config,
            view_id,
            editor_rect,
            &font,
            metrics,
            welcome_bindings,
            term_cells,
            images,
            syntax_cache,
            cursor_visible,
        )?;
    }

    if let Some(s) = search {
        use crate::overlay::search::draw_search_bar;
        draw_search_bar(ctx, s, &font, width)?;
    }

    draw_status_bar(ctx, width, height, &font, ws, mods, lsp_status)?;
    Ok(())
}
