//! The explorer uses Aurea's list, scrolling, disclosure and icon widgets.
//! Workspace commands still own file opening and asynchronous folder refresh.

use std::cell::RefCell;

use aurea::render::{Color, DrawingContext, Font, Icon, Rect};
use aurea::ui::{
    Button, Component, Disclosure, DisclosureStyle, EmptyState, IconMark, InputResponse, Label,
    ListView, ScrollStyle, StateColors, Surface, TextSize, Theme, UiState,
};
use aurea::{AureaResult, KeyCode, MouseButton, WindowEvent};
use ozone_buffer::Pos;
use ozone_config::Config;
use ozone_editor::commands::tree_row_parts;
use ozone_editor::{ViewId, Workspace};
use taste::detect_path;

use crate::layout::{STATUS_H, pane_at};
use crate::theme::{palette, solid};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Id {
    List(ViewId),
    Folder(ViewId, usize),
    Refresh(ViewId),
}

std::thread_local! {
    static UI: RefCell<UiState<Id>> = RefCell::new(UiState::new());
    static ACTIVATIONS: RefCell<Vec<(ViewId, usize)>> = const { RefCell::new(Vec::new()) };
}

pub(crate) fn take_activations() -> Vec<(ViewId, usize)> {
    ACTIVATIONS.with(|actions| std::mem::take(&mut *actions.borrow_mut()))
}

pub(crate) fn wants_frame() -> bool {
    UI.with(|ui| ui.borrow().wants_frame())
}

/// Pointer input goes through the same hit rectangles Aurea painted. Editor
/// shortcuts and Left/Right stay with Ozone's command/keymap layer.
pub(crate) fn handle_event(
    event: &WindowEvent,
    ws: &mut Workspace,
    width: f32,
    height: f32,
) -> InputResponse {
    let Some(tree) = ws.file_tree_view() else {
        UI.with(|ui| *ui.borrow_mut() = UiState::new());
        return InputResponse::default();
    };
    let over_tree = match event {
        WindowEvent::MouseMove { x, y, .. } | WindowEvent::MouseButton { x, y, .. } => {
            let rect = Rect::new(0.0, 0.0, width, (height - STATUS_H).max(0.0));
            ws.panes
                .as_ref()
                .and_then(|p| pane_at(p, rect, *x as f32, *y as f32))
                .is_some_and(|(id, _)| id == tree)
        }
        _ => false,
    };
    match event {
        WindowEvent::KeyInput { key, modifiers, .. } => {
            if ws.active_view_id != Some(tree)
                || modifiers.ctrl
                || modifiers.alt
                || modifiers.meta
                || !matches!(
                    key,
                    KeyCode::Up
                        | KeyCode::Down
                        | KeyCode::Home
                        | KeyCode::End
                        | KeyCode::PageUp
                        | KeyCode::PageDown
                        | KeyCode::Enter
                )
            {
                return InputResponse::default();
            }
        }
        WindowEvent::MouseButton {
            button: MouseButton::Left,
            pressed: true,
            x,
            y,
            ..
        } => {
            let rect = Rect::new(0.0, 0.0, width, (height - STATUS_H).max(0.0));
            if ws
                .panes
                .as_ref()
                .and_then(|panes| pane_at(panes, rect, *x as f32, *y as f32))
                .map(|(view, _)| view)
                == Some(tree)
            {
                ws.focus_file_tree();
            }
        }
        WindowEvent::MouseMove { .. }
        | WindowEvent::MouseButton { .. }
        | WindowEvent::MouseWheel { .. }
        | WindowEvent::Unfocused => {}
        _ => return InputResponse::default(),
    }
    UI.with(|ui| {
        let mut ui = ui.borrow_mut();
        if matches!(event, WindowEvent::KeyInput { .. }) {
            ui.focus(Id::List(tree), 1);
        }
        let mut input = ui.handle_input(event);
        input.consumed |= over_tree;
        input.redraw |= over_tree && matches!(event, WindowEvent::MouseButton { .. });
        input
    })
}

