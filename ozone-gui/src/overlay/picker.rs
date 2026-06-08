//! Fuzzy picker overlay, shared by the M-x command palette and the Ctrl+P file
//! picker. `PickerState` holds the query + filtered items; each item carries a
//! `PickerAction` performed on commit (run a command, or open a file).

use aurea::AureaResult;
use aurea::render::{DrawingContext, Point, Rect};
use ozone_buffer::{BufferId, BufferKind};
use ozone_config::Config;
use ozone_editor::{AutocommandRegistry, CommandRegistry, Workspace};

use crate::actions::{dispatch_autocmds, run_cmd, run_cmd_with_arg};
use crate::components::{
    ListRow, centered_rect, draw_field, draw_list, draw_panel, draw_scrim, fill_round_rect, style,
};
use crate::editor_font;
use crate::layout::baseline_in_rect;
use crate::theme::solid;

/// What committing a picker item does.
#[derive(Clone)]
pub(crate) enum PickerAction {
    RunCommand(String),
    /// Run a command with an argument (caller-supplied Select items).
    RunCommandArg(String, Option<String>),
    ApplyTheme(String),
    OpenFile(std::path::PathBuf),
    SwitchBuffer(BufferId),
}

/// One selectable row in the picker.
pub(crate) struct PickerItem {
    /// Primary text shown in the list.
    pub(crate) display: String,
    /// Secondary text (right-aligned, dim); empty to omit.
    pub(crate) detail: String,
    /// Lowercase text the query is fuzzy-matched against.
    pub(crate) haystack: String,
    pub(crate) action: PickerAction,
}

/// Runtime state for the floating fuzzy picker.
pub(crate) struct PickerState {
    /// Prompt shown before the query (e.g. `M-x`, `find file:`).
    prompt: String,
    query: String,
    all: Vec<PickerItem>,
    /// Indices into `all` matching the current query.
    filtered: Vec<usize>,
    selected: usize,
}

impl PickerState {
    pub(crate) fn new(prompt: impl Into<String>, all: Vec<PickerItem>) -> Self {
        let mut s = Self {
            prompt: prompt.into(),
            query: String::new(),
            all,
            filtered: Vec::new(),
            selected: 0,
        };
        s.refilter();
        s
    }

    fn refilter(&mut self) {
        let q = self.query.to_lowercase();
        self.filtered = self
            .all
            .iter()
            .enumerate()
            .filter(|(_, item)| subsequence_match(&item.haystack, &q))
            .map(|(i, _)| i)
            .collect();
        if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len().saturating_sub(1);
        }
    }

    pub(crate) fn push(&mut self, c: char) {
        self.query.push(c);
        self.refilter();
    }

    pub(crate) fn backspace(&mut self) {
        self.query.pop();
        self.refilter();
    }

    pub(crate) fn move_sel(&mut self, delta: isize) {
        if self.filtered.is_empty() {
            return;
        }
        let len = self.filtered.len() as isize;
        let next = (self.selected as isize + delta).rem_euclid(len);
        self.selected = next as usize;
    }

    fn selected_item(&self) -> Option<&PickerItem> {
        self.filtered.get(self.selected).map(|&i| &self.all[i])
    }
}

/// fzf-style subsequence match: every char of `needle` appears in `haystack` in order.
fn subsequence_match(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let mut chars = haystack.chars();
    'outer: for nc in needle.chars() {
        for hc in chars.by_ref() {
            if hc == nc {
                continue 'outer;
            }
        }
        return false;
    }
    true
}

/// All registered commands as picker items (sorted by display name).
pub(crate) fn command_picker_items(reg: &CommandRegistry) -> Vec<PickerItem> {
    let mut v: Vec<PickerItem> = reg
        .names()
        .map(|n| {
            let display = reg.display_name(n);
            let haystack = format!("{} {}", display, n).to_lowercase();
            PickerItem {
                display,
                detail: reg.description(n).unwrap_or("").to_string(),
                haystack,
                action: PickerAction::RunCommand(n.to_string()),
            }
        })
        .collect();
    v.sort_by(|a, b| a.display.cmp(&b.display));
    v
}

/// Workspace files as picker items (relative paths, opened on commit).
pub(crate) fn file_picker_items() -> Vec<PickerItem> {
    let Ok(base) = std::env::current_dir() else {
        return Vec::new();
    };
    ozone_editor::commands::collect_workspace_files(&base, 5000)
        .into_iter()
        .map(|rel| PickerItem {
            haystack: rel.to_lowercase(),
            display: rel.clone(),
            detail: String::new(),
            action: PickerAction::OpenFile(base.join(&rel)),
        })
        .collect()
}

