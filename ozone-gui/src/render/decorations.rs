use aurea::render::Color;
use ozone_editor::{
    BRACKET_NAMESPACE, DecorationKind, HlRole, ViewId, Workspace, matching_bracket,
};

use crate::theme::palette;

pub(super) fn decoration_role_color(role: HlRole) -> Color {
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

/// Severity ranking for the gutter dot when a line has multiple diagnostics —
/// the most severe wins (`Error` > `Warn` > `Info` > `Hint`).
pub(super) fn gutter_severity_rank(role: HlRole) -> u8 {
    match role {
        HlRole::Error => 3,
        HlRole::Warn => 2,
        HlRole::Info => 1,
        HlRole::Hint => 0,
        _ => 0,
    }
}

pub(super) fn decoration_highlight_color(role: HlRole) -> Color {
    let color = decoration_role_color(role);
    if role == HlRole::Bracket {
        return color;
    }
    Color::rgba(color.r, color.g, color.b, color.a.min(110))
}

pub(super) fn sync_bracket_decorations(ws: &mut Workspace, view_id: ViewId) {
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
            matches!(decoration.kind, DecorationKind::Highlight(HlRole::Bracket)).then_some((
                buffer,
                decoration.start,
                decoration.end,
            ))
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
