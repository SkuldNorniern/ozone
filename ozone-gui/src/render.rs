//! Editor rendering: the `draw_*` pass over the workspace.
//!
//! `draw_editor` is the entry point the run loop calls each frame; everything
//! else here is private detail (pane tree, per-view text/gutter/cursor, terminal
//! colour grid, image pane, status bar). Geometry comes from [`crate::layout`],
//! colours from [`crate::theme`]; this module owns no state — it paints the
//! current `Workspace` into a `DrawingContext`.

use aurea::AureaResult;
use aurea::render::{Color, DrawingContext, Font, Image, Point, Rect};

use ozone_buffer::{BufferKind, Pos};
use ozone_config::{Config, CursorStyle, LineNumbers};
use ozone_editor::{
    BRACKET_NAMESPACE, Decoration, DecorationKind, HlRole, PaneTree, ViewId, VirtualPos,
    Workspace, matching_bracket,
};
use ozone_syntax::{Filetype, ScanState, TokenKind, scan_line};

use crate::input::ActiveMods;
use crate::layout::*;
use crate::components::draw_pill;
use crate::search::{SearchState, draw_search_bar};
use crate::theme::{palette, solid, stroke, term_color, token_color};
use crate::{ImageCache, TermCells, editor_font};

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_editor(
    ctx: &mut dyn DrawingContext,
    ws: &mut Workspace,
    config: &Config,
    search: Option<&SearchState>,
    term_cells: &TermCells,
    images: &ImageCache,
    mods: ActiveMods,
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
    // Report the real measured cell width so the PTY can be sized to match.
    *char_w_out = char_w;

    let editor_rect = Rect::new(0.0, 0.0, width, (height - STATUS_H).max(0.0));
    let metrics = TextMetrics {
        char_w,
        text_ascent,
        text_descent,
    };

    if let Some(panes) = &ws.panes {
        let panes = panes.clone();
        draw_pane_tree(
            ctx,
            ws,
            config,
            &panes,
            editor_rect,
            &font,
            metrics,
            term_cells,
            images,
        )?;
    } else if let Some(view_id) = ws.active_view().map(|view| view.id) {
        draw_view(
            ctx,
            ws,
            config,
            view_id,
            editor_rect,
            &font,
            metrics,
            term_cells,
            images,
        )?;
    }

    if let Some(s) = search {
        draw_search_bar(ctx, s, &font, width)?;
    }

    draw_status_bar(ctx, width, height, &font, ws, mods)?;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct TextMetrics {
    char_w: f32,
    text_ascent: f32,
    text_descent: f32,
}

#[allow(clippy::too_many_arguments)]
fn draw_pane_tree(
    ctx: &mut dyn DrawingContext,
    ws: &mut Workspace,
    config: &Config,
    tree: &PaneTree,
    rect: Rect,
    font: &Font,
    metrics: TextMetrics,
    term_cells: &TermCells,
    images: &ImageCache,
) -> AureaResult<()> {
    match tree {
        PaneTree::Leaf { view_id } => draw_view(
            ctx, ws, config, *view_id, rect, font, metrics, term_cells, images,
        ),
        PaneTree::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            let (first_rect, second_rect, divider) = split_rect(rect, *axis, *ratio);
            draw_pane_tree(
                ctx, ws, config, first, first_rect, font, metrics, term_cells, images,
            )?;
            draw_pane_tree(
                ctx,
                ws,
                config,
                second,
                second_rect,
                font,
                metrics,
                term_cells,
                images,
            )?;
            ctx.draw_rect(divider, &solid(palette().border))?;
            Ok(())
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_view(
    ctx: &mut dyn DrawingContext,
    ws: &mut Workspace,
    config: &Config,
    view_id: ViewId,
    rect: Rect,
    font: &Font,
    metrics: TextMetrics,
    term_cells: &TermCells,
    images: &ImageCache,
) -> AureaResult<()> {
    let Some(buffer_id) = ws.views.get(&view_id).map(|view| view.buffer_id) else {
        return Ok(());
    };
    let Some(line_count) = ws.buffers.get(&buffer_id).map(|buf| buf.line_count()) else {
        return Ok(());
    };

    let is_active_pane = ws.active_view_id == Some(view_id);

    // Image buffers render the picture, not text — handle and return early.
    if matches!(
        ws.buffers.get(&buffer_id).map(|b| &b.kind),
        Some(BufferKind::Image(_))
    ) {
        ctx.draw_rect(rect, &solid(palette().background))?;
        let img = images.get(&buffer_id).and_then(|o| o.as_ref());
        draw_image_pane(ctx, rect, img, font, metrics)?;
        if is_active_pane {
            ctx.draw_rect(
                Rect::new(rect.x, rect.y, rect.width, 2.0),
                &solid(palette().active_pane_border),
            )?;
        }
        return Ok(());
    }
    let line_h = font.size * config.editor.line_height;
    let content_top = rect.y + EDITOR_TOP_PAD;
    let content_h = (rect.height - EDITOR_TOP_PAD).max(0.0);
    let visible = ((content_h / line_h) as usize).max(1);

    if let Some(view) = ws.views.get_mut(&view_id) {
        view.page_height = visible;
        view.scroll_line = view.scroll_line.min(max_scroll_line(line_count, visible));
    }
    sync_bracket_decorations(ws, view_id);

    let Some(view) = ws.views.get(&view_id) else {
        return Ok(());
    };
    let Some(buf) = ws.buffers.get(&buffer_id) else {
        return Ok(());
    };

    ctx.draw_rect(rect, &solid(palette().background))?;

    // Filetype for syntax
    let ft = match &buf.kind {
        BufferKind::File(p) => Filetype::from_path(&p.to_string_lossy()),
        _ => Filetype::Plain,
    };

    // Virtual surfaces (terminal, pickers, references) have no line numbers;
    // real buffers use their buffer-local override, else the global default.
    let line_numbers = match buf.kind {
        BufferKind::Terminal
        | BufferKind::Search
        | BufferKind::References
        | BufferKind::Image(_) => LineNumbers::Off,
        _ => ws
            .buffer_local(buffer_id)
            .and_then(|l| l.line_numbers)
            .unwrap_or(config.editor.line_numbers),
    };

    let scroll = view.scroll_line;
    let visible = visible + 1;
    let gutter_w = gutter_width(line_count, metrics.char_w, line_numbers);
    let text_x = rect.x + gutter_w + PAD;

    // For a live terminal, render the colour grid instead of the text buffer.
    let term_grid: Option<&[Vec<ozone_term::Cell>]> = if matches!(buf.kind, BufferKind::Terminal) {
        term_cells.get(&buffer_id).map(|v| v.as_slice())
    } else {
        None
    };

    let visible_start = buf.pos_to_offset(Pos::new(scroll, 0));
    let visible_end_line = (scroll + visible).min(line_count);
    let visible_end = if visible_end_line >= line_count {
        buf.text().len().saturating_add(1)
    } else {
        buf.pos_to_offset(Pos::new(visible_end_line, 0))
    };
    let decorations: Vec<Decoration> = ws
        .decorations()
        .in_range_for_view(buffer_id, view_id, visible_start, visible_end)
        .into_iter()
        .cloned()
        .collect();

    // Gutter strip
    if gutter_w > 0.0 {
        ctx.draw_rect(
            Rect::new(rect.x, rect.y, gutter_w, rect.height),
            &solid(palette().gutter),
        )?;
    }

    // Pre-scan: walk from line 0 to scroll to find block-comment state.
    // Acceptable for Phase 1 file sizes.
    let mut scan_state = ScanState::clean();
    for l in 0..scroll {
        if let Some(text) = buf.line(l) {
            let (_, ns) = scan_line(ft, &text, scan_state);
            scan_state = ns;
        }
    }

    for i in 0..visible {
        let line_idx = scroll + i;
        if line_idx >= line_count {
            break;
        }

        let line_top = content_top + i as f32 * line_h;
        if line_top >= content_top + content_h || line_top >= rect.y + rect.height {
            break;
        }

        let baseline =
            baseline_in_rect(line_top, line_h, metrics.text_ascent, metrics.text_descent);
        let is_cursor = line_idx == view.cursor.line;
        let line_text = buf.line(line_idx).unwrap_or_default();
        let line_start = buf.pos_to_offset(Pos::new(line_idx, 0));
        let line_end = line_start + line_text.len();
        let line_decorations: Vec<&Decoration> = decorations
            .iter()
            .filter(|decoration| {
                if decoration.start == decoration.end {
                    decoration.start >= line_start && decoration.start <= line_end
                } else {
                    decoration.start < line_end && decoration.end > line_start
                }
            })
            .collect();

        // Cursor-line highlight
        if is_cursor && is_active_pane {
            ctx.draw_rect(
                Rect::new(rect.x, line_top + 1.0, rect.width, line_h - 1.0),
                &solid(palette().cursor_line),
            )?;
        }

        // Selection is view-local and byte-oriented, matching the editor's
        // `Pos`/`Span` model. Draw it before search/bracket decorations.
        if let Some(selection) = view.selection
            && line_idx >= selection.start.line
            && line_idx <= selection.end.line
        {
            let line_len = buf.line_len(line_idx);
            let start_col = if line_idx == selection.start.line {
                selection.start.col.min(line_len)
            } else {
                0
            };
            let end_col = if line_idx == selection.end.line {
                selection.end.col.min(line_len)
            } else {
                line_len
            };
            if end_col > start_col {
                let sx = text_x + start_col as f32 * metrics.char_w;
                let sw = (end_col - start_col) as f32 * metrics.char_w;
                ctx.draw_rect(
                    Rect::new(sx, line_top + 1.0, sw, line_h - 2.0),
                    &solid(palette().selection),
                )?;
            }
        }

        for decoration in &line_decorations {
            if let DecorationKind::Highlight(role) = &decoration.kind {
                let start = decoration.start.max(line_start) - line_start;
                let end = decoration.end.min(line_end) - line_start;
                if end > start {
                    ctx.draw_rect(
                        Rect::new(
                            text_x + start as f32 * metrics.char_w,
                            line_top + 1.0,
                            (end - start) as f32 * metrics.char_w,
                            line_h - 2.0,
                        ),
                        &solid(decoration_highlight_color(*role)),
                    )?;
                }
            }
        }

        // Gutter line number (absolute / relative / off per config)
        let gutter_label = match line_numbers {
            LineNumbers::Off => None,
            LineNumbers::Absolute => Some(format!("{:>4}", line_idx + 1)),
            LineNumbers::Relative => {
                if is_cursor {
                    Some(format!("{:<4}", line_idx + 1))
                } else {
                    let dist = line_idx.abs_diff(view.cursor.line);
                    Some(format!("{:>4}", dist))
                }
            }
        };
        if let Some(num) = gutter_label {
            let ng = if is_cursor {
                palette().line_number_active
            } else {
                palette().line_number
            };
            let num_x =
                (rect.x + gutter_w - PAD - num.len() as f32 * metrics.char_w).max(rect.x + 4.0);
            ctx.draw_text_with_font(&num, Point::new(num_x, baseline), font, &solid(ng))?;
        }

        let gutter_signs: String = line_decorations
            .iter()
            .filter_map(|decoration| match &decoration.kind {
                DecorationKind::GutterSign(sign)
                    if decoration.start >= line_start && decoration.start <= line_end =>
                {
                    Some(sign.as_str())
                }
                _ => None,
            })
            .collect();
        if !gutter_signs.is_empty() && gutter_w > 0.0 {
            ctx.draw_text_with_font(
                &gutter_signs,
                Point::new(rect.x + 3.0, baseline),
                font,
                &solid(palette().picker_prompt),
            )?;
        }

        // Line content: terminal colour grid, or syntax-highlighted buffer text.
        if let Some(grid) = term_grid {
            if let Some(row) = grid.get(line_idx) {
                draw_term_row(
                    ctx,
                    row,
                    text_x,
                    line_top,
                    baseline,
                    line_h,
                    metrics.char_w,
                    font,
                )?;
            }
        } else {
            let (spans, new_state) = scan_line(ft, &line_text, scan_state);
            scan_state = new_state;
            let inline_virtual: Vec<&Decoration> = line_decorations
                .iter()
                .copied()
                .filter(|decoration| {
                    let anchored_here =
                        decoration.start >= line_start && decoration.start <= line_end;
                    anchored_here
                        && matches!(
                        &decoration.kind,
                        DecorationKind::VirtualText {
                            pos: VirtualPos::Inline,
                            ..
                        }
                    )
                })
                .collect();

            if !inline_virtual.is_empty() {
                draw_line_with_inline_virtual_text(
                    ctx,
                    &line_text,
                    &spans,
                    &inline_virtual,
                    line_start,
                    text_x,
                    baseline,
                    metrics.char_w,
                    font,
                )?;
            } else if spans.is_empty() || ft == Filetype::Plain {
                ctx.draw_text_with_font(
                    &line_text,
                    Point::new(text_x, baseline),
                    font,
                    &solid(token_color(TokenKind::Default)),
                )?;
            } else {
                draw_highlighted(
                    ctx,
                    &line_text,
                    &spans,
                    text_x,
                    baseline,
                    metrics.char_w,
                    font,
                )?;
            }
        }

        for decoration in &line_decorations {
            match &decoration.kind {
                DecorationKind::Underline(role) => {
                    let start = decoration.start.max(line_start) - line_start;
                    let end = decoration.end.min(line_end) - line_start;
                    if end > start {
                        let y = line_top + line_h - 2.0;
                        ctx.draw_line(
                            text_x + start as f32 * metrics.char_w,
                            y,
                            text_x + end as f32 * metrics.char_w,
                            y,
                            &stroke(decoration_role_color(*role), 1.0),
                        )?;
                    }
                }
                DecorationKind::VirtualText {
                    text,
                    pos: VirtualPos::Eol,
                    role,
                } if decoration.start >= line_start && decoration.start <= line_end => {
                    ctx.draw_text_with_font(
                        text,
                        Point::new(
                            text_x + (line_text.len() + 1) as f32 * metrics.char_w,
                            baseline,
                        ),
                        font,
                        &solid(decoration_role_color(*role)),
                    )?;
                }
                _ => {}
            }
        }

        if is_cursor && is_active_pane {
            draw_cursor(
                ctx,
                text_x + view.cursor.col as f32 * metrics.char_w,
                line_top,
                line_h,
                metrics.char_w,
                config.editor.cursor_style,
            )?;
        }
    }

    // Gutter divider
    if gutter_w > 0.0 {
        ctx.draw_line(
            rect.x + gutter_w,
            rect.y,
            rect.x + gutter_w,
            rect.y + rect.height,
            &stroke(palette().border, 1.0),
        )?;
    }

    // Scrollbar thumb (right edge), only when content overflows the viewport.
    let viewport_lines = (content_h / line_h).max(1.0);
    if (line_count as f32) > viewport_lines {
        let track_h = rect.height;
        let thumb_h = (track_h * viewport_lines / line_count as f32).clamp(24.0, track_h);
        let max_scroll = max_scroll_line(line_count, viewport_lines as usize);
        let t = if max_scroll > 0 {
            (scroll as f32 / max_scroll as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let thumb_y = rect.y + t * (track_h - thumb_h);
        let bar_x = rect.x + rect.width - 4.0;
        ctx.draw_rect(
            Rect::new(bar_x, thumb_y, 3.0, thumb_h),
            &solid(palette().scrollbar_thumb),
        )?;
    }

    if is_active_pane {
        ctx.draw_rect(
            Rect::new(rect.x, rect.y, rect.width, 2.0),
            &solid(palette().active_pane_border),
        )?;
    }

    Ok(())
}

fn decoration_role_color(role: HlRole) -> Color {
    match role {
        HlRole::Search => palette().search_match,
        HlRole::SearchCurrent => palette().search_match_active,
        HlRole::Bracket => palette().bracket_match,
        HlRole::Selection => palette().selection,
        HlRole::Error => palette().notify_error,
        HlRole::Warn => palette().notify_warn,
        HlRole::Info => palette().notify_info,
        HlRole::Hint => palette().picker_detail,
    }
}

fn decoration_highlight_color(role: HlRole) -> Color {
    let color = decoration_role_color(role);
    if role == HlRole::Bracket {
        return color;
    }
    Color::rgba(color.r, color.g, color.b, color.a.min(110))
}

fn sync_bracket_decorations(ws: &mut Workspace, view_id: ViewId) {
    let pair = ws.views.get(&view_id).and_then(|view| {
        let buffer = ws.buffers.get(&view.buffer_id)?;
        let (first, second) = matching_bracket(buffer, view.cursor)?;
        Some((
            view.buffer_id,
            buffer.pos_to_offset(first),
            buffer.pos_to_offset(second),
        ))
    });

    let mut expected = pair.map(|(buffer, first, second)| {
        let mut starts = [first, second];
        starts.sort_unstable();
        (buffer, starts)
    });
    let mut current: Vec<_> = ws
        .decorations()
        .namespace_for_view(BRACKET_NAMESPACE, view_id)
        .into_iter()
        .filter_map(|(buffer, decoration)| {
            matches!(
                decoration.kind,
                DecorationKind::Highlight(HlRole::Bracket)
            )
            .then_some((buffer, decoration.start, decoration.end))
        })
        .collect();
    current.sort_by_key(|(_, start, _)| *start);
    let unchanged = match (expected.as_ref(), current.as_slice()) {
        (None, []) => true,
        (Some((buffer, [first, second])), [(a, a_start, a_end), (b, b_start, b_end)]) => {
            a == buffer
                && b == buffer
                && (*a_start, *a_end) == (*first, first + 1)
                && (*b_start, *b_end) == (*second, second + 1)
        }
        _ => false,
    };
    if unchanged {
        return;
    }

    ws.decorations_mut()
        .clear_namespace_for_view(BRACKET_NAMESPACE, view_id);
    let Some((buffer, [first, second])) = expected.take() else {
        return;
    };
    for start in [first, second] {
        ws.decorations_mut().add_for_view(
            buffer,
            view_id,
            BRACKET_NAMESPACE,
            start,
            start + 1,
            DecorationKind::Highlight(HlRole::Bracket),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozone_buffer::Pos;
    use ozone_editor::SplitAxis;

    #[test]
    fn bracket_decorations_are_independent_per_split_view() {
        let mut ws = Workspace::new();
        let first = ws.active_view_id.unwrap();
        ws.active_buffer_mut()
            .unwrap()
            .insert(Pos::zero(), "(a) [b]");
        ws.views.get_mut(&first).unwrap().cursor = Pos::new(0, 0);
        let second = ws.split_active_pane(SplitAxis::Vertical).unwrap();
        ws.views.get_mut(&second).unwrap().cursor = Pos::new(0, 4);
        let buffer = ws.active_buffer().unwrap().id;

        sync_bracket_decorations(&mut ws, first);
        sync_bracket_decorations(&mut ws, second);

        let first_ranges: Vec<_> = ws
            .decorations()
            .in_range_for_view(buffer, first, 0, 10)
            .into_iter()
            .map(|d| (d.start, d.end))
            .collect();
        let second_ranges: Vec<_> = ws
            .decorations()
            .in_range_for_view(buffer, second, 0, 10)
            .into_iter()
            .map(|d| (d.start, d.end))
            .collect();
        assert_eq!(first_ranges, vec![(0, 1), (2, 3)]);
        assert_eq!(second_ranges, vec![(4, 5), (6, 7)]);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_line_with_inline_virtual_text(
    ctx: &mut dyn DrawingContext,
    text: &str,
    spans: &[ozone_syntax::TokenSpan],
    decorations: &[&Decoration],
    line_start: usize,
    x0: f32,
    baseline: f32,
    char_w: f32,
    font: &Font,
) -> AureaResult<()> {
    let mut decorations = decorations.to_vec();
    decorations.sort_by_key(|decoration| decoration.start);
    let mut decoration_index = 0usize;
    let mut x = x0;

    for (byte, ch) in text.char_indices() {
        while let Some(decoration) = decorations.get(decoration_index)
            && decoration.start.saturating_sub(line_start) <= byte
        {
            if let DecorationKind::VirtualText { text, role, .. } = &decoration.kind {
                ctx.draw_text_with_font(
                    text,
                    Point::new(x, baseline),
                    font,
                    &solid(decoration_role_color(*role)),
                )?;
                x += ctx
                    .measure_text(text, font)
                    .map(|metrics| metrics.advance)
                    .unwrap_or(text.chars().count() as f32 * char_w);
            }
            decoration_index += 1;
        }

        let mut encoded = [0u8; 4];
        let glyph = ch.encode_utf8(&mut encoded);
        let kind = spans
            .iter()
            .find(|span| byte >= span.start && byte < span.start + span.len)
            .map(|span| span.kind)
            .unwrap_or(TokenKind::Default);
        ctx.draw_text_with_font(
            glyph,
            Point::new(x, baseline),
            font,
            &solid(token_color(kind)),
        )?;
        x += ctx
            .measure_text(glyph, font)
            .map(|metrics| metrics.advance)
            .unwrap_or(char_w);
    }

    while let Some(decoration) = decorations.get(decoration_index) {
        if let DecorationKind::VirtualText { text, role, .. } = &decoration.kind {
            ctx.draw_text_with_font(
                text,
                Point::new(x, baseline),
                font,
                &solid(decoration_role_color(*role)),
            )?;
            x += ctx
                .measure_text(text, font)
                .map(|metrics| metrics.advance)
                .unwrap_or(text.chars().count() as f32 * char_w);
        }
        decoration_index += 1;
    }

    Ok(())
}

/// Draw a line with per-token colouring. Gaps between spans use Default colour.
fn draw_highlighted(
    ctx: &mut dyn DrawingContext,
    text: &str,
    spans: &[ozone_syntax::TokenSpan],
    x0: f32,
    y: f32,
    char_w: f32,
    font: &Font,
) -> AureaResult<()> {
    let bytes = text.as_bytes();
    let mut last = 0usize;

    for span in spans {
        // Gap before this span
        if span.start > last {
            let seg = &text[last..span.start];
            let sx = x0 + last as f32 * char_w;
            ctx.draw_text_with_font(
                seg,
                Point::new(sx, y),
                font,
                &solid(token_color(TokenKind::Default)),
            )?;
        }

        let end = (span.start + span.len).min(bytes.len());
        let seg = &text[span.start..end];
        let sx = x0 + span.start as f32 * char_w;
        ctx.draw_text_with_font(seg, Point::new(sx, y), font, &solid(token_color(span.kind)))?;

        last = end;
    }

    // Trailing gap
    if last < text.len() {
        let seg = &text[last..];
        let sx = x0 + last as f32 * char_w;
        ctx.draw_text_with_font(
            seg,
            Point::new(sx, y),
            font,
            &solid(token_color(TokenKind::Default)),
        )?;
    }

    Ok(())
}

/// Draw one row of terminal cells: per-cell background fills, then runs of
/// glyphs batched by identical pen (foreground colour + bold) into single text
/// draws. Honours reverse-video by swapping fg/bg.
#[allow(clippy::too_many_arguments)]
fn draw_term_row(
    ctx: &mut dyn DrawingContext,
    row: &[ozone_term::Cell],
    x0: f32,
    line_top: f32,
    baseline: f32,
    line_h: f32,
    char_w: f32,
    font: &Font,
) -> AureaResult<()> {
    use ozone_term::Color as TC;

    // Resolve a cell's effective (fg, optional bg) after applying reverse-video.
    let resolve = |c: &ozone_term::Cell| -> (aurea::render::Color, Option<aurea::render::Color>) {
        if c.inverse {
            // Reverse video: foreground paints the background and vice versa.
            return (
                term_color(c.bg, palette().background),
                Some(term_color(c.fg, palette().foreground)),
            );
        }
        let bg = match c.bg {
            TC::Default => None,
            other => Some(term_color(other, palette().background)),
        };
        (term_color(c.fg, palette().foreground), bg)
    };

    // Background fills first (so glyphs sit on top).
    for (i, cell) in row.iter().enumerate() {
        if let (_, Some(bg)) = resolve(cell) {
            let bx = x0 + i as f32 * char_w;
            ctx.draw_rect(
                Rect::new(bx, line_top + 1.0, char_w + 0.5, line_h - 1.0),
                &solid(bg),
            )?;
        }
    }

    // Glyph runs batched by foreground colour (spaces included; they draw blank).
    let mut i = 0usize;
    while i < row.len() {
        let (fg, _) = resolve(&row[i]);
        let start = i;
        let mut text = String::new();
        while i < row.len() && resolve(&row[i]).0 == fg {
            text.push(row[i].ch);
            i += 1;
        }
        if text.trim_end().is_empty() {
            continue; // run of spaces: nothing to draw
        }
        let sx = x0 + start as f32 * char_w;
        ctx.draw_text_with_font(&text, Point::new(sx, baseline), font, &solid(fg))?;
    }

    Ok(())
}

/// Draw an image centered in `rect`, scaled to fit while preserving aspect
/// ratio (never upscaling past 1:1). Shows a label if the image failed to load.
fn draw_image_pane(
    ctx: &mut dyn DrawingContext,
    rect: Rect,
    image: Option<&Image>,
    font: &Font,
    metrics: TextMetrics,
) -> AureaResult<()> {
    let Some(img) = image else {
        // Decode failed or not ready: centered dim label.
        let msg = "cannot display image";
        let w = ctx
            .measure_text(msg, font)
            .map(|m| m.advance)
            .unwrap_or(msg.len() as f32 * metrics.char_w);
        let bl = rect.y + rect.height / 2.0;
        ctx.draw_text_with_font(
            msg,
            Point::new(rect.x + (rect.width - w) / 2.0, bl),
            font,
            &solid(palette().picker_detail),
        )?;
        return Ok(());
    };
    if img.width == 0 || img.height == 0 {
        return Ok(());
    }

    let pad = 12.0;
    let avail_w = (rect.width - pad * 2.0).max(1.0);
    let avail_h = (rect.height - pad * 2.0).max(1.0);
    let iw = img.width as f32;
    let ih = img.height as f32;
    // Fit; don't upscale beyond native size.
    let scale = (avail_w / iw).min(avail_h / ih).min(1.0);
    let dw = iw * scale;
    let dh = ih * scale;
    let dx = rect.x + (rect.width - dw) / 2.0;
    let dy = rect.y + (rect.height - dh) / 2.0;
    ctx.draw_image_rect(img, Rect::new(dx, dy, dw, dh))?;

    // Dimensions label, bottom-centered.
    let label = format!("{}×{}", img.width, img.height);
    let lw = ctx
        .measure_text(&label, font)
        .map(|m| m.advance)
        .unwrap_or(0.0);
    let ly = (rect.y + rect.height - 6.0).min(rect.y + rect.height);
    ctx.draw_text_with_font(
        &label,
        Point::new(rect.x + (rect.width - lw) / 2.0, ly),
        font,
        &solid(palette().picker_detail),
    )?;
    Ok(())
}

fn draw_status_bar(
    ctx: &mut dyn DrawingContext,
    width: f32,
    height: f32,
    font: &Font,
    ws: &Workspace,
    mods: ActiveMods,
) -> AureaResult<()> {
    let bar_top = height - STATUS_H;
    ctx.draw_rect(
        Rect::new(0.0, bar_top, width, STATUS_H),
        &solid(palette().statusbar_bg),
    )?;
    ctx.draw_line(0.0, bar_top, width, bar_top, &stroke(palette().border, 1.0))?;

    // Emacs-style modeline: the left badge is the buffer's *major mode*, not a
    // generic "EDIT" (Ozone is non-modal). Transient input modes (find, M-x) have
    // their own overlays, so they don't belong here.
    let (mode, file_name, cursor_info, dirty, pane_info) =
        if let (Some(view), Some(buf)) = (ws.active_view(), ws.active_buffer()) {
            let file_name = match &buf.kind {
                BufferKind::File(p) | BufferKind::Image(p) => p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("?")
                    .to_string(),
                BufferKind::Scratch => "*scratch*".to_string(),
                BufferKind::Search => "*files*".to_string(),
                BufferKind::References => "*references*".to_string(),
                BufferKind::Terminal => "*terminal*".to_string(),
            };
            let cursor_info = format!("{}:{}", view.cursor.line + 1, view.cursor.col + 1);
            let dirty = if buf.is_dirty() { "*" } else { "" };
            let mode = match &buf.kind {
                BufferKind::File(p) => major_mode_label(Filetype::from_path(&p.to_string_lossy())),
                BufferKind::Search => "Files",
                BufferKind::References => "Refs",
                BufferKind::Terminal => "Term",
                BufferKind::Image(_) => "Image",
                BufferKind::Scratch => "Text",
            };
            let pane_info = pane_status(ws, view.id);
            (mode, file_name, cursor_info, dirty.to_string(), pane_info)
        } else {
            (
                "",
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            )
        };

    let ascent = ctx
        .measure_text("M", font)
        .map(|m| m.ascent)
        .unwrap_or(font.size * 0.8);
    let descent = ctx
        .measure_text("M", font)
        .map(|m| m.descent)
        .unwrap_or(font.size * 0.2);
    let baseline = baseline_in_rect(bar_top, STATUS_H, ascent, descent);

    let mode_text = format!(" {} ", mode);
    let mode_w = ctx
        .measure_text(&mode_text, font)
        .map(|m| m.advance)
        .unwrap_or(font.size * 4.0);
    ctx.draw_rect(
        Rect::new(8.0, bar_top + 4.0, mode_w + 8.0, STATUS_H - 8.0),
        &solid(palette().status_mode_bg),
    )?;
    ctx.draw_text_with_font(
        &mode_text,
        Point::new(12.0, baseline),
        font,
        &solid(palette().statusbar_fg),
    )?;

    let left = format!("  {}{}    {}", file_name, dirty, cursor_info);
    ctx.draw_text_with_font(
        &left,
        Point::new(16.0 + mode_w, baseline),
        font,
        &solid(palette().statusbar_fg),
    )?;

    // Live modifier indicator, far right: a lit pill per *held* logical modifier.
    let mut x = width - 12.0;
    if mods.any() {
        let labels = [
            ("Shift", mods.shift),
            ("Super", mods.super_),
            ("Meta", mods.meta),
            ("Ctrl", mods.control),
        ];
        for (label, active) in labels {
            if !active {
                continue;
            }
            // Pre-measure so the chip can be placed right-to-left, then draw it
            // with the shared pill component.
            let chip_w = ctx
                .measure_text(label, font)
                .map(|m| m.advance)
                .unwrap_or(label.len() as f32 * font.size * 0.6)
                + 12.0;
            x -= chip_w;
            draw_pill(
                ctx,
                label,
                x,
                bar_top + 4.0,
                STATUS_H - 8.0,
                baseline,
                6.0,
                font,
                palette().status_mode_bg,
                palette().picker_prompt,
            )?;
            x -= 6.0; // gap between pills
        }
    }

    // Encoding / pane info, left of the modifier indicator.
    let right = if pane_info.is_empty() {
        "UTF-8".to_string()
    } else {
        format!("{}  UTF-8", pane_info)
    };
    let right_w = ctx
        .measure_text(&right, font)
        .map(|m| m.advance)
        .unwrap_or(right.len() as f32 * font.size * 0.6);
    let right_x = (x - right_w - 12.0).max(16.0 + mode_w);
    ctx.draw_text_with_font(
        &right,
        Point::new(right_x, baseline),
        font,
        &solid(palette().statusbar_dim),
    )?;

    Ok(())
}

fn draw_cursor(
    ctx: &mut dyn DrawingContext,
    x: f32,
    line_top: f32,
    line_h: f32,
    char_w: f32,
    style: CursorStyle,
) -> AureaResult<()> {
    match style {
        CursorStyle::Bar => {
            ctx.draw_rect(
                Rect::new(x, line_top + 1.0, 2.0, line_h - 1.0),
                &solid(palette().cursor),
            )?;
        }
        CursorStyle::Block => {
            ctx.draw_rect(
                Rect::new(x, line_top + 2.0, char_w.max(6.0), line_h - 3.0),
                &solid(palette().cursor),
            )?;
        }
        CursorStyle::Underline => {
            ctx.draw_rect(
                Rect::new(x, line_top + line_h - 3.0, char_w.max(6.0), 2.0),
                &solid(palette().cursor),
            )?;
        }
    }
    Ok(())
}

/// Emacs-style major-mode label shown in the status badge.
fn major_mode_label(filetype: Filetype) -> &'static str {
    match filetype {
        Filetype::Rust => "Rust",
        Filetype::Toml => "TOML",
        Filetype::Json => "JSON",
        Filetype::Markdown => "Markdown",
        Filetype::Plain => "Text",
    }
}

fn pane_status(ws: &Workspace, active: ViewId) -> String {
    let Some(panes) = &ws.panes else {
        return String::new();
    };
    let leaves = panes.leaves();
    if leaves.len() <= 1 {
        return String::new();
    }
    let Some(idx) = leaves.iter().position(|id| *id == active) else {
        return String::new();
    };
    format!("pane {}/{}", idx + 1, leaves.len())
}