/// Open buffers as picker items, in most-recently-used order (`mru` lists the
/// most recent first). Skips transient picker/reference surfaces. The active
/// buffer is excluded so the picker switches *away* from it.
pub(crate) fn buffer_picker_items(ws: &Workspace, mru: &[BufferId]) -> Vec<PickerItem> {
    let active = ws.active_view().map(|v| v.buffer_id);
    let mut items = Vec::new();
    let push = |id: BufferId, ws: &Workspace, items: &mut Vec<PickerItem>| {
        if Some(id) == active {
            return;
        }
        let Some(buf) = ws.buffers.get(&id) else {
            return;
        };
        let (name, detail) = match &buf.kind {
            BufferKind::File(p) | BufferKind::Image(p) => {
                let name = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("?")
                    .to_string();
                (name, p.to_string_lossy().to_string())
            }
            BufferKind::Scratch => ("*scratch*".to_string(), String::new()),
            BufferKind::Terminal => ("*terminal*".to_string(), String::new()),
            // Transient surfaces are not useful to switch to.
            BufferKind::Search | BufferKind::References | BufferKind::FileTree => return,
        };
        let dirty = if buf.is_dirty() { " ●" } else { "" };
        let display = format!("{name}{dirty}");
        let haystack = format!("{display} {detail}").to_lowercase();
        items.push(PickerItem {
            display,
            detail,
            haystack,
            action: PickerAction::SwitchBuffer(id),
        });
    };

    // MRU order first (most recent that isn't the active buffer), then any
    // remaining open buffers not yet listed.
    for id in mru {
        push(*id, ws, &mut items);
    }
    let listed: std::collections::HashSet<BufferId> = mru.iter().copied().collect();
    let mut rest: Vec<BufferId> = ws
        .buffers
        .keys()
        .copied()
        .filter(|id| !listed.contains(id))
        .collect();
    rest.sort_by_key(|id| id.raw());
    for id in rest {
        push(id, ws, &mut items);
    }
    items
}

/// Build picker items from caller-supplied [`SelectItem`]s (the `Select` intent).
pub(crate) fn select_picker_items(items: Vec<ozone_editor::SelectItem>) -> Vec<PickerItem> {
    items
        .into_iter()
        .map(|it| {
            let haystack = format!("{} {}", it.label, it.detail).to_lowercase();
            PickerItem {
                display: it.label,
                detail: it.detail,
                haystack,
                action: PickerAction::RunCommandArg(it.command, it.arg),
            }
        })
        .collect()
}

pub(crate) fn theme_picker_items() -> Vec<PickerItem> {
    crate::theme::available_themes()
        .into_iter()
        .map(|theme| PickerItem {
            haystack: format!("{} {}", theme.name, theme.id).to_lowercase(),
            display: theme.name,
            detail: theme.id.clone(),
            action: PickerAction::ApplyTheme(theme.id),
        })
        .collect()
}

/// Handle a key while the picker is open. Letters arrive via TextInput;
/// this covers navigation/commit/cancel. Returns whether a redraw is needed.
pub(crate) fn handle_palette_key(
    key: aurea::KeyCode,
    palette: &mut Option<PickerState>,
    ws: &mut Workspace,
    reg: &CommandRegistry,
    autocmds: &AutocommandRegistry,
) -> bool {
    use aurea::KeyCode::*;
    let Some(p) = palette.as_mut() else {
        return false;
    };
    match key {
        Escape => {
            *palette = None;
            true
        }
        Enter => {
            // Clone the chosen action, then close before performing it.
            let action = p.selected_item().map(|item| item.action.clone());
            *palette = None;
            match action {
                Some(PickerAction::RunCommand(cmd)) => run_cmd(&cmd, ws, reg, autocmds),
                Some(PickerAction::RunCommandArg(cmd, arg)) => match arg {
                    Some(arg) => run_cmd_with_arg(&cmd, arg, ws, reg, autocmds),
                    None => run_cmd(&cmd, ws, reg, autocmds),
                },
                Some(PickerAction::ApplyTheme(name)) => {
                    if crate::theme::activate(&name) {
                        // Persist the choice so it survives a restart.
                        crate::theme::persist_theme_name(&name);
                    }
                }
                Some(PickerAction::OpenFile(path)) => {
                    let _ = ws.open_file(path);
                    dispatch_autocmds(ws, reg, autocmds);
                }
                Some(PickerAction::SwitchBuffer(id)) => {
                    ws.switch_active_buffer(id);
                    dispatch_autocmds(ws, reg, autocmds);
                }
                None => {}
            }
            true
        }
        Up => {
            p.move_sel(-1);
            true
        }
        Down => {
            p.move_sel(1);
            true
        }
        Backspace => {
            p.backspace();
            true
        }
        _ => false,
    }
}