pub(crate) fn draw(
    ctx: &mut dyn DrawingContext,
    ws: &mut Workspace,
    view_id: ViewId,
    rect: Rect,
    font: &Font,
    config: &Config,
) -> AureaResult<()> {
    let p = palette();
    ctx.draw_rect(rect, &solid(p.gutter))?;
    let view = &ws.views[&view_id];
    let buffer = &ws.buffers[&view.buffer_id];
    let count = buffer.line_count().saturating_sub(2);
    let row_height = (font.size * config.editor.line_height).max(24.0);
    let header_height = row_height + 12.0;
    let header = Rect::new(
        rect.x + 10.0,
        rect.y + 6.0,
        (rect.width - 20.0).max(0.0),
        row_height,
    );
    let body = Rect::new(
        rect.x + 4.0,
        rect.y + header_height,
        (rect.width - 8.0).max(0.0),
        (rect.height - header_height - 4.0).max(0.0),
    );
    let mut offset = view.scroll_line as f32 * row_height + view.scroll_y;
    let mut selected = (count > 0).then_some(
        view.cursor
            .line
            .saturating_sub(1)
            .min(count.saturating_sub(1)),
    );
    let mut clicked_folder = None;
    let mut refresh = false;
    let response = UI.with(|state| {
        let mut state = state.borrow_mut();
        state.set_reduced_motion(true);
        let mut theme = Theme::dark();
        theme.font_family = font.family.clone();
        theme.text_size = font.size;
        theme.palette.text = p.picker_fg;
        theme.palette.dim = p.picker_detail;
        theme.palette.focus = p.active_pane_border;
        theme.palette.surface = p.gutter;
        theme.palette.raised = p.picker_input_bg;
        theme.palette.hover = p.picker_selection;
        theme.palette.pressed = p.picker_selection;
        theme.palette.accent = p.picker_prompt;
        let mut ui = state.frame(ctx, &theme);
        let title = buffer.line(0).unwrap_or_default();
        let title = title.trim_start_matches('▸').trim();
        ui.clipped(header, |ui| {
            let (button, title_rect) = header.split_right(62.0_f32.min(header.width), 4.0);
            if title_rect.width > 0.0 {
                Label::new(title, p.picker_prompt)
                    .font(font)
                    .paint(ui, title_rect)?;
            }
            refresh = Button::new("Refresh")
                .quiet()
                .font(&Font::new(&font.family, font.size * 0.8))
                .show(ui, Id::Refresh(view_id), button)?
                .clicked;
            Ok(())
        })?;
        if count == 0 {
            return ui.clipped(body, |ui| {
                EmptyState::new("No files")
                    .help("This folder has no visible files.")
                    .show(ui, Id::List(view_id), body)
            });
        }
        ListView::new(&mut offset, count, row_height)
            .selected(&mut selected)
            .style(ScrollStyle {
                smooth: false,
                step: row_height * 3.0,
                thumb: p.statusbar_dim,
                thumb_hover: p.picker_detail,
                ..ScrollStyle::new(&theme)
            })
            .show(&mut ui, Id::List(view_id), body, |ui, row| {
                let Some(line) = buffer.line(row.index + 1) else {
                    return Ok(());
                };
                let Some((_, path)) = tree_row_parts(&line) else {
                    return Label::new(&line, p.picker_detail)
                        .font(font)
                        .paint(ui, row.rect.inset_xy(8.0, 0.0));
                };
                let directory = path.ends_with('/');
                let path = path.trim_end_matches('/');
                let depth = path.matches('/').count();
                let name = path.rsplit('/').next().unwrap_or(path);
                let background = if row.picked {
                    p.picker_selection
                } else if row.hovered {
                    p.picker_input_bg
                } else {
                    p.gutter
                };
                Surface::new(background).radius(4.0).paint(ui, row.rect)?;
                let indent = (depth as f32 * 16.0).min((row.rect.width - 48.0).max(0.0));
                let marker = Rect::new(row.rect.x + indent, row.rect.y, row_height, row_height);
                if directory {
                    let mut open = !ws.tree_collapsed.contains(path);
                    let style = DisclosureStyle {
                        face: StateColors::new(background, p.picker_input_bg, p.picker_selection),
                        text: p.picker_fg,
                        chevron: p.picker_detail,
                        radius: 4.0,
                        text_size: TextSize::Body,
                    };
                    if Disclosure::new("", &mut open)
                        .style(style)
                        .show(ui, Id::Folder(view_id, row.index), marker)?
                        .changed
                    {
                        clicked_folder = Some(row.index);
                    }
                }
                let icon = Rect::new(
                    marker.right(),
                    row.rect.y + 4.0,
                    row_height - 8.0,
                    row_height - 8.0,
                );
                let color = if directory {
                    p.picker_prompt
                } else {
                    file_type_color(name, p.picker_fg)
                };
                IconMark::new(if directory { Icon::Folder } else { Icon::File }, color)
                    .paint(ui, icon)?;
                let label = Rect::new(
                    icon.right() + 6.0,
                    row.rect.y,
                    (row.rect.right() - icon.right() - 12.0).max(0.0),
                    row_height,
                );
                if label.width > 0.0 {
                    Label::new(name, p.picker_fg).font(font).paint(ui, label)?;
                }
                Ok(())
            })
    })?;
    if let Some(row) = clicked_folder {
        selected = Some(row);
    }
    if let Some(view) = ws.views.get_mut(&view_id) {
        view.cursor = Pos::new(selected.map_or(0, |row| row + 1), 0);
        view.clear_selection();
        view.col_memory = 0;
        view.page_height = ((body.height / row_height) as usize).max(1);
        view.scroll_line = (offset / row_height) as usize;
        view.scroll_y = offset % row_height;
    }
    if (response.activated || clicked_folder.is_some())
        && let Some(row) = selected
    {
        ACTIVATIONS.with(|actions| actions.borrow_mut().push((view_id, row + 1)));
    }
    if refresh {
        ws.request_ui(ozone_editor::UiIntent::FileTree {
            collapsed: ws.tree_collapsed.clone(),
        });
    }
    if ws.active_view_id == Some(view_id) {
        ctx.draw_rect(
            Rect::new(rect.x, rect.y, rect.width, 2.0),
            &solid(p.active_pane_border),
        )?;
    }
    Ok(())
}

