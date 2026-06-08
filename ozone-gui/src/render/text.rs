use aurea::AureaResult;
use aurea::render::{DrawingContext, Font, Point};

use ozone_editor::{Decoration, DecorationKind};
use ozone_syntax::{TokenKind, TokenSpan};

use crate::theme::{solid, token_color};
use super::decorations::decoration_role_color;

pub(super) fn wrap_line_segments(text: &str, max_cols: usize) -> Vec<(usize, usize)> {
    let max_cols = max_cols.max(1);
    if text.is_empty() {
        return vec![(0, 0)];
    }

    let mut segments = Vec::new();
    let mut start = 0usize;
    while start < text.len() {
        let mut end = start;
        let mut cols = 0usize;
        let mut last_break = None;
        for (offset, ch) in text[start..].char_indices() {
            if cols >= max_cols {
                break;
            }
            let absolute = start + offset;
            end = absolute + ch.len_utf8();
            cols += 1;
            if ch.is_whitespace() {
                last_break = Some(end);
            }
        }

        if end >= text.len() {
            segments.push((start, text.len()));
            break;
        }

        let split = last_break
            .filter(|break_at| *break_at > start)
            .unwrap_or(end);
        segments.push((start, split));
        start = split;
        while start < text.len() {
            let Some(ch) = text[start..].chars().next() else {
                break;
            };
            if !ch.is_whitespace() {
                break;
            }
            start += ch.len_utf8();
        }
    }

    if segments.is_empty() {
        segments.push((0, 0));
    }
    segments
}

pub(super) fn line_prefix_end(text: &str, max_cols: usize) -> usize {
    if max_cols == 0 {
        return 0;
    }
    text.char_indices()
        .nth(max_cols)
        .map(|(offset, _)| offset)
        .unwrap_or(text.len())
}

pub(super) fn shift_token_spans(
    spans: &[TokenSpan],
    segment_start: usize,
    segment_end: usize,
) -> Vec<TokenSpan> {
    spans
        .iter()
        .filter_map(|span| {
            let start = span.start.max(segment_start);
            let end = (span.start + span.len).min(segment_end);
            (end > start).then_some(TokenSpan {
                start: start - segment_start,
                len: end - start,
                kind: span.kind,
            })
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_line_with_inline_virtual_text(
    ctx: &mut dyn DrawingContext,
    text: &str,
    spans: &[TokenSpan],
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
        x += char_w;
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
pub(super) fn draw_highlighted(
    ctx: &mut dyn DrawingContext,
    text: &str,
    spans: &[TokenSpan],
    x0: f32,
    y: f32,
    char_w: f32,
    font: &Font,
) -> AureaResult<()> {
    let bytes = text.as_bytes();
    let mut last = 0usize;

    for span in spans {
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

#[cfg(test)]
mod tests {
    use super::*;
    use ozone_syntax::TokenSpan;

    #[test]
    fn wrap_segments_prefer_whitespace_and_skip_leading_wrap_space() {
        let text = "alpha beta gamma";
        let segments: Vec<_> = wrap_line_segments(text, 8)
            .into_iter()
            .map(|(start, end)| &text[start..end])
            .collect();
        assert_eq!(segments, vec!["alpha ", "beta ", "gamma"]);
    }

    #[test]
    fn wrap_segments_split_long_words_on_char_boundaries() {
        let text = "abcλδε";
        let segments: Vec<_> = wrap_line_segments(text, 4)
            .into_iter()
            .map(|(start, end)| &text[start..end])
            .collect();
        assert_eq!(segments, vec!["abcλ", "δε"]);
    }

    #[test]
    fn shifted_token_spans_are_segment_relative() {
        let spans = vec![TokenSpan {
            start: 2,
            len: 6,
            kind: ozone_syntax::TokenKind::String,
        }];
        let shifted = shift_token_spans(&spans, 4, 10);
        assert_eq!(shifted.len(), 1);
        assert_eq!(shifted[0].start, 0);
        assert_eq!(shifted[0].len, 4);
    }

    #[test]
    fn line_prefix_end_clips_on_char_boundaries() {
        let text = "abλδε";
        assert_eq!(&text[..line_prefix_end(text, 3)], "abλ");
        assert_eq!(&text[..line_prefix_end(text, 99)], text);
        assert_eq!(line_prefix_end(text, 0), 0);
    }
}
