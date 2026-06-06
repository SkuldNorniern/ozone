use std::collections::HashMap;
use std::os::raw::c_void;
use std::sync::{Arc, Mutex};

use ozone_term::Terminal;

/// A live terminal: the shell process plus the locally-echoed input line.
struct TermSession {
    term: Terminal,
    input: String,
}

use aurea::render::{Canvas, Color, DrawingContext, Font, Paint, PaintStyle, Point, Rect, RendererBackend};
use aurea::{AureaResult, Element, Window, WindowEvent};
use ozone_buffer::{BufferId, BufferKind};
use ozone_editor::{AutocommandRegistry, CommandContext, CommandRegistry, EditorEvent, IndentConfig, Key, KeyStroke, Keymap, KeymapOutcome, ModifierMap, PhysicalMods, PaneTree, SplitAxis, ViewId, Workspace, matching_bracket};
use ozone_editor::commands::register_defaults;
use ozone_syntax::{Filetype, ScanState, TokenKind, scan_line};
use ozone_config::{Config, CursorStyle, LineNumbers};

// ---------------------------------------------------------------------------
// SendableCanvas + SharedCanvas wrappers
// ---------------------------------------------------------------------------

struct SendableCanvas(Canvas);
unsafe impl Send for SendableCanvas {}
unsafe impl Sync for SendableCanvas {}
impl std::ops::Deref for SendableCanvas {
    type Target = Canvas;
    fn deref(&self) -> &Self::Target { &self.0 }
}
impl std::ops::DerefMut for SendableCanvas {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
}
impl Element for SendableCanvas {
    fn handle(&self) -> *mut c_void { self.0.handle() }
    unsafe fn invalidate_platform(&self, rect: Option<aurea::render::Rect>) {
        unsafe { Element::invalidate_platform(&self.0, rect) }
    }
}

struct SharedCanvas(Arc<Mutex<SendableCanvas>>);
impl Element for SharedCanvas {
    fn handle(&self) -> *mut c_void { self.0.lock().unwrap().handle() }
    unsafe fn invalidate_platform(&self, rect: Option<aurea::render::Rect>) {
        let g = self.0.lock().unwrap();
        unsafe { Element::invalidate_platform(&*g, rect) }
    }
}

// ---------------------------------------------------------------------------
// Fuzzy picker overlay (shared by M-x command palette and Ctrl+P file picker)
// ---------------------------------------------------------------------------

/// What committing a picker item does.
enum PickerAction {
    RunCommand(String),
    OpenFile(std::path::PathBuf),
}

/// One selectable row in the picker.
struct PickerItem {
    /// Primary text shown in the list.
    display: String,
    /// Secondary text (right-aligned, dim); empty to omit.
    detail: String,
    /// Lowercase text the query is fuzzy-matched against.
    haystack: String,
    action: PickerAction,
}

/// Runtime state for the floating fuzzy picker.
struct PickerState {
    /// Prompt shown before the query (e.g. `M-x`, `find file:`).
    prompt: String,
    query: String,
    all: Vec<PickerItem>,
    /// Indices into `all` matching the current query.
    filtered: Vec<usize>,
    selected: usize,
}