/// Uses Taste's language metadata for file icon colors; `dim` is the fallback.
fn file_type_color(name: &str, dim: Color) -> Color {
    detect_path(name)
        .and_then(|detection| detection.language.color())
        .map(|color| Color::rgb(color.r, color.g, color.b))
        .unwrap_or(dim)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aurea::render::{CpuRasterizer, Renderer};
    use aurea::{Modifiers, MouseButtons};

    const WIDTH: u32 = 1000;
    const HEIGHT: u32 = 420;

    struct Explorer {
        ws: Workspace,
        tree: ViewId,
        raster: CpuRasterizer,
        config: Config,
    }

    impl Explorer {
        fn new(rows: usize) -> Self {
            UI.with(|ui| *ui.borrow_mut() = UiState::new());
            take_activations();
            let mut ws = Workspace::new();
            let mut content = "▸ workspace/\n├── ▾ src/\tsrc/\n".to_string();
            for i in 0..rows {
                content.push_str(&format!("│   ├── file-{i}.rs\tsrc/file-{i}.rs\n"));
            }
            let tree = ws.show_file_tree(content);
            let mut result = Self {
                ws,
                tree,
                raster: CpuRasterizer::new(WIDTH, HEIGHT),
                config: Config::default(),
            };
            result.draw();
            result
        }

        fn draw(&mut self) {
            let outer = Rect::new(0.0, 0.0, WIDTH as f32, HEIGHT as f32 - STATUS_H);
            let rect =
                crate::layout::pane_rect(self.ws.panes.as_ref().expect("layout"), outer, self.tree)
                    .expect("tree rect");
            let mut ctx = self.raster.begin_frame().expect("frame");
            draw(
                &mut *ctx,
                &mut self.ws,
                self.tree,
                rect,
                &crate::editor_font(&self.config),
                &self.config,
            )
            .expect("draw explorer");
            drop(ctx);
            self.raster.end_frame().expect("end");
        }

        fn event(&mut self, event: WindowEvent) -> InputResponse {
            let response = handle_event(&event, &mut self.ws, WIDTH as f32, HEIGHT as f32);
            self.draw();
            response
        }

        fn key(&mut self, key: KeyCode) {
            assert!(
                self.event(WindowEvent::KeyInput {
                    key,
                    pressed: true,
                    modifiers: Modifiers::default()
                })
                .consumed
            );
        }

        fn click(&mut self, x: f64, y: f64, clicks: u8) {
            for pressed in [true, false] {
                assert!(
                    self.event(WindowEvent::MouseButton {
                        button: MouseButton::Left,
                        pressed,
                        modifiers: Modifiers::default(),
                        x,
                        y,
                        click_count: clicks,
                        time: Default::default(),
                    })
                    .consumed
                );
            }
        }

        fn row_center(&self, index: usize) -> f64 {
            let height = (self.config.editor.font_size * self.config.editor.line_height).max(24.0);
            f64::from(height + 12.0 + (index as f32 + 0.5) * height)
        }
    }

    #[test]
    fn keyboard_selects_scrolls_and_activates_without_editing_buffer_text() {
        let mut explorer = Explorer::new(100);
        let buffer = explorer.ws.views[&explorer.tree].buffer_id;
        let content = explorer.ws.buffers[&buffer].text();
        explorer.key(KeyCode::Down);
        assert_eq!(explorer.ws.views[&explorer.tree].cursor.line, 2);
        explorer.key(KeyCode::End);
        assert_eq!(explorer.ws.views[&explorer.tree].cursor.line, 101);
        assert!(explorer.ws.views[&explorer.tree].scroll_line > 0);
        explorer.key(KeyCode::Enter);
        assert_eq!(take_activations(), vec![(explorer.tree, 101)]);
        assert_eq!(explorer.ws.buffers[&buffer].text(), content);
    }

    #[test]
    fn disclosure_click_toggles_once_and_file_clicks_select_before_activation() {
        let mut explorer = Explorer::new(10);
        explorer.click(16.0, explorer.row_center(0), 1);
        assert_eq!(take_activations(), vec![(explorer.tree, 1)]);
        explorer.click(110.0, explorer.row_center(2), 1);
        assert_eq!(explorer.ws.views[&explorer.tree].cursor.line, 3);
        assert!(take_activations().is_empty());
        explorer.click(110.0, explorer.row_center(2), 2);
        assert_eq!(take_activations(), vec![(explorer.tree, 3)]);
    }

    #[test]
    fn refresh_button_queues_a_background_request() {
        let mut explorer = Explorer::new(10);
        explorer.click(190.0, 18.0, 1);
        assert!(matches!(
            explorer.ws.drain_ui_intents().as_slice(),
            [ozone_editor::UiIntent::FileTree { .. }]
        ));
        assert!(take_activations().is_empty());
    }

    #[test]
    fn empty_explorer_is_clipped_even_in_a_very_narrow_pane() {
        let mut explorer = Explorer::new(0);
        explorer.ws.show_file_tree("▸ empty/\n".into());
        for width in [1.0, 30.0, 120.0] {
            let mut raster = CpuRasterizer::new(200, 300);
            {
                let mut ctx = raster.begin_frame().expect("frame");
                draw(
                    &mut *ctx,
                    &mut explorer.ws,
                    explorer.tree,
                    Rect::new(0.0, 0.0, width, 300.0),
                    &crate::editor_font(&explorer.config),
                    &explorer.config,
                )
                .expect("empty explorer");
            }
            raster.end_frame().expect("end");
            assert!(
                raster
                    .pixels()
                    .iter()
                    .enumerate()
                    .all(|(i, &pixel)| i % 200 < width as usize || pixel == 0)
            );
        }
    }

    #[test]
    fn wheel_scrolls_only_the_explorer_and_ink_stays_inside_its_pane() {
        let mut explorer = Explorer::new(200);
        explorer.event(WindowEvent::MouseMove {
            x: 120.0,
            y: 220.0,
            buttons: MouseButtons::default(),
            modifiers: Modifiers::default(),
            time: Default::default(),
        });
        assert!(
            explorer
                .event(WindowEvent::MouseWheel {
                    delta_x: 0.0,
                    delta_y: -3.0,
                    modifiers: Modifiers::default(),
                })
                .consumed
        );
        assert!(explorer.ws.views[&explorer.tree].scroll_line > 0);
        let outer = Rect::new(0.0, 0.0, WIDTH as f32, HEIGHT as f32 - STATUS_H);
        let rect = crate::layout::pane_rect(
            explorer.ws.panes.as_ref().expect("layout"),
            outer,
            explorer.tree,
        )
        .expect("tree rect");
        for (i, &pixel) in explorer.raster.pixels().iter().enumerate() {
            if i % WIDTH as usize >= rect.right().ceil() as usize
                || i / WIDTH as usize >= rect.bottom().ceil() as usize
            {
                assert_eq!(pixel, 0, "tree painted outside its pane at {i}");
            }
        }
        explorer.ws.focus_tree_editor();
        let before = explorer.ws.views[&explorer.tree].cursor;
        assert!(
            !handle_event(
                &WindowEvent::KeyInput {
                    key: KeyCode::Down,
                    pressed: true,
                    modifiers: Modifiers::default(),
                },
                &mut explorer.ws,
                WIDTH as f32,
                HEIGHT as f32
            )
            .consumed
        );
        explorer.draw();
        assert_eq!(explorer.ws.views[&explorer.tree].cursor, before);
    }

    #[test]
    fn renders_the_explorer_beside_the_editor() {
        let mut explorer = Explorer::new(0);
        explorer.ws.show_file_tree(concat!(
            "▸ ozone/\n",
            "├── ▾ ozone-gui/\tozone-gui/\n",
            "│   ├── ▾ src/\tozone-gui/src/\n",
            "│   │   ├── app.rs\tozone-gui/src/app.rs\n",
            "│   │   ├── tree.rs\tozone-gui/src/tree.rs\n",
            "│   │   └── a-very-long-filename-that-must-be-clipped.rs\tozone-gui/src/a-very-long-filename-that-must-be-clipped.rs\n",
            "│   └── Cargo.toml\tozone-gui/Cargo.toml\n",
            "├── ▸ ozone-editor/\tozone-editor/\n",
            "├── Cargo.toml\tCargo.toml\n",
            "└── README.md\tREADME.md\n",
        ).into());
        explorer.ws.tree_collapsed.insert("ozone-editor".into());
        explorer.ws.focus_tree_editor();
        explorer.ws.active_buffer_mut().expect("scratch").set_text(
            "// The explorer stays beside your editor.\n\nfn main() {\n    println!(\"Hello, Ozone!\");\n}\n",
        );
        {
            let mut ctx = explorer.raster.begin_frame().expect("frame");
            crate::render::draw_editor(
                &mut *ctx,
                &mut explorer.ws,
                &explorer.config,
                &[],
                None,
                &crate::TermCells::new(),
                &crate::ImageCache::new(),
                &mut crate::SyntaxCache::new(),
                crate::input::ActiveMods::default(),
                true,
                crate::lsp::LspStatus::Idle,
                None,
                &mut 0.0,
            )
            .expect("editor frame");
        }
        explorer.raster.end_frame().expect("end");
        assert!(explorer.ws.file_tree_view().is_some());
        if let Ok(path) = std::env::var("OZONE_TREE_PREVIEW") {
            let rgba: Vec<u8> = explorer
                .raster
                .pixels()
                .iter()
                .flat_map(|&pixel| {
                    [
                        (pixel >> 16) as u8,
                        (pixel >> 8) as u8,
                        pixel as u8,
                        (pixel >> 24) as u8,
                    ]
                })
                .collect();
            image::save_buffer(path, &rgba, WIDTH, HEIGHT, image::ColorType::Rgba8)
                .expect("preview");
        }
    }

    #[test]
    fn file_type_color_uses_taste_metadata_and_fallback() {
        let dim = Color::rgb(1, 2, 3);
        assert_eq!(file_type_color("main.rs", dim), Color::rgb(222, 165, 132));
        assert_eq!(file_type_color("unknown.nope", dim), dim);
    }
}
