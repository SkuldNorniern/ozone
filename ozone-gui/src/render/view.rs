use aurea::AureaResult;
use aurea::render::{DrawingContext, Font, Path, PathCommand, Point, Rect};

use ozone_buffer::{BufferKind, Pos};
use ozone_config::{Config, CursorStyle, LineNumbers};
use ozone_editor::{Decoration, DecorationKind, ViewId, VirtualPos, Workspace, fold};
use ozone_syntax::{Filetype, ScanState, TokenKind, scan_line};

use super::TextMetrics;
use super::decorations::{
    decoration_highlight_color, decoration_role_color, sync_bracket_decorations,
};
use super::image::draw_image_pane;
use super::text::{
    draw_highlighted, draw_line_with_inline_virtual_text, line_prefix_end, shift_token_spans,
    wrap_line_segments,
};
use crate::components::pill::draw_pill;
use crate::layout::*;
use crate::terminals::draw_term_row;
use crate::theme::{palette, solid, stroke, token_color};
use crate::{ImageCache, TermCells};

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_view(
    ctx: &mut dyn DrawingContext,
    ws: &mut Workspace,
    config: &Config,
    view_id: ViewId,
    rect: Rect,
    font: &Font,
    metrics: TextMetrics,
    welcome_bindings: &[(String, String)],
    term_cells: &TermCells,
    images: &ImageCache,
    cursor_visible: bool,
) -> AureaResult<()> {
    let Some(buffer_id) = ws.views.get(&view_id).map(|view| view.buffer_id) else {
        return Ok(());
    };
    let Some(line_count) = ws.buffers.get(&buffer_id).map(|buf| buf.line_count()) else {
        return Ok(());
    };

    let is_active_pane = ws.active_view_id == Some(view_id);

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
        if view.scroll_line >= max_scroll_line(line_count, visible) {
            view.scroll_y = 0.0;
        }
    }
    sync_bracket_decorations(ws, view_id);

    let Some(view) = ws.views.get(&view_id) else {
        return Ok(());
    };
    let Some(buf) = ws.buffers.get(&buffer_id) else {
        return Ok(());
    };
    let show_welcome = matches!(buf.kind, BufferKind::Scratch) && buf.text().is_empty();

    ctx.draw_rect(rect, &solid(palette().background))?;

    let ft = match &buf.kind {
        BufferKind::File(p) => Filetype::from_path(&p.to_string_lossy()),
        _ => Filetype::Plain,
    };

    let line_numbers = match buf.kind {
        BufferKind::Terminal
        | BufferKind::Search
        | BufferKind::References
        | BufferKind::FileTree
        | BufferKind::Image(_) => LineNumbers::Off,
        _ => ws
            .buffer_local(buffer_id)
            .and_then(|l| l.line_numbers)
            .unwrap_or(config.editor.line_numbers),
    };
    let word_wrap = !matches!(
        buf.kind,
        BufferKind::Terminal
            | BufferKind::Search
            | BufferKind::References
            | BufferKind::FileTree
            | BufferKind::Image(_)
    ) && ws
        .buffer_local(buffer_id)
        .and_then(|local| local.word_wrap)
        .unwrap_or(config.editor.word_wrap);

    let scroll = view.scroll_line;
    let scroll_y = view.scroll_y;
    let visible = visible + 1;
    let gutter_w = gutter_width(line_count, metrics.char_w, line_numbers);
    let text_x = rect.x + gutter_w + PAD;
    let text_cols = (((rect.x + rect.width - PAD) - text_x) / metrics.char_w)
        .floor()
        .max(1.0) as usize;

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

    if gutter_w > 0.0 {
        ctx.draw_rect(
            Rect::new(rect.x, rect.y, gutter_w, rect.height),
            &solid(palette().gutter),
        )?;
    }

    let mut scan_state = ScanState::clean();
    if !matches!(ft, Filetype::Plain | Filetype::Toml) {
        for text in buf.lines_slice(0, scroll) {
            let (_, ns) = scan_line(ft, &text, scan_state);
            scan_state = ns;
        }
    }

    let visible_line_end = (scroll + visible + 1).min(line_count);
    let visible_texts = buf.lines_slice(scroll, visible_line_end);

    let mut visual_i = 0usize;
    let mut line_idx = scroll;
    while visual_i < visible && line_idx < line_count {
        let line_offset = line_idx - scroll;
        let full_line_text = visible_texts.get(line_offset).cloned().unwrap_or_default();
        let wrap_segments = if word_wrap {
            wrap_line_segments(&full_line_text, text_cols)
        } else {
            vec![(0, line_prefix_end(&full_line_text, text_cols))]
        };
        let line_start = buf.pos_to_offset(Pos::new(line_idx, 0));
        let (spans, new_state) = if term_grid.is_none() {
            scan_line(ft, &full_line_text, scan_state)
        } else {
            (Vec::new(), scan_state)
        };
        scan_state = new_state;

        // Folding: lines inside a collapsed region are not drawn (the syntax
        // scan state above has already advanced past them). The header line
        // itself stays visible and gets a fold marker below.
        if fold::is_hidden(buf, &view.folds, line_idx) {
            line_idx += 1;
            continue;
        }

        for (segment_index, (segment_start, segment_end)) in
            wrap_segments.iter().copied().enumerate()
        {
            if visual_i >= visible {
                break;
            }
            let i = visual_i;
            if line_idx >= line_count {
                break;
            }

            let line_top = content_top - scroll_y + i as f32 * line_h;
            if line_top >= content_top + content_h || line_top >= rect.y + rect.height {
                break;
            }

            let baseline =
                baseline_in_rect(line_top, line_h, metrics.text_ascent, metrics.text_descent);
            let is_cursor = line_idx == view.cursor.line;
            let line_text = &full_line_text[segment_start..segment_end];
            let line_end = line_start + full_line_text.len();
            let segment_abs_start = line_start + segment_start;
            let segment_abs_end = line_start + segment_end;
            let segment_is_line_end =
                segment_index + 1 == wrap_segments.len() && segment_end == full_line_text.len();
            let line_decorations: Vec<&Decoration> = decorations
                .iter()
                .filter(|decoration| {
                    if decoration.start == decoration.end {
                        decoration.start >= segment_abs_start && decoration.start <= segment_abs_end
                    } else {
                        decoration.start < segment_abs_end && decoration.end > segment_abs_start
                    }
                })
                .collect();

            if is_cursor && is_active_pane {
                ctx.draw_rect(
                    Rect::new(rect.x, line_top + 1.0, rect.width, line_h - 1.0),
                    &solid(palette().cursor_line),
                )?;
            }

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
                let start_col = start_col.max(segment_start).min(segment_end);
                let end_col = end_col.max(segment_start).min(segment_end);
                if end_col > start_col {
                    let sx = text_x + (start_col - segment_start) as f32 * metrics.char_w;
                    let sw = (end_col - start_col) as f32 * metrics.char_w;
                    ctx.draw_rect(
                        Rect::new(sx, line_top + 1.0, sw, line_h - 2.0),
                        &solid(palette().selection),
                    )?;
                }
            }

            for decoration in &line_decorations {
                if let DecorationKind::Highlight(role) = &decoration.kind {
                    let start = decoration.start.max(segment_abs_start) - segment_abs_start;
                    let end = decoration.end.min(segment_abs_end) - segment_abs_start;
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

            let gutter_label = if segment_index == 0 {
                match line_numbers {
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
                }
            } else {
                None
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

                // Fold gutter indicator: filled triangle, no font required.
                if gutter_w > 0.0 {
                    let is_folded = view.folds.contains(&line_idx);
                    if is_folded || fold::is_foldable(buf, line_idx) {
                        let color = if is_folded {
                            palette().picker_prompt
                        } else {
                            palette().line_number
                        };
                        let cx = rect.x + 7.0;
                        let cy = line_top + line_h / 2.0;
                        let r = 3.5_f32;
                        let tri = if is_folded {
                            // right-pointing: tip at right
                            Path {
                                commands: vec![
                                    PathCommand::MoveTo(Point::new(cx - r, cy - r)),
                                    PathCommand::LineTo(Point::new(cx + r, cy)),
                                    PathCommand::LineTo(Point::new(cx - r, cy + r)),
                                    PathCommand::Close,
                                ],
                            }
                        } else {
                            // down-pointing: tip at bottom
                            Path {
                                commands: vec![
                                    PathCommand::MoveTo(Point::new(cx - r, cy - r)),
                                    PathCommand::LineTo(Point::new(cx + r, cy - r)),
                                    PathCommand::LineTo(Point::new(cx, cy + r)),
                                    PathCommand::Close,
                                ],
                            }
                        };
                        ctx.draw_path(&tri, &solid(color))?;
                    }
                }
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

            if let Some(grid) = term_grid {
                if let Some(row) = grid.get(line_idx) {
                    let row = &row[..row.len().min(text_cols)];
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
                let inline_virtual: Vec<&Decoration> = line_decorations
                    .iter()
                    .copied()
                    .filter(|decoration| {
                        let anchored_here = decoration.start >= segment_abs_start
                            && decoration.start <= segment_abs_end;
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
                        line_text,
                        &shift_token_spans(&spans, segment_start, segment_end),
                        &inline_virtual,
                        segment_abs_start,
                        text_x,
                        baseline,
                        metrics.char_w,
                        font,
                    )?;
                } else if spans.is_empty() || ft == Filetype::Plain {
                    ctx.draw_text_with_font(
                        line_text,
                        Point::new(text_x, baseline),
                        font,
                        &solid(token_color(TokenKind::Default)),
                    )?;
                } else {
                    draw_highlighted(
                        ctx,
                        line_text,
                        &shift_token_spans(&spans, segment_start, segment_end),
                        text_x,
                        baseline,
                        metrics.char_w,
                        font,
                    )?;
                }
            }

            // Fold badge: rounded rect + 3 drawn dots, no font required.
            if segment_is_line_end && view.folds.contains(&line_idx) {
                let badge_x = text_x + (line_text.len() + 1) as f32 * metrics.char_w;
                let badge_h = (line_h * 0.55).max(8.0);
                let badge_w = badge_h * 1.8;
                let badge_top = line_top + (line_h - badge_h) / 2.0;
                draw_pill(
                    ctx,
                    "",
                    badge_x,
                    badge_top,
                    badge_h,
                    baseline,
                    0.0,
                    font,
                    palette().gutter,
                    palette().line_number_active,
                )?;
                // 3 small circles as dots
                let dot_r = (badge_h * 0.12).max(1.5);
                let dot_y = badge_top + badge_h / 2.0;
                let gap = badge_w / 4.0;
                for i in 1..=3_u8 {
                    ctx.draw_circle(
                        Point::new(badge_x + gap * i as f32, dot_y),
                        dot_r,
                        &solid(palette().line_number_active),
                    )?;
                }
            }

            for decoration in &line_decorations {
                match &decoration.kind {
                    DecorationKind::Underline(role) => {
                        let start = decoration.start.max(segment_abs_start) - segment_abs_start;
                        let end = decoration.end.min(segment_abs_end) - segment_abs_start;
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
                    } if segment_is_line_end
                        && decoration.start >= line_start
                        && decoration.start <= line_end =>
                    {
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

            if cursor_visible
                && is_cursor
                && is_active_pane
                && view.cursor.col >= segment_start
                && (view.cursor.col < segment_end || segment_is_line_end)
            {
                draw_cursor(
                    ctx,
                    text_x + view.cursor.col.saturating_sub(segment_start) as f32 * metrics.char_w,
                    line_top,
                    line_h,
                    metrics.char_w,
                    config.editor.cursor_style,
                )?;
            }
            visual_i += 1;
        }
        line_idx += 1;
    }

    if gutter_w > 0.0 {
        ctx.draw_line(
            rect.x + gutter_w,
            rect.y,
            rect.x + gutter_w,
            rect.y + rect.height,
            &stroke(palette().border, 1.0),
        )?;
    }

    if show_welcome {
        draw_welcome_screen(ctx, rect, font, metrics, text_x, welcome_bindings)?;
    }

    let viewport_lines = (content_h / line_h).max(1.0);
    if (line_count as f32) > viewport_lines {
        let track_h = rect.height;
        let thumb_h = (track_h * viewport_lines / line_count as f32).clamp(24.0, track_h);
        let max_scroll = max_scroll_line(line_count, viewport_lines as usize);
        let t = if max_scroll > 0 {
            ((scroll as f32 + scroll_y / line_h) / max_scroll as f32).clamp(0.0, 1.0)
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

fn draw_welcome_screen(
    ctx: &mut dyn DrawingContext,
    rect: Rect,
    font: &Font,
    metrics: TextMetrics,
    text_x: f32,
    bindings: &[(String, String)],
) -> AureaResult<()> {
    if rect.width < 420.0 || rect.height < 220.0 {
        return Ok(());
    }

    let title_font = Font::new(&font.family, font.size + 9.0);
    let subtitle_font = Font::new(&font.family, font.size + 1.0);
    let title_metrics = ctx.measure_text("Ozone", &title_font).ok();
    let title_w = title_metrics
        .as_ref()
        .map(|m| m.advance)
        .unwrap_or(font.size * 5.0);

    let title_x = (rect.x + rect.width * 0.5 - title_w * 0.5).max(text_x);
    let title_y = rect.y + rect.height * 0.28;
    ctx.draw_text_with_font(
        "Ozone",
        Point::new(title_x, title_y),
        &title_font,
        &solid(palette().foreground),
    )?;

    let subtitle = "scratch buffer";
    let subtitle_w = ctx
        .measure_text(subtitle, &subtitle_font)
        .map(|m| m.advance)
        .unwrap_or(subtitle.len() as f32 * metrics.char_w);
    ctx.draw_text_with_font(
        subtitle,
        Point::new(
            (rect.x + rect.width * 0.5 - subtitle_w * 0.5).max(text_x),
            title_y + 30.0,
        ),
        &subtitle_font,
        &solid(palette().statusbar_dim),
    )?;

    if bindings.is_empty() {
        return Ok(());
    }

    let rows = bindings.iter().take(6).collect::<Vec<_>>();
    let key_cols = rows
        .iter()
        .map(|(key, _)| key.chars().count())
        .max()
        .unwrap_or(0)
        .max(8);
    let command_cols = rows
        .iter()
        .map(|(_, command)| command.chars().count())
        .max()
        .unwrap_or(0)
        .max(14);
    let total_w = (key_cols + command_cols + 4) as f32 * metrics.char_w;
    let start_x = (rect.x + rect.width * 0.5 - total_w * 0.5).max(text_x);
    let row_h = (font.size * 1.65).max(18.0);
    let first_y = title_y + 84.0;

    for (i, (key, command)) in rows.into_iter().enumerate() {
        let y = first_y + i as f32 * row_h;
        ctx.draw_text_with_font(
            key,
            Point::new(start_x, y),
            font,
            &solid(palette().picker_prompt),
        )?;
        ctx.draw_text_with_font(
            command,
            Point::new(start_x + (key_cols as f32 + 4.0) * metrics.char_w, y),
            font,
            &solid(palette().line_number_active),
        )?;
    }

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