/// Render the picker overlay: a centered rounded panel with prompt + result list.
pub(crate) fn draw_palette(
    ctx: &mut dyn DrawingContext,
    p: &PickerState,
    config: &Config,
) -> AureaResult<()> {
    let w = ctx.width() as f32;
    let h = ctx.height() as f32;
    let font = editor_font(config);
    let line_h = (font.size * 1.7).max(18.0);
    let pad = 14.0;
    let radius = 10.0;

    let m = ctx.measure_text("M", &font).ok();
    let ascent = m.as_ref().map(|x| x.ascent).unwrap_or(font.size * 0.8);
    let descent = m.as_ref().map(|x| x.descent).unwrap_or(font.size * 0.2);

    // Dim the editor behind the panel.
    draw_scrim(ctx, w, h)?;

    // Window the visible rows around the selection.
    let max_rows = 12usize;
    let start = if p.selected >= max_rows {
        p.selected + 1 - max_rows
    } else {
        0
    };
    let shown: Vec<usize> = p
        .filtered
        .iter()
        .skip(start)
        .take(max_rows)
        .copied()
        .collect();

    let pw = (w * 0.6).clamp(380.0, 760.0);
    let header_h = line_h + pad * 2.0;
    let body_rows = shown.len().max(1); // reserve a row for "no matches"
    let ph = header_h + body_rows as f32 * line_h + pad;
    let panel = centered_rect(w, h, pw, ph, 20.0);
    let (px, py) = (panel.x, panel.y);

    // Rounded bordered panel.
    let s = style();
    draw_panel(ctx, panel, radius)?;

    // Input box: "<prompt> <query>" with a caret (shared field component).
    let input_rect = Rect::new(px + pad, py + pad, pw - 2.0 * pad, line_h);
    fill_round_rect(ctx, input_rect, 6.0, s.input_bg)?;
    let in_text = Rect::new(
        input_rect.x + 8.0,
        input_rect.y,
        input_rect.width - 16.0,
        input_rect.height,
    );
    draw_field(ctx, in_text, &p.prompt, &p.query, &font, ascent, descent)?;

    // Result list.
    let list_top = py + header_h;
    if shown.is_empty() {
        let bl = baseline_in_rect(list_top, line_h, ascent, descent);
        ctx.draw_text_with_font(
            "no matches",
            Point::new(px + pad + 8.0, bl),
            &font,
            &solid(s.dim),
        )?;
        return Ok(());
    }
    let rows: Vec<ListRow> = shown
        .iter()
        .map(|&idx| ListRow {
            primary: &p.all[idx].display,
            detail: &p.all[idx].detail,
        })
        .collect();
    let sel = p.selected.checked_sub(start);
    draw_list(
        ctx, px, list_top, pw, line_h, pad, &rows, sel, &font, ascent, descent,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<PickerItem> {
        ["buffer.next", "pane.close", "pane.split-right", "file.save"]
            .iter()
            .map(|n| PickerItem {
                display: ozone_editor::commands::pretty_command_name(n),
                detail: String::new(),
                haystack: format!("{} {}", ozone_editor::commands::pretty_command_name(n), n)
                    .to_lowercase(),
                action: PickerAction::RunCommand(n.to_string()),
            })
            .collect()
    }

    fn selected_cmd(p: &PickerState) -> Option<String> {
        match p.selected_item().map(|i| &i.action) {
            Some(PickerAction::RunCommand(c)) => Some(c.clone()),
            _ => None,
        }
    }

    #[test]
    fn subsequence_matches_in_order() {
        assert!(subsequence_match("pane.split-right", "pansplit"));
        assert!(subsequence_match("buffer.next", "bnext"));
        assert!(subsequence_match("anything", ""));
        assert!(!subsequence_match("pane.close", "xyz"));
        assert!(!subsequence_match("abc", "cba"));
    }

    #[test]
    fn picker_filters_and_wraps_selection() {
        let mut p = PickerState::new("M-x", items());
        assert_eq!(p.filtered.len(), 4);
        p.push('p');
        p.push('a');
        assert_eq!(p.filtered.len(), 2);
        assert!(selected_cmd(&p).unwrap().starts_with("pane."));
        p.move_sel(1);
        assert_eq!(p.selected, 1);
        p.move_sel(1);
        assert_eq!(p.selected, 0);
        p.backspace();
        p.backspace();
        assert_eq!(p.filtered.len(), 4);
    }
}