impl PickerState {
    fn new(prompt: impl Into<String>, all: Vec<PickerItem>) -> Self {
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

    fn push(&mut self, c: char) {
        self.query.push(c);
        self.refilter();
    }

    fn backspace(&mut self) {
        self.query.pop();
        self.refilter();
    }

    fn move_sel(&mut self, delta: isize) {
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

#[cfg(test)]
mod palette_tests {
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
        assert!(subsequence_match("anything", "")); // empty matches all
        assert!(!subsequence_match("pane.close", "xyz"));
        assert!(!subsequence_match("abc", "cba")); // order matters
    }

    #[test]
    fn picker_filters_and_wraps_selection() {
        let mut p = PickerState::new("M-x", items());
        assert_eq!(p.filtered.len(), 4);
        p.push('p');
        p.push('a');
        // "pa" matches the two pane.* commands
        assert_eq!(p.filtered.len(), 2);
        assert!(selected_cmd(&p).unwrap().starts_with("pane."));
        p.move_sel(1);
        assert_eq!(p.selected, 1);
        p.move_sel(1); // wraps back to 0 (2 items)
        assert_eq!(p.selected, 0);
        p.backspace();
        p.backspace();
        assert_eq!(p.filtered.len(), 4);
    }
}

/// All registered commands as picker items (sorted by display name).
fn command_picker_items(reg: &CommandRegistry) -> Vec<PickerItem> {
    let mut v: Vec<PickerItem> = reg
        .names()
        .map(|n| {
            let display = reg.display_name(n);
            // Match against display name and raw id.
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
fn file_picker_items() -> Vec<PickerItem> {
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

// ---------------------------------------------------------------------------
// In-buffer search (Ctrl+F) state
// ---------------------------------------------------------------------------

struct SearchState {
    query: String,
    /// Byte offsets of matches in the active buffer.
    matches: Vec<usize>,
    current: usize,
    case_sensitive: bool,
}

impl SearchState {
    fn new(case_sensitive: bool) -> Self {
        Self { query: String::new(), matches: Vec::new(), current: 0, case_sensitive }
    }
    fn current_offset(&self) -> Option<usize> {
        self.matches.get(self.current).copied()
    }
    fn next(&mut self) {
        if !self.matches.is_empty() {
            self.current = (self.current + 1) % self.matches.len();
        }
    }
    fn prev(&mut self) {
        if !self.matches.is_empty() {
            self.current = (self.current + self.matches.len() - 1) % self.matches.len();
        }
    }
}

/// Recompute matches for the active buffer from the current query.
fn search_recompute(s: &mut SearchState, ws: &Workspace) {
    let text = ws.active_buffer().map(|b| b.text()).unwrap_or_default();
    s.matches = ozone_editor::find_matches(&text, &s.query, s.case_sensitive);
    if s.current >= s.matches.len() {
        s.current = 0;
    }
}

/// Point `current` at the first match at/after the cursor (wrapping).
fn search_select_from_cursor(s: &mut SearchState, ws: &Workspace) {
    let from = ws
        .active_view()
        .and_then(|v| ws.buffers.get(&v.buffer_id).map(|b| b.pos_to_offset(v.cursor)))
        .unwrap_or(0);
    if let Some(i) = ozone_editor::search::first_match_from(&s.matches, from) {
        s.current = i;
    }
}

/// Move the cursor to the current match and scroll it into view.
fn search_jump(s: &SearchState, ws: &mut Workspace) {
    let Some(off) = s.current_offset() else { return };
    let pos = ws.active_buffer().map(|b| b.offset_to_pos(off));
    if let (Some(pos), Some(view)) = (pos, ws.active_view_mut()) {
        view.cursor = pos;
        view.col_memory = pos.col;
        view.scroll_to_cursor(view.page_height.max(1));
    }
}

/// Handle a key while search is active. Returns whether a redraw is needed.
fn handle_search_key(key: aurea::KeyCode, search: &mut Option<SearchState>, ws: &mut Workspace) -> bool {
    use aurea::KeyCode::*;
    let Some(s) = search.as_mut() else { return false };
    match key {
        Escape => {
            *search = None;
            true
        }
        Enter | Down => {
            s.next();
            search_jump(s, ws);
            true
        }
        Up => {
            s.prev();
            search_jump(s, ws);
            true
        }
        Backspace => {
            s.query.pop();
            search_recompute(s, ws);
            search_select_from_cursor(s, ws);
            search_jump(s, ws);
            true
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub struct OzoneGui {
    workspace: Arc<Mutex<Workspace>>,
    commands: Arc<CommandRegistry>,
    config: Arc<Config>,
    autocmds: Arc<AutocommandRegistry>,
    keymap: Arc<Keymap>,
    modmap: ModifierMap,
}

impl OzoneGui {
    pub fn new(workspace: Workspace) -> Self {
        Self::with_config(workspace, Config::default_config())
    }

    pub fn with_config(mut workspace: Workspace, config: Config) -> Self {
        // Editing uses the configured indentation.
        workspace.indent = IndentConfig {
            width: config.editor.tab_width,
            soft_tabs: config.editor.soft_tabs,
        };

        let mut reg = CommandRegistry::new();
        register_defaults(&mut reg);
        let autocmds = AutocommandRegistry::from_config(&config.autocmds);
        dispatch_autocmds(&mut workspace, &reg, &autocmds);

        // Layered keymap: shipped defaults, then the user's [[keymap]] on top.
        let mut keymap = Keymap::with_defaults();
        keymap.add_user_config(&config.keymaps);

        // Logical→physical modifier map: platform default + [modifiers] overrides.
        let modmap = ModifierMap::platform_default().with_overrides(
            config.modifiers.control.as_deref(),
            config.modifiers.meta.as_deref(),
            config.modifiers.super_.as_deref(),
        );

        Self {
            workspace: Arc::new(Mutex::new(workspace)),
            commands: Arc::new(reg),
            config: Arc::new(config),
            autocmds: Arc::new(autocmds),
            keymap: Arc::new(keymap),
            modmap,
        }
    }

    pub fn run(self) -> AureaResult<()> {
        const W: u32 = 1280;
        const H: u32 = 800;

        let mut window = Window::new("Ozone", W as i32, H as i32)?;

        // Overlay states shared with the draw callback.
        let palette: Arc<Mutex<Option<PickerState>>> = Arc::new(Mutex::new(None));
        let search: Arc<Mutex<Option<SearchState>>> = Arc::new(Mutex::new(None));

        let raw_canvas = Canvas::new(W, H, RendererBackend::Cpu)?;
        let workspace_for_draw = self.workspace.clone();
        let config_for_draw = self.config.clone();
        let palette_for_draw = palette.clone();
        let search_for_draw = search.clone();

        raw_canvas.set_draw_callback(move |ctx| {
            {
                let mut ws = workspace_for_draw.lock().unwrap();
                let srch = search_for_draw.lock().unwrap();
                draw_editor(ctx, &mut ws, &config_for_draw, srch.as_ref())?;
            }
            if let Some(p) = palette_for_draw.lock().unwrap().as_ref() {
                draw_palette(ctx, p, &config_for_draw)?;
            }
            Ok(())
        })?;

        let canvas_arc = Arc::new(Mutex::new(SendableCanvas(raw_canvas)));

        // Set canvas directly as window content — no Box wrapper.
        // Keeps the HWND hierarchy as canvas → NativeGuiWindow (one hop).
        // set_window_content resizes the canvas to fill the client area and
        // calls SetFocus(window) so keyboard input works immediately.
        window.set_content(SharedCanvas(canvas_arc.clone()))?;

        {
            let mut canvas = canvas_arc.lock().unwrap();
            let mut ws = self.workspace.lock().unwrap();
            let config = self.config.clone();
            canvas.draw(|ctx| draw_editor(ctx, &mut ws, &config, None))?;
            canvas.invalidate_all();
        }

        let mut last_title = String::new();
        // Pending chord prefix carried across key events (e.g. after `ctrl+k`).
        let mut chord_pending: Vec<KeyStroke> = Vec::new();
        // Live terminal sessions, keyed by their Terminal buffer.
        let mut terminals: HashMap<BufferId, TermSession> = HashMap::new();
        let mut failed_terminals: std::collections::HashSet<BufferId> = std::collections::HashSet::new();

        // --------------- event loop ---------------
        loop {
            // Pump Win32 messages FIRST so the events are in the Rust queue
            // before we drain it.  Ensures key presses are processed in the
            // same 8 ms frame they arrive, not the next one.
            unsafe { aurea::ffi::ng_platform_poll_events() };

            let events = window.poll_events();
            let mut should_close = false;
            let mut needs_redraw = false;
            // When the palette opens via a key, the trigger char (e.g. the `x`
            // of M-x) may also arrive as TextInput; swallow that one char.
            let mut swallow_text = false;

            let has_text_input = events.iter().any(|event| {
                matches!(event, WindowEvent::TextInput { text } if text.chars().any(|c| !c.is_control()))
            });

            for event in events {
                match event {
                    WindowEvent::CloseRequested => { should_close = true; }
                    WindowEvent::Resized { width, height } => {
                        let _ = (width, height);
                        needs_redraw = true;
                    }

                    WindowEvent::KeyInput { key, pressed: true, modifiers } => {
                        let mut pal = palette.lock().unwrap();
                        let mut srch = search.lock().unwrap();
                        if pal.is_some() {
                            let mut ws = self.workspace.lock().unwrap();
                            if handle_palette_key(key, &mut pal, &mut ws, &self.commands, &self.autocmds) {
                                needs_redraw = true;
                            }
                        } else if srch.is_some() {
                            let mut ws = self.workspace.lock().unwrap();
                            if handle_search_key(key, &mut srch, &mut ws) {
                                needs_redraw = true;
                            }
                        } else if let Some(term_id) = active_terminal(&self.workspace.lock().unwrap())
                            .filter(|id| terminals.contains_key(id))
                        {
                            // Active buffer is a live terminal: Enter sends the
                            // line, Backspace edits it; typed chars come via TextInput.
                            use aurea::KeyCode::*;
                            let sess = terminals.get_mut(&term_id).unwrap();
                            match key {
                                Enter => {
                                    let line = std::mem::take(&mut sess.input);
                                    sess.term.write_line(&line);
                                    needs_redraw = true;
                                }
                                Backspace => {
                                    sess.input.pop();
                                    needs_redraw = true;
                                }
                                _ => {}
                            }
                        } else {
                            let r = handle_key(
                                key,
                                modifiers,
                                !has_text_input,
                                &mut self.workspace.lock().unwrap(),
                                &self.commands,
                                &self.autocmds,
                                &self.keymap,
                                &self.modmap,
                                &mut chord_pending,
                                &mut pal,
                                &mut srch,
                            );
                            if r {
                                needs_redraw = true;
                            }
                            // If a key just opened the palette or search, the
                            // trigger char (M-x's `x`, M-f's `f`) also arrives as
                            // TextInput on Windows; drop it so the query starts empty.
                            if pal.is_some() || srch.is_some() {
                                swallow_text = true;
                            }
                        }
                    }

                    // Text input is the primary edit path. While the palette is
                    // open, typed chars filter it instead of editing the buffer.
                    WindowEvent::TextInput { text } => {
                        if swallow_text {
                            swallow_text = false; // drop the palette trigger char
                            continue;
                        }
                        let mut pal = palette.lock().unwrap();
                        if let Some(p) = pal.as_mut() {
                            for c in text.chars().filter(|c| !c.is_control()) {
                                p.push(c);
                                needs_redraw = true;
                            }
                        } else {
                            drop(pal);
                            let mut srch = search.lock().unwrap();
                            if let Some(s) = srch.as_mut() {
                                let mut ws = self.workspace.lock().unwrap();
                                let mut changed = false;
                                for c in text.chars().filter(|c| !c.is_control()) {
                                    s.query.push(c);
                                    changed = true;
                                }
                                if changed {
                                    search_recompute(s, &ws);
                                    search_select_from_cursor(s, &ws);
                                    search_jump(s, &mut ws);
                                    needs_redraw = true;
                                }
                            } else {
                                drop(srch);
                                let mut ws = self.workspace.lock().unwrap();
                                let term_id = active_terminal(&ws).filter(|id| terminals.contains_key(id));
                                if let Some(term_id) = term_id {
                                    // Echo typed chars into the terminal's input line.
                                    let sess = terminals.get_mut(&term_id).unwrap();
                                    for c in text.chars().filter(|c| !c.is_control()) {
                                        sess.input.push(c);
                                        needs_redraw = true;
                                    }
                                } else if insert_text_raw(&text, &mut ws) {
                                    dispatch_autocmds(&mut ws, &self.commands, &self.autocmds);
                                    needs_redraw = true;
                                }
                            }
                        }
                    }

                    WindowEvent::MouseWheel { delta_y, .. } => {
                        let mut ws = self.workspace.lock().unwrap();
                        let max_scroll = ws
                            .active_view()
                            .and_then(|view| {
                                ws.buffers
                                    .get(&view.buffer_id)
                                    .map(|buf| max_scroll_line(buf.line_count(), view.page_height))
                            })
                            .unwrap_or(0);
                        if let Some(view) = ws.active_view_mut() {
                            let lines = (delta_y.abs() * 3.0).round() as usize;
                            if lines > 0 {
                                if delta_y > 0.0 {
                                    view.scroll_line = view.scroll_line.saturating_sub(lines);
                                } else {
                                    view.scroll_line = view.scroll_line.saturating_add(lines).min(max_scroll);
                                }
                            }
                        }
                        needs_redraw = true;
                    }

                    _ => {}
                }
            }

            if should_close { break; }

            // --- terminal sync: spawn, stream output into the buffer, scroll ---
            {
                let mut ws = self.workspace.lock().unwrap();
                let term_bufs: Vec<BufferId> = ws
                    .buffers
                    .iter()
                    .filter(|(_, b)| matches!(b.kind, BufferKind::Terminal))
                    .map(|(id, _)| *id)
                    .collect();
                // Attach a shell to any Terminal buffer that lacks one.
                for id in &term_bufs {
                    if terminals.contains_key(id) || failed_terminals.contains(id) {
                        continue;
                    }
                    match Terminal::spawn() {
                        Ok(term) => {
                            terminals.insert(*id, TermSession { term, input: String::new() });
                        }
                        Err(e) => {
                            failed_terminals.insert(*id);
                            if let Some(buf) = ws.buffers.get_mut(id) {
                                buf.set_text(&format!("could not start terminal: {e}\n"));
                            }
                            needs_redraw = true;
                        }
                    }
                }
                // Forget sessions whose buffer was closed.
                terminals.retain(|id, _| ws.buffers.contains_key(id));

                let active_term = active_terminal(&ws);
                for (id, sess) in terminals.iter_mut() {
                    let is_active = active_term == Some(*id);
                    let mut text = sess.term.output_snapshot();
                    if is_active {
                        text.push_str(&sess.input);
                    }
                    let changed = ws.buffers.get(id).map(|b| b.text() != text).unwrap_or(false);
                    if changed {
                        if let Some(buf) = ws.buffers.get_mut(id) {
                            buf.set_text(&text);
                        }
                        if is_active {
                            // Pin the cursor to the end so the view follows output.
                            let last = ws.buffers.get(id).map(|b| b.line_count().saturating_sub(1)).unwrap_or(0);
                            let col = ws.buffers.get(id).map(|b| b.line_len(last)).unwrap_or(0);
                            if let Some(view) = ws.active_view_mut() {
                                view.cursor = ozone_buffer::Pos::new(last, col);
                                view.scroll_line = usize::MAX; // clamped to bottom in draw_view
                            }
                        }
                        needs_redraw = true;
                    }
                }
            }

            // Update window title when active file changes
            {
                let ws = self.workspace.lock().unwrap();
                let title = window_title(&ws);
                if title != last_title {
                    let _ = window.set_title(&title);
                    last_title = title;
                }
            }

            if needs_redraw {
                let mut canvas = canvas_arc.lock().unwrap();
                let mut ws = self.workspace.lock().unwrap();
                let config = self.config.clone();
                let pal = palette.lock().unwrap();
                let srch = search.lock().unwrap();
                canvas.draw(|ctx| {
                    draw_editor(ctx, &mut ws, &config, srch.as_ref())?;
                    if let Some(p) = pal.as_ref() {
                        draw_palette(ctx, p, &config)?;
                    }
                    Ok(())
                })?;
                canvas.invalidate_all();
            }

            window.process_frames()?;
            std::thread::sleep(std::time::Duration::from_millis(8));
        }

        Ok(())
    }
}

fn window_title(ws: &Workspace) -> String {
    // Use ASCII-only separators: Windows ANSI title bar can't render em-dashes.
    match ws.active_buffer() {
        Some(buf) => {
            let dirty = if buf.is_dirty() { "*" } else { "" };
            match &buf.kind {
                BufferKind::File(p) => {
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                    format!("Ozone - {}{}", dirty, name)
                }
                BufferKind::Scratch => format!("Ozone - {}scratch", dirty),
                BufferKind::Search => format!("Ozone - {}files", dirty),
                BufferKind::References => format!("Ozone - {}references", dirty),
                BufferKind::Terminal => format!("Ozone - {}terminal", dirty),
            }
        }
        None => "Ozone".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Key routing
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn handle_key(
    key: aurea::KeyCode,
    mods: aurea::Modifiers,
    allow_text_fallback: bool,
    ws: &mut Workspace,
    reg: &CommandRegistry,
    autocmds: &AutocommandRegistry,
    keymap: &Keymap,
    modmap: &ModifierMap,
    pending: &mut Vec<KeyStroke>,
    palette: &mut Option<PickerState>,
    search: &mut Option<SearchState>,
) -> bool {
    use aurea::KeyCode::*;

    // Bare modifier presses are never a binding and never cancel a chord.
    if matches!(key, Shift | Control | Alt | Meta) {
        return false;
    }

    // Picker buffers take precedence so Enter/Esc act on the selection rather
    // than the editing defaults. (Edit keys are swallowed to keep the list.)
    let in_picker = matches!(ws.active_buffer().map(|b| &b.kind), Some(BufferKind::Search));
    if in_picker && pending.is_empty() && !mods.ctrl && !mods.alt {
        match key {
            Enter => {
                run_cmd("picker.open-selection", ws, reg, autocmds);
                return true;
            }
            Escape => {
                run_cmd("pane.close", ws, reg, autocmds);
                return true;
            }
            Backspace | Delete | Tab => return true,
            _ => {}
        }
    }

    // Resolve through the layered keymap (handles chords via `pending`).
    if let Some(stroke) = keystroke_from(key, mods, modmap) {
        let filetype = active_filetype_name(ws);
        match keymap.resolve(pending, &stroke, filetype.as_deref()) {
            KeymapOutcome::Execute(cmd) => {
                pending.clear();
                if cmd == "command.palette" {
                    // GUI-level command: open the overlay instead of dispatching.
                    *palette = Some(PickerState::new("M-x", command_picker_items(reg)));
                } else if cmd == "file.picker" {
                    *palette = Some(PickerState::new("find file:", file_picker_items()));
                } else if cmd == "search.start" {
                    let mut s = SearchState::new(false);
                    search_recompute(&mut s, ws);
                    search_select_from_cursor(&mut s, ws);
                    *search = Some(s);
                } else {
                    run_cmd(&cmd, ws, reg, autocmds);
                }
                return true;
            }
            KeymapOutcome::Pending => {
                pending.push(stroke);
                return true;
            }
            KeymapOutcome::NoMatch => {
                // A failed chord continuation is swallowed; a fresh unmatched key
                // falls through to text entry below.
                let had_pending = !pending.is_empty();
                pending.clear();
                if had_pending {
                    return true;
                }
            }
        }
    }

    // Fallbacks for keys that are not bound commands.
    if !mods.ctrl && !mods.alt {
        if key == Tab {
            let unit = ws.indent.unit(); // soft tab / tab per config
            return insert_text_raw(&unit, ws);
        }
        if allow_text_fallback && let Some(ch) = keycode_to_char(key, mods.shift) {
            let mut buf = [0u8; 4];
            return insert_text_raw(ch.encode_utf8(&mut buf), ws);
        }
    }

    false
}

/// The active buffer's id if it is a live terminal surface.
fn active_terminal(ws: &Workspace) -> Option<BufferId> {
    let view = ws.active_view()?;
    match ws.buffers.get(&view.buffer_id)?.kind {
        BufferKind::Terminal => Some(view.buffer_id),
        _ => None,
    }
}

/// Filetype token for the active buffer (for filetype-scoped keymaps).
fn active_filetype_name(ws: &Workspace) -> Option<String> {
    match &ws.active_buffer()?.kind {
        BufferKind::File(p) => Some(filetype_config_name(Filetype::from_path(&p.to_string_lossy()))),
        _ => None,
    }
}

fn filetype_config_name(ft: Filetype) -> String {
    match ft {
        Filetype::Rust => "rust",
        Filetype::Toml => "toml",
        Filetype::Json => "json",
        Filetype::Markdown => "markdown",
        Filetype::Plain => "plain",
    }
    .to_string()
}

/// Convert a platform key + physical modifiers into a logical [`KeyStroke`] via
/// the modifier map. Returns `None` for keys with no token (modifiers, unknown).
fn keystroke_from(key: aurea::KeyCode, mods: aurea::Modifiers, map: &ModifierMap) -> Option<KeyStroke> {
    let k = keycode_key(key)?;
    let phys = PhysicalMods::new(mods.ctrl, mods.alt, mods.shift, mods.meta);
    Some(KeyStroke::from_physical(phys, k, map))
}

/// Map a platform key code to a structured [`Key`]. `None` for modifier-only
/// or unknown codes.
fn keycode_key(key: aurea::KeyCode) -> Option<Key> {
    use aurea::KeyCode::*;
    Some(match key {
        A => Key::Char('a'), B => Key::Char('b'), C => Key::Char('c'), D => Key::Char('d'),
        E => Key::Char('e'), F => Key::Char('f'), G => Key::Char('g'), H => Key::Char('h'),
        I => Key::Char('i'), J => Key::Char('j'), K => Key::Char('k'), L => Key::Char('l'),
        M => Key::Char('m'), N => Key::Char('n'), O => Key::Char('o'), P => Key::Char('p'),
        Q => Key::Char('q'), R => Key::Char('r'), S => Key::Char('s'), T => Key::Char('t'),
        U => Key::Char('u'), V => Key::Char('v'), W => Key::Char('w'), X => Key::Char('x'),
        Y => Key::Char('y'), Z => Key::Char('z'),
        Key0 => Key::Char('0'), Key1 => Key::Char('1'), Key2 => Key::Char('2'),
        Key3 => Key::Char('3'), Key4 => Key::Char('4'), Key5 => Key::Char('5'),
        Key6 => Key::Char('6'), Key7 => Key::Char('7'), Key8 => Key::Char('8'),
        Key9 => Key::Char('9'),
        Space => Key::Space, Enter => Key::Enter, Escape => Key::Escape, Tab => Key::Tab,
        Backspace => Key::Backspace, Delete => Key::Delete, Insert => Key::Insert,
        Home => Key::Home, End => Key::End, PageUp => Key::PageUp, PageDown => Key::PageDown,
        Up => Key::Up, Down => Key::Down, Left => Key::Left, Right => Key::Right,
        F1 => Key::F(1), F2 => Key::F(2), F3 => Key::F(3), F4 => Key::F(4),
        F5 => Key::F(5), F6 => Key::F(6), F7 => Key::F(7), F8 => Key::F(8),
        F9 => Key::F(9), F10 => Key::F(10), F11 => Key::F(11), F12 => Key::F(12),
        Shift | Control | Alt | Meta | Unknown(_) => return None,
    })
}

/// Handle a key while the picker is open. Letters arrive via TextInput;
/// this covers navigation/commit/cancel. Returns whether a redraw is needed.
fn handle_palette_key(
    key: aurea::KeyCode,
    palette: &mut Option<PickerState>,
    ws: &mut Workspace,
    reg: &CommandRegistry,
    autocmds: &AutocommandRegistry,
) -> bool {
    use aurea::KeyCode::*;
    let Some(p) = palette.as_mut() else { return false };
    match key {
        Escape => {
            *palette = None;
            true
        }
        Enter => {
            // Clone the chosen action, then close before performing it.
            let action = p.selected_item().map(|item| match &item.action {
                PickerAction::RunCommand(c) => PickerAction::RunCommand(c.clone()),
                PickerAction::OpenFile(path) => PickerAction::OpenFile(path.clone()),
            });
            *palette = None;
            match action {
                Some(PickerAction::RunCommand(cmd)) => run_cmd(&cmd, ws, reg, autocmds),
                Some(PickerAction::OpenFile(path)) => {
                    let _ = ws.open_file(path);
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
        // Modifier-only keys: ignore. Letters/symbols come through TextInput.
        _ => false,
    }
}

fn run_cmd(name: &str, ws: &mut Workspace, reg: &CommandRegistry, autocmds: &AutocommandRegistry) {
    if name == "file.save" {
        if let Some(buffer_id) = ws.active_view().map(|view| view.buffer_id) {
            run_pre_save_autocmds(buffer_id, ws, reg, autocmds);
        }
    } else if name == "file.save-all" {
        let ids: Vec<_> = ws.buffers.keys().copied().collect();
        for id in ids {
            run_pre_save_autocmds(id, ws, reg, autocmds);
        }
    }

    execute_command(name, ws, reg);
    dispatch_autocmds(ws, reg, autocmds);
}

fn execute_command(name: &str, ws: &mut Workspace, reg: &CommandRegistry) {
    if let Some(mut ctx) = CommandContext::new(ws) {
        reg.execute(name, &mut ctx);
    }
    if let Some(view) = ws.active_view_mut() {
        view.scroll_to_cursor(view.page_height.max(1));
    }
}

fn run_pre_save_autocmds(
    buffer_id: BufferId,
    ws: &mut Workspace,
    reg: &CommandRegistry,
    autocmds: &AutocommandRegistry,
) {
    let path = ws.buffers.get(&buffer_id).and_then(|buf| match &buf.kind {
        BufferKind::File(path) => Some(path.clone()),
        _ => None,
    });
    let Some(path) = path else {
        return;
    };

    let event = EditorEvent::BufferPreSave { id: buffer_id, path };
    let commands: Vec<String> = autocmds
        .matching_commands(&event)
        .into_iter()
        .map(str::to_string)
        .collect();
    for command in commands {
        if command == "file.save" || command == "file.save-all" {
            continue;
        }
        execute_command(&command, ws, reg);
    }
}

fn dispatch_autocmds(ws: &mut Workspace, reg: &CommandRegistry, autocmds: &AutocommandRegistry) {
    const MAX_AUTOCMD_ROUNDS: usize = 16;

    for _ in 0..MAX_AUTOCMD_ROUNDS {
        let events = ws.drain_events();
        if events.is_empty() {
            break;
        }

        let commands: Vec<String> = events
            .iter()
            .flat_map(|event| autocmds.matching_commands(event))
            .map(str::to_string)
            .collect();

        if commands.is_empty() {
            continue;
        }

        for command in commands {
            if command == "file.save" || command == "file.save-all" {
                continue;
            }
            execute_command(&command, ws, reg);
        }
    }
}

fn insert_text_raw(text: &str, ws: &mut Workspace) -> bool {
    let filtered: String = text.chars().filter(|c| !c.is_control()).collect();
    if filtered.is_empty() { return false; }

    let Some(view) = ws.active_view() else { return false };
    let cursor = view.cursor;
    let buf_id = view.buffer_id;

    // Virtual/read-only surfaces (pickers, terminal placeholder) reject edits.
    if matches!(
        ws.buffers.get(&buf_id).map(|b| &b.kind),
        Some(BufferKind::Search | BufferKind::References | BufferKind::Terminal)
    ) {
        return false;
    }

    if let Some(buf) = ws.buffers.get_mut(&buf_id) {
        let delta = buf.insert(cursor, &filtered);
        // Cursor columns are byte offsets (see Pos docs); advance by the inserted
        // byte length, not the char count, or multi-byte input desyncs the cursor
        // from the buffer offset.
        let bytes = filtered.len();
        let cursor_event = ws.active_view_mut().map(|view| {
            view.cursor.col += bytes;
            view.col_memory = view.cursor.col;
            view.scroll_to_cursor(view.page_height.max(1));
            EditorEvent::CursorMoved { view_id: view.id, pos: view.cursor }
        });
        if let Some(event) = cursor_event {
            ws.emit(event);
        }
        ws.emit(EditorEvent::BufferChanged { id: buf_id, delta });
        return true;
    }
    false
}

fn keycode_to_char(key: aurea::KeyCode, shift: bool) -> Option<char> {
    use aurea::KeyCode::*;
    Some(match key {
        A => if shift { 'A' } else { 'a' }, B => if shift { 'B' } else { 'b' },
        C => if shift { 'C' } else { 'c' }, D => if shift { 'D' } else { 'd' },
        E => if shift { 'E' } else { 'e' }, F => if shift { 'F' } else { 'f' },
        G => if shift { 'G' } else { 'g' }, H => if shift { 'H' } else { 'h' },
        I => if shift { 'I' } else { 'i' }, J => if shift { 'J' } else { 'j' },
        K => if shift { 'K' } else { 'k' }, L => if shift { 'L' } else { 'l' },
        M => if shift { 'M' } else { 'm' }, N => if shift { 'N' } else { 'n' },
        O => if shift { 'O' } else { 'o' }, P => if shift { 'P' } else { 'p' },
        Q => if shift { 'Q' } else { 'q' }, R => if shift { 'R' } else { 'r' },
        S => if shift { 'S' } else { 's' }, T => if shift { 'T' } else { 't' },
        U => if shift { 'U' } else { 'u' }, V => if shift { 'V' } else { 'v' },
        W => if shift { 'W' } else { 'w' }, X => if shift { 'X' } else { 'x' },
        Y => if shift { 'Y' } else { 'y' }, Z => if shift { 'Z' } else { 'z' },
        Key0 => if shift { ')' } else { '0' }, Key1 => if shift { '!' } else { '1' },
        Key2 => if shift { '@' } else { '2' }, Key3 => if shift { '#' } else { '3' },
        Key4 => if shift { '$' } else { '4' }, Key5 => if shift { '%' } else { '5' },
        Key6 => if shift { '^' } else { '6' }, Key7 => if shift { '&' } else { '7' },
        Key8 => if shift { '*' } else { '8' }, Key9 => if shift { '(' } else { '9' },
        Space => ' ',
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Rendering constants — Catppuccin Mocha
// ---------------------------------------------------------------------------

const BG:           Color = Color::rgb(30,  30,  46);
const GUTTER_BG:    Color = Color::rgb(24,  24,  37);
const GUTTER_FG:    Color = Color::rgb(88,  91, 112);
const GUTTER_ACT:   Color = Color::rgb(205, 214, 244);
const STATUSBAR_BG: Color = Color::rgb(24,  24,  37);
const STATUSBAR_FG: Color = Color::rgb(166, 227, 161);
const STATUSBAR_DIM: Color = Color::rgb(137, 180, 250);
const STATUS_MODE_BG: Color = Color::rgb(49,  50,  68);
const BORDER:       Color = Color::rgb(49,  50,  68);
const CURSOR_BG:    Color = Color::rgba(245, 224, 220, 220);
const CURSOR_LINE:  Color = Color::rgba(49,  50,  68, 140);
const ACTIVE_PANE_BORDER: Color = Color::rgb(137, 180, 250);
const BRACKET_MATCH: Color = Color::rgba(137, 180, 250, 70);
const PALETTE_SCRIM:  Color = Color::rgba(0, 0, 0, 110);
const PALETTE_BG:     Color = Color::rgb(24, 24, 37);
const PALETTE_BORDER: Color = Color::rgb(69, 71, 90);
const PALETTE_INPUT_BG: Color = Color::rgb(17, 17, 27);
const PALETTE_SEL:    Color = Color::rgb(49, 50, 68);
const PALETTE_FG:     Color = Color::rgb(205, 214, 244);
const PALETTE_DESC:   Color = Color::rgb(127, 132, 156);
const PALETTE_PROMPT: Color = Color::rgb(203, 166, 247);
const SCROLLBAR_THUMB: Color = Color::rgba(88, 91, 112, 180);
const SEARCH_MATCH:   Color = Color::rgba(249, 226, 175, 70);  // yellow, all matches
const SEARCH_CURRENT: Color = Color::rgba(250, 179, 135, 150); // peach, current match

// Catppuccin Mocha syntax palette
fn token_color(kind: TokenKind) -> Color {
    match kind {
        TokenKind::Keyword        => Color::rgb(203, 166, 247), // mauve
        TokenKind::KeywordControl => Color::rgb(243, 139, 168), // red
        TokenKind::Type           => Color::rgb(137, 180, 250), // blue
        TokenKind::String         => Color::rgb(166, 227, 161), // green
        TokenKind::Comment        => Color::rgb(88,  91,  112), // overlay0
        TokenKind::Number         => Color::rgb(250, 179, 135), // peach
        TokenKind::Macro          => Color::rgb(137, 220, 235), // sky
        TokenKind::Attribute      => Color::rgb(245, 194, 231), // flamingo
        TokenKind::Lifetime       => Color::rgb(245, 194, 231), // flamingo
        TokenKind::Function       => Color::rgb(137, 180, 250), // blue
        TokenKind::Operator       => Color::rgb(137, 220, 235), // sky
        TokenKind::SectionHeader  => Color::rgb(203, 166, 247), // mauve
        _                         => Color::rgb(205, 214, 244), // text
    }
}

const GUTTER_MIN_W: f32 = 52.0;
const PAD:      f32 = 8.0;
const STATUS_H: f32 = 28.0;
const EDITOR_TOP_PAD: f32 = 10.0;
const SPLIT_GAP: f32 = 4.0;

fn editor_font(config: &Config) -> Font {
    Font::new(&config.editor.font, config.editor.font_size)
}

// ---------------------------------------------------------------------------
// draw_editor
// ---------------------------------------------------------------------------

fn draw_editor(
    ctx: &mut dyn DrawingContext,
    ws: &mut Workspace,
    config: &Config,
    search: Option<&SearchState>,
) -> AureaResult<()> {
    let width  = ctx.width()  as f32;
    let height = ctx.height() as f32;

    ctx.clear(BG)?;

    let font   = editor_font(config);
    let metrics = ctx.measure_text("M", &font).ok();
    let char_w = metrics.as_ref().map(|m| m.advance).unwrap_or(font.size * 0.6);
    let text_ascent = metrics.as_ref().map(|m| m.ascent).unwrap_or(font.size * 0.8);
    let text_descent = metrics.as_ref().map(|m| m.descent).unwrap_or(font.size * 0.2);

    let editor_rect = Rect::new(0.0, 0.0, width, (height - STATUS_H).max(0.0));
    let metrics = TextMetrics { char_w, text_ascent, text_descent };

    if let Some(panes) = &ws.panes {
        let panes = panes.clone();
        draw_pane_tree(ctx, ws, config, &panes, editor_rect, &font, metrics, search)?;
    } else if let Some(view_id) = ws.active_view().map(|view| view.id) {
        draw_view(ctx, ws, config, view_id, editor_rect, &font, metrics, search)?;
    }

    if let Some(s) = search {
        draw_search_bar(ctx, s, &font, width)?;
    }

    draw_status_bar(ctx, width, height, &font, ws)?;
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
    search: Option<&SearchState>,
) -> AureaResult<()> {
    match tree {
        PaneTree::Leaf { view_id } => draw_view(ctx, ws, config, *view_id, rect, font, metrics, search),
        PaneTree::Split { axis, ratio, first, second } => {
            let (first_rect, second_rect, divider) = split_rect(rect, *axis, *ratio);
            draw_pane_tree(ctx, ws, config, first, first_rect, font, metrics, search)?;
            draw_pane_tree(ctx, ws, config, second, second_rect, font, metrics, search)?;
            ctx.draw_rect(divider, &solid(BORDER))?;
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
    search: Option<&SearchState>,
) -> AureaResult<()> {
    let Some(buffer_id) = ws.views.get(&view_id).map(|view| view.buffer_id) else {
        return Ok(());
    };
    let Some(line_count) = ws.buffers.get(&buffer_id).map(|buf| buf.line_count()) else {
        return Ok(());
    };

    let is_active_pane = ws.active_view_id == Some(view_id);
    let line_h = font.size * config.editor.line_height;
    let content_top = rect.y + EDITOR_TOP_PAD;
    let content_h = (rect.height - EDITOR_TOP_PAD).max(0.0);
    let visible = ((content_h / line_h) as usize).max(1);

    if let Some(view) = ws.views.get_mut(&view_id) {
        view.page_height = visible;
        view.scroll_line = view.scroll_line.min(max_scroll_line(line_count, visible));
    }

    let Some(view) = ws.views.get(&view_id) else {
        return Ok(());
    };
    let Some(buf) = ws.buffers.get(&buffer_id) else {
        return Ok(());
    };

    ctx.draw_rect(rect, &solid(BG))?;

    // Filetype for syntax
    let ft = match &buf.kind {
        BufferKind::File(p) => Filetype::from_path(&p.to_string_lossy()),
        _ => Filetype::Plain,
    };

    let scroll      = view.scroll_line;
    let visible     = visible + 1;
    let gutter_w    = gutter_width(line_count, metrics.char_w, config.editor.line_numbers);
    let text_x      = rect.x + gutter_w + PAD;

    // Matching-bracket pair for the active cursor (highlighted behind the glyphs).
    let bracket_pair = if is_active_pane {
        matching_bracket(buf, view.cursor)
    } else {
        None
    };

    // Search-match positions for the active pane (Pos, is_current).
    let search_hits: Vec<(ozone_buffer::Pos, bool)> = match (is_active_pane, search) {
        (true, Some(s)) if !s.query.is_empty() => s
            .matches
            .iter()
            .enumerate()
            .map(|(i, &off)| (buf.offset_to_pos(off), i == s.current))
            .collect(),
        _ => Vec::new(),
    };
    let search_qlen = search.map(|s| s.query.chars().count()).unwrap_or(0);

    // Gutter strip
    if gutter_w > 0.0 {
        ctx.draw_rect(Rect::new(rect.x, rect.y, gutter_w, rect.height), &solid(GUTTER_BG))?;
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
        if line_idx >= line_count { break; }

        let line_top = content_top + i as f32 * line_h;
        if line_top >= content_top + content_h || line_top >= rect.y + rect.height { break; }

        let baseline = baseline_in_rect(line_top, line_h, metrics.text_ascent, metrics.text_descent);
        let is_cursor = line_idx == view.cursor.line;

        // Cursor-line highlight
        if is_cursor && is_active_pane {
            ctx.draw_rect(Rect::new(rect.x, line_top + 1.0, rect.width, line_h - 1.0), &solid(CURSOR_LINE))?;
        }

        // Search match highlights (behind the glyphs).
        if search_qlen > 0 {
            for (pos, is_current) in &search_hits {
                if pos.line == line_idx {
                    let hx = text_x + pos.col as f32 * metrics.char_w;
                    let hw = search_qlen as f32 * metrics.char_w;
                    let col = if *is_current { SEARCH_CURRENT } else { SEARCH_MATCH };
                    ctx.draw_rect(Rect::new(hx, line_top + 1.0, hw, line_h - 2.0), &solid(col))?;
                }
            }
        }

        // Matching-bracket boxes (behind the glyphs).
        if let Some((p1, p2)) = bracket_pair {
            for bp in [p1, p2] {
                if bp.line == line_idx {
                    let bx = text_x + bp.col as f32 * metrics.char_w;
                    ctx.draw_rect(
                        Rect::new(bx, line_top + 1.0, metrics.char_w, line_h - 2.0),
                        &solid(BRACKET_MATCH),
                    )?;
                }
            }
        }

        // Gutter line number (absolute / relative / off per config)
        let gutter_label = match config.editor.line_numbers {
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
            let ng = if is_cursor { GUTTER_ACT } else { GUTTER_FG };
            let num_x = (rect.x + gutter_w - PAD - num.len() as f32 * metrics.char_w).max(rect.x + 4.0);
            ctx.draw_text_with_font(&num, Point::new(num_x, baseline), &font, &solid(ng))?;
        }

        // Line text with syntax
        if let Some(line_text) = buf.line(line_idx) {
            let (spans, new_state) = scan_line(ft, &line_text, scan_state);
            scan_state = new_state;

            if spans.is_empty() || ft == Filetype::Plain {
                ctx.draw_text_with_font(
                    &line_text,
                    Point::new(text_x, baseline),
                    &font,
                    &solid(token_color(TokenKind::Default)),
                )?;
            } else {
                draw_highlighted(ctx, &line_text, &spans, text_x, baseline, metrics.char_w, &font)?;
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
        ctx.draw_line(rect.x + gutter_w, rect.y, rect.x + gutter_w, rect.y + rect.height, &stroke(BORDER, 1.0))?;
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
        ctx.draw_rect(Rect::new(bar_x, thumb_y, 3.0, thumb_h), &solid(SCROLLBAR_THUMB))?;
    }

    if is_active_pane {
        ctx.draw_rect(Rect::new(rect.x, rect.y, rect.width, 2.0), &solid(ACTIVE_PANE_BORDER))?;
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
            ctx.draw_text_with_font(seg, Point::new(sx, y), font, &solid(token_color(TokenKind::Default)))?;
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
        ctx.draw_text_with_font(seg, Point::new(sx, y), font, &solid(token_color(TokenKind::Default)))?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// draw_status_bar
// ---------------------------------------------------------------------------

fn draw_status_bar(
    ctx: &mut dyn DrawingContext,
    width: f32,
    height: f32,
    font: &Font,
    ws: &Workspace,
) -> AureaResult<()> {
    let bar_top = height - STATUS_H;
    ctx.draw_rect(Rect::new(0.0, bar_top, width, STATUS_H), &solid(STATUSBAR_BG))?;
    ctx.draw_line(0.0, bar_top, width, bar_top, &stroke(BORDER, 1.0))?;

    // Emacs-style modeline: the left badge is the buffer's *major mode*, not a
    // generic "EDIT" (Ozone is non-modal). Transient input modes (find, M-x) have
    // their own overlays, so they don't belong here.
    let (mode, file_name, cursor_info, dirty, pane_info) = if let (Some(view), Some(buf)) = (
        ws.active_view(), ws.active_buffer(),
    ) {
        let file_name = match &buf.kind {
            BufferKind::File(p) => p.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string(),
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
            BufferKind::Scratch => "Text",
        };
        let pane_info = pane_status(ws, view.id);
        (mode, file_name, cursor_info, dirty.to_string(), pane_info)
    } else {
        ("", String::new(), String::new(), String::new(), String::new())
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
    ctx.draw_rect(Rect::new(8.0, bar_top + 4.0, mode_w + 8.0, STATUS_H - 8.0), &solid(STATUS_MODE_BG))?;
    ctx.draw_text_with_font(&mode_text, Point::new(12.0, baseline), font, &solid(STATUSBAR_FG))?;

    let left = format!("  {}{}    {}", file_name, dirty, cursor_info);
    ctx.draw_text_with_font(&left, Point::new(16.0 + mode_w, baseline), font, &solid(STATUSBAR_FG))?;

    let right = if pane_info.is_empty() {
        "UTF-8".to_string()
    } else {
        format!("{}  UTF-8", pane_info)
    };
    let right_w = ctx
        .measure_text(&right, font)
        .map(|m| m.advance)
        .unwrap_or(right.len() as f32 * font.size * 0.6);
    let right_x = (width - right_w - 12.0).max(16.0 + mode_w);
    ctx.draw_text_with_font(&right, Point::new(right_x, baseline), font, &solid(STATUSBAR_DIM))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Fuzzy picker overlay rendering
// ---------------------------------------------------------------------------

fn draw_palette(ctx: &mut dyn DrawingContext, p: &PickerState, config: &Config) -> AureaResult<()> {
    let w = ctx.width() as f32;
    let h = ctx.height() as f32;
    let font = editor_font(config);
    let line_h = (font.size * 1.7).max(18.0);
    let pad = 14.0;
    let radius = 10.0;

    let m = ctx.measure_text("M", &font).ok();
    let ascent = m.as_ref().map(|x| x.ascent).unwrap_or(font.size * 0.8);
    let descent = m.as_ref().map(|x| x.descent).unwrap_or(font.size * 0.2);
    let measure = |ctx: &mut dyn DrawingContext, s: &str| {
        ctx.measure_text(s, &font).map(|m| m.advance).unwrap_or(s.len() as f32 * font.size * 0.6)
    };

    // Dim the editor behind the panel.
    ctx.draw_rect(Rect::new(0.0, 0.0, w, h), &solid(PALETTE_SCRIM))?;

    // Window the visible rows around the selection.
    let max_rows = 12usize;
    let start = if p.selected >= max_rows { p.selected + 1 - max_rows } else { 0 };
    let shown: Vec<usize> = p.filtered.iter().skip(start).take(max_rows).copied().collect();

    let pw = (w * 0.6).clamp(380.0, 760.0);
    let header_h = line_h + pad * 2.0;
    let body_rows = shown.len().max(1); // reserve a row for "no matches"
    let ph = header_h + body_rows as f32 * line_h + pad;
    let px = (w - pw) / 2.0;
    let py = ((h - ph) / 2.0).max(20.0);

    // Rounded panel with a 1px border.
    fill_round_rect(ctx, Rect::new(px - 1.0, py - 1.0, pw + 2.0, ph + 2.0), radius + 1.0, PALETTE_BORDER)?;
    fill_round_rect(ctx, Rect::new(px, py, pw, ph), radius, PALETTE_BG)?;

    // Input box: "<prompt> <query>" with a caret.
    let input_rect = Rect::new(px + pad, py + pad, pw - 2.0 * pad, line_h);
    fill_round_rect(ctx, input_rect, 6.0, PALETTE_INPUT_BG)?;
    let in_baseline = baseline_in_rect(input_rect.y, input_rect.height, ascent, descent);
    ctx.draw_text_with_font(&p.prompt, Point::new(input_rect.x + 8.0, in_baseline), &font, &solid(PALETTE_PROMPT))?;
    let prompt_w = measure(ctx, &format!("{} ", p.prompt));
    let query_x = input_rect.x + 8.0 + prompt_w;
    ctx.draw_text_with_font(&p.query, Point::new(query_x, in_baseline), &font, &solid(PALETTE_FG))?;
    let caret_x = query_x + measure(ctx, &p.query) + 1.0;
    ctx.draw_rect(Rect::new(caret_x, input_rect.y + 4.0, 2.0, line_h - 8.0), &solid(PALETTE_FG))?;

    // Result list.
    let list_top = py + header_h;
    if shown.is_empty() {
        let bl = baseline_in_rect(list_top, line_h, ascent, descent);
        ctx.draw_text_with_font("no matches", Point::new(px + pad + 8.0, bl), &font, &solid(PALETTE_DESC))?;
        return Ok(());
    }
    for (row, &idx) in shown.iter().enumerate() {
        let y = list_top + row as f32 * line_h;
        let item = &p.all[idx];
        let selected = start + row == p.selected;
        if selected {
            fill_round_rect(ctx, Rect::new(px + pad, y, pw - 2.0 * pad, line_h), 6.0, PALETTE_SEL)?;
        }
        let bl = baseline_in_rect(y, line_h, ascent, descent);
        // Primary display text; secondary detail right-aligned (dim).
        ctx.draw_text_with_font(&item.display, Point::new(px + pad + 8.0, bl), &font, &solid(PALETTE_FG))?;
        if !item.detail.is_empty() {
            let dw = measure(ctx, &item.detail);
            let name_w = measure(ctx, &item.display);
            let dx = px + pw - pad - 8.0 - dw;
            if dx > px + pad + 8.0 + name_w + 16.0 {
                ctx.draw_text_with_font(&item.detail, Point::new(dx, bl), &font, &solid(PALETTE_DESC))?;
            }
        }
    }
    Ok(())
}

/// Top-right find bar: `find: <query>   (i/n)`.
fn draw_search_bar(ctx: &mut dyn DrawingContext, s: &SearchState, font: &Font, width: f32) -> AureaResult<()> {
    let line_h = (font.size * 1.7).max(18.0);
    let m = ctx.measure_text("M", font).ok();
    let ascent = m.as_ref().map(|x| x.ascent).unwrap_or(font.size * 0.8);
    let descent = m.as_ref().map(|x| x.descent).unwrap_or(font.size * 0.2);

    let count = if s.matches.is_empty() {
        if s.query.is_empty() { String::new() } else { "  (no matches)".to_string() }
    } else {
        format!("  ({}/{})", s.current + 1, s.matches.len())
    };
    let text = format!("find: {}{}", s.query, count);
    let text_w = ctx.measure_text(&text, font).map(|m| m.advance).unwrap_or(text.len() as f32 * font.size * 0.6);

    let pad = 10.0;
    let bw = (text_w + pad * 2.0 + 16.0).min(width - 24.0);
    let bx = width - bw - 12.0;
    let by = 10.0;
    fill_round_rect(ctx, Rect::new(bx - 1.0, by - 1.0, bw + 2.0, line_h + 2.0), 9.0, PALETTE_BORDER)?;
    fill_round_rect(ctx, Rect::new(bx, by, bw, line_h), 8.0, PALETTE_BG)?;

    let bl = baseline_in_rect(by, line_h, ascent, descent);
    // Prompt dim, query bright.
    let prompt_w = ctx.measure_text("find: ", font).map(|m| m.advance).unwrap_or(0.0);
    ctx.draw_text_with_font("find: ", Point::new(bx + pad, bl), font, &solid(PALETTE_DESC))?;
    let rest = format!("{}{}", s.query, count);
    ctx.draw_text_with_font(&rest, Point::new(bx + pad + prompt_w, bl), font, &solid(PALETTE_FG))?;
    Ok(())
}

/// Fill a rounded rectangle using a cross of rects plus four corner circles.
fn fill_round_rect(ctx: &mut dyn DrawingContext, rect: Rect, r: f32, color: Color) -> AureaResult<()> {
    let r = r.min(rect.width / 2.0).min(rect.height / 2.0).max(0.0);
    if r <= 0.5 {
        return ctx.draw_rect(rect, &solid(color));
    }
    ctx.draw_rect(Rect::new(rect.x, rect.y + r, rect.width, rect.height - 2.0 * r), &solid(color))?;
    ctx.draw_rect(Rect::new(rect.x + r, rect.y, rect.width - 2.0 * r, rect.height), &solid(color))?;
    ctx.draw_circle(Point::new(rect.x + r, rect.y + r), r, &solid(color))?;
    ctx.draw_circle(Point::new(rect.x + rect.width - r, rect.y + r), r, &solid(color))?;
    ctx.draw_circle(Point::new(rect.x + r, rect.y + rect.height - r), r, &solid(color))?;
    ctx.draw_circle(Point::new(rect.x + rect.width - r, rect.y + rect.height - r), r, &solid(color))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn baseline_in_rect(top: f32, height: f32, ascent: f32, descent: f32) -> f32 {
    top + (height + ascent - descent) / 2.0
}

fn gutter_width(line_count: usize, char_w: f32, mode: LineNumbers) -> f32 {
    if mode == LineNumbers::Off {
        return 0.0;
    }
    let digits = line_count.max(1).to_string().len().max(2);
    GUTTER_MIN_W.max((digits as f32 + 2.0) * char_w + PAD)
}

fn split_rect(rect: Rect, axis: SplitAxis, ratio: f32) -> (Rect, Rect, Rect) {
    let ratio = ratio.clamp(0.1, 0.9);
    match axis {
        SplitAxis::Vertical => {
            let first_w = (rect.width * ratio - SPLIT_GAP / 2.0).max(0.0);
            let divider_x = rect.x + first_w;
            let second_x = divider_x + SPLIT_GAP;
            let second_w = (rect.x + rect.width - second_x).max(0.0);
            (
                Rect::new(rect.x, rect.y, first_w, rect.height),
                Rect::new(second_x, rect.y, second_w, rect.height),
                Rect::new(divider_x, rect.y, SPLIT_GAP, rect.height),
            )
        }
        SplitAxis::Horizontal => {
            let first_h = (rect.height * ratio - SPLIT_GAP / 2.0).max(0.0);
            let divider_y = rect.y + first_h;
            let second_y = divider_y + SPLIT_GAP;
            let second_h = (rect.y + rect.height - second_y).max(0.0);
            (
                Rect::new(rect.x, rect.y, rect.width, first_h),
                Rect::new(rect.x, second_y, rect.width, second_h),
                Rect::new(rect.x, divider_y, rect.width, SPLIT_GAP),
            )
        }
    }
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
            ctx.draw_rect(Rect::new(x, line_top + 1.0, 2.0, line_h - 1.0), &solid(CURSOR_BG))?;
        }
        CursorStyle::Block => {
            ctx.draw_rect(Rect::new(x, line_top + 2.0, char_w.max(6.0), line_h - 3.0), &solid(CURSOR_BG))?;
        }
        CursorStyle::Underline => {
            ctx.draw_rect(Rect::new(x, line_top + line_h - 3.0, char_w.max(6.0), 2.0), &solid(CURSOR_BG))?;
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

fn max_scroll_line(line_count: usize, page_height: usize) -> usize {
    line_count.saturating_sub(page_height.max(1))
}

fn solid(c: Color) -> Paint { Paint::new().color(c).style(PaintStyle::Fill) }
fn stroke(c: Color, w: f32) -> Paint { Paint::new().color(c).style(PaintStyle::Stroke).stroke_width(w) }
