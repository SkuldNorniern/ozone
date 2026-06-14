use std::sync::{Arc, Mutex};

use aurea::render::{Canvas, Image, RendererBackend};
use aurea::{AureaResult, Window};
use ozone_buffer::{BufferId, BufferKind};
use ozone_config::Config;
use ozone_editor::commands::register_defaults;
use ozone_editor::{
    AutocommandRegistry, CommandRegistry, IndentConfig, Keymap, KeymapConflict, KeymapReport,
    KeymapWarning, ModifierMap, NotifyLevel, Workspace, buffer_language,
};

use crate::actions::dispatch_autocmds;
use crate::canvas::{SendableCanvas, SharedCanvas};
use crate::event::{AppState, EventResult, handle_window_event};
use crate::input::ActiveMods;
use crate::keys::{
    active_filetype_name, apply_filetype_config, filetype_config_name, modifier_which_key_entries,
    which_key_entries,
};
use crate::layout::STATUS_H;
use crate::lsp::LspStatus;
use crate::overlay::completion::{CompletionState, draw_completion};
use crate::overlay::minibuffer::{Minibuffer, draw_minibuffer};
use crate::overlay::notify::Notifications;
use crate::overlay::picker::{PickerState, draw_palette};
use crate::overlay::search::SearchState;
use crate::overlay::whichkey::{WhichKeyView, draw_which_key};
use crate::render::draw_editor;
use crate::shell::ShellJobs;
use crate::theme::initialize as initialize_theme;
use crate::{ImageCache, SyntaxCache, TermCells, editor_font, lock};

/// How long a bare modifier must be held alone before the which-key hint shows.
const MOD_HINT_DELAY: std::time::Duration = std::time::Duration::from_millis(400);

/// How long an unfinished chord prefix (e.g. `C-k` waiting for its second
/// stroke) may stay pending before it is auto-cancelled, so a half-typed chord
/// can't trap input. Generous enough to type a deliberate two-stroke chord.
const CHORD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Which single logical modifier is held alone, eligible for the bare-modifier
/// which-key hint. Returns `(control, meta, super_)` with exactly one set, or
/// `None` when zero or several are held. `shift` is ignored (it pairs with a key
/// rather than acting as a chord prefix on its own).
fn bare_modifier_held(active: ActiveMods) -> Option<(bool, bool, bool)> {
    let count = active.control as u8 + active.meta as u8 + active.super_ as u8;
    if count == 1 {
        Some((active.control, active.meta, active.super_))
    } else {
        None
    }
}

/// Decode any newly-opened image buffers into the cache and drop entries for
/// closed buffers. Sets `needs_redraw` when a new image is decoded.
fn sync_images(state: &mut AppState) {
    let ws = lock(state.workspace.as_ref());
    let mut imgs = lock(state.images.as_ref());
    for (id, buf) in ws.buffers.iter() {
        if let BufferKind::Image(path) = &buf.kind
            && !imgs.contains_key(id)
        {
            imgs.insert(*id, decode_image(path));
            state.needs_redraw = true;
        }
    }
    imgs.retain(|id, _| ws.buffers.contains_key(id));
}

/// Apply `[[filetype]]` config to file buffers the first time each is seen,
/// tracking applied buffers so it runs once per buffer.
fn apply_pending_filetypes(state: &mut AppState) {
    if state.config.filetypes.is_empty() {
        return;
    }
    let mut ws = lock(state.workspace.as_ref());
    let pending: Vec<(BufferId, &'static str)> = ws
        .buffers
        .iter()
        .filter(|(id, _)| !state.ft_applied.contains(id))
        .filter_map(|(id, b)| match b.kind {
            BufferKind::File(_) => Some((*id, filetype_config_name(buffer_language(b)))),
            _ => None,
        })
        .collect();
    for (id, ftname) in pending {
        state.ft_applied.insert(id);
        if let Some(fc) = state.config.filetypes.iter().find(|f| f.name == ftname) {
            apply_filetype_config(&mut ws, id, fc);
        }
    }
    state.ft_applied.retain(|id| ws.buffers.contains_key(id));
}

/// Build the which-key view-model for the current frame: the pending-chord
/// continuations, else the bare-modifier hint, else nothing. Suppressed while an
/// overlay (palette/search/minibuffer) is open.
fn compute_which_key(state: &AppState) -> WhichKeyView {
    let overlays_open = lock(state.palette.as_ref()).is_some()
        || lock(state.search.as_ref()).is_some()
        || lock(state.minibuffer.as_ref()).is_some();
    if overlays_open {
        return None;
    }

    let ws = lock(state.workspace.as_ref());
    let ft = active_filetype_name(&ws);

    if !state.chord_pending.is_empty() {
        let entries = which_key_entries(&state.keymap, &state.chord_pending, ft, &state.commands);
        if entries.is_empty() {
            return None;
        }
        let prefix = state
            .chord_pending
            .iter()
            .map(ozone_editor::stroke_label)
            .collect::<Vec<_>>()
            .join(" ");
        return Some((prefix, entries));
    }

    if state.mod_hint_visible {
        let active = ActiveMods::from_physical(state.live_mods, &state.modmap);
        if let Some((control, meta, super_)) = bare_modifier_held(active) {
            let entries = modifier_which_key_entries(
                &state.keymap,
                control,
                meta,
                super_,
                ft,
                &state.commands,
            );
            if !entries.is_empty() {
                return Some((modifier_prefix_label(control, meta, super_), entries));
            }
        }
    }

    None
}

/// Header label for the bare-modifier hint panel (`draw_which_key` appends `-`).
fn modifier_prefix_label(control: bool, meta: bool, super_: bool) -> String {
    if control {
        "C"
    } else if meta {
        "M"
    } else if super_ {
        "s"
    } else {
        ""
    }
    .to_string()
}

pub struct OzoneGui {
    pub(crate) workspace: Arc<Mutex<Workspace>>,
    pub(crate) commands: Arc<CommandRegistry>,
    pub(crate) config: Arc<Config>,
    pub(crate) autocmds: Arc<AutocommandRegistry>,
    pub(crate) keymap: Arc<Keymap>,
    keymap_report: KeymapReport,
    unknown_commands: Vec<String>,
    /// Complete bindings shadowed by a longer chord sharing their prefix, so
    /// the shorter command can never fire. Reported at startup.
    shadowed_chords: Vec<String>,
    /// Command ids registered more than once (later handler won). Reported at
    /// startup; should always be empty in a clean build.
    duplicate_commands: Vec<String>,
    /// Warnings produced before the GUI exists (e.g. config read/parse issues
    /// from `Config::load_user_with_warning`), shown as startup toasts so a
    /// windowed release build — which has no console for stderr — still sees them.
    startup_warnings: Vec<String>,
    pub(crate) modmap: ModifierMap,
}

/// Push the keymap validation diagnostics (conflicts / dropped bindings /
/// unknown commands / prefix-shadowed bindings) as toasts. Shared by startup
/// and `config.reload` so both report the same problems the same way.
fn push_keymap_diagnostics(
    notes: &mut Notifications,
    report: &KeymapReport,
    unknown_commands: &[String],
    shadowed_chords: &[String],
) {
    if !report.conflicts.is_empty() {
        let details = report
            .conflicts
            .iter()
            .map(KeymapConflict::message)
            .collect::<Vec<_>>()
            .join("\n");
        notes.push(
            NotifyLevel::Error,
            format!("Duplicate keybindings detected:\n{details}"),
            Some(15_000),
        );
    }
    if !report.warnings.is_empty() {
        let details = report
            .warnings
            .iter()
            .map(KeymapWarning::message)
            .collect::<Vec<_>>()
            .join("\n");
        notes.push(
            NotifyLevel::Warn,
            format!("Some keybindings were ignored:\n{details}"),
            Some(15_000),
        );
    }
    if !unknown_commands.is_empty() {
        let details = unknown_commands.join("\n");
        notes.push(
            NotifyLevel::Warn,
            format!("Keybindings point to unknown commands:\n{details}"),
            Some(15_000),
        );
    }
    if !shadowed_chords.is_empty() {
        let details = shadowed_chords.join("\n");
        notes.push(
            NotifyLevel::Warn,
            format!(
                "These bindings can never fire — a longer chord shares their prefix:\n{details}"
            ),
            Some(15_000),
        );
    }
}

impl OzoneGui {
    pub fn new(workspace: Workspace) -> Self {
        Self::with_config(workspace, Config::default_config())
    }

    pub fn with_config(mut workspace: Workspace, config: Config) -> Self {
        initialize_theme(&config.theme);
        workspace.indent = IndentConfig {
            width: config.editor.tab_width,
            soft_tabs: config.editor.soft_tabs,
        };

        let mut reg = CommandRegistry::new();
        register_defaults(&mut reg);
        // A command id registered twice silently loses its first handler — a
        // programming bug (guarded by a test for built-ins, surfaced here for
        // any future runtime/plugin registration).
        let duplicate_commands = reg.duplicate_registrations().to_vec();
        let autocmds = AutocommandRegistry::from_config(&config.autocmds);
        dispatch_autocmds(&mut workspace, &reg, &autocmds, &mut ShellJobs::new());

        // Keybindings are config-driven, not hardcoded: every binding (including
        // the shipped defaults) comes from the config's `[keymap]` / keymap.toml,
        // which is generated on first launch. Removing a binding there unbinds
        // it — there is no built-in default layer underneath.
        let mut keymap = Keymap::new();
        let keymap_report = keymap.add_user_config(&config.keymaps);
        // A binding target is valid if it's a registered command or a shell
        // sigil (`|cmd` filter / `!cmd` run-on-file), which `actions` dispatches
        // without the registry. Anything else is a typo that binds a dead key.
        let unknown_commands = keymap.unknown_commands(|name| {
            name.starts_with('|') || name.starts_with('!') || reg.contains(name)
        });
        let shadowed_chords = keymap.shadowed_by_longer_chord();

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
            keymap_report,
            unknown_commands,
            shadowed_chords,
            duplicate_commands,
            startup_warnings: Vec::new(),
            modmap,
        }
    }

    /// Attach warnings gathered before the GUI was constructed (e.g. config
    /// load/parse problems). They are shown as startup toasts in [`Self::run`].
    /// Empty strings are ignored.
    pub fn with_startup_warnings(mut self, warnings: impl IntoIterator<Item = String>) -> Self {
        self.startup_warnings
            .extend(warnings.into_iter().filter(|w| !w.trim().is_empty()));
        self
    }

    pub fn run(self) -> AureaResult<()> {
        const W: u32 = 1280;
        const H: u32 = 800;

        let mut window = Window::new("Ozone", W as i32, H as i32)?;
        set_window_icon(&window);

        let palette: Arc<Mutex<Option<PickerState>>> = Arc::new(Mutex::new(None));
        let search: Arc<Mutex<Option<SearchState>>> = Arc::new(Mutex::new(None));
        let minibuffer: Arc<Mutex<Option<Minibuffer>>> = Arc::new(Mutex::new(None));
        let completion: Arc<Mutex<Option<CompletionState>>> = Arc::new(Mutex::new(None));
        let mut startup_notifications = Notifications::new();
        for warning in &self.startup_warnings {
            startup_notifications.push(NotifyLevel::Warn, warning.clone(), Some(15_000));
        }
        if self.config.keymaps.is_empty() {
            // No `[keymap]`/keymap.toml means every key (Ctrl/Meta chords
            // included) is unbound — config-driven keymaps have no hardcoded
            // fallback. Surface this loudly; `--reset-config` regenerates the
            // default keymap.toml.
            startup_notifications.push(
                NotifyLevel::Warn,
                "No keybindings configured — Ctrl/Meta shortcuts are unbound. \
                 Run `ozone --reset-config` to regenerate the default keymap."
                    .to_string(),
                Some(15_000),
            );
        }
        push_keymap_diagnostics(
            &mut startup_notifications,
            &self.keymap_report,
            &self.unknown_commands,
            &self.shadowed_chords,
        );
        if !self.duplicate_commands.is_empty() {
            let details = self.duplicate_commands.join("\n");
            startup_notifications.push(
                NotifyLevel::Error,
                format!("Commands registered more than once (later handler won):\n{details}"),
                Some(15_000),
            );
        }
        let notifications: Arc<Mutex<Notifications>> = Arc::new(Mutex::new(startup_notifications));
        let which_key: Arc<Mutex<WhichKeyView>> = Arc::new(Mutex::new(None));
        let images: Arc<Mutex<ImageCache>> = Arc::new(Mutex::new(ImageCache::new()));

        let raw_canvas = Canvas::new(W, H, RendererBackend::Cpu)?;
        let workspace_for_draw = self.workspace.clone();
        let config_for_draw = self.config.clone();
        let commands_for_draw = self.commands.clone();
        let keymap_for_draw = self.keymap.clone();
        let palette_for_draw = palette.clone();
        let search_for_draw = search.clone();
        let minibuffer_for_draw = minibuffer.clone();
        let completion_for_draw = completion.clone();
        let notifications_for_draw = notifications.clone();
        let which_key_for_draw = which_key.clone();
        let images_for_draw = images.clone();
        let callback_syntax_cache = Mutex::new(SyntaxCache::new());

        raw_canvas.set_draw_callback(move |ctx| {
            let pal = lock(palette_for_draw.as_ref());
            let srch = lock(search_for_draw.as_ref());
            let mb = lock(minibuffer_for_draw.as_ref());
            let comp = lock(completion_for_draw.as_ref());
            let notes = lock(notifications_for_draw.as_ref());
            let mut ws = lock(workspace_for_draw.as_ref());
            let imgs = lock(images_for_draw.as_ref());

            let mut scratch_char_w = 0.0;
            let welcome_bindings = welcome_keymap_rows(&keymap_for_draw, &commands_for_draw);
            draw_editor(
                ctx,
                &mut ws,
                &config_for_draw,
                &welcome_bindings,
                srch.as_ref(),
                &TermCells::new(),
                &imgs,
                &mut lock(&callback_syntax_cache),
                ActiveMods::default(),
                true,
                LspStatus::Idle,
                &mut scratch_char_w,
            )?;
            if let Some(p) = pal.as_ref() {
                draw_palette(ctx, p, &config_for_draw)?;
            }
            if let Some(m) = mb.as_ref() {
                let f = editor_font(&config_for_draw);
                let (cw, ch) = (ctx.width() as f32, ctx.height() as f32);
                draw_minibuffer(ctx, m, &f, cw, ch, STATUS_H)?;
            }
            if let Some(c) = comp.as_ref() {
                draw_completion(ctx, c, &config_for_draw)?;
            }
            // Which-key panel: the frame scheduler presents this callback, so the
            // panel must be drawn here (not only in the loop's `canvas.draw`).
            if let Some((prefix, entries)) = lock(which_key_for_draw.as_ref()).as_ref()
                && !entries.is_empty()
            {
                let f = editor_font(&config_for_draw);
                let (cw, ch) = (ctx.width() as f32, ctx.height() as f32);
                draw_which_key(ctx, prefix, entries, &f, cw, ch)?;
            }
            if !notes.is_empty() {
                let f = editor_font(&config_for_draw);
                let (cw, ch) = (ctx.width() as f32, ctx.height() as f32);
                notes.draw(ctx, &f, cw, ch)?;
            }
            Ok(())
        })?;

        let canvas_arc = Arc::new(Mutex::new(SendableCanvas(raw_canvas)));
        window.set_content(SharedCanvas(canvas_arc.clone()))?;

        {
            let mut canvas = lock(canvas_arc.as_ref());
            let mut ws = lock(self.workspace.as_ref());
            let config = self.config.clone();
            let welcome_bindings = welcome_keymap_rows(&self.keymap, &self.commands);
            let mut scratch_char_w = 0.0;
            let mut init_syntax_cache = SyntaxCache::new();
            canvas.draw(|ctx| {
                draw_editor(
                    ctx,
                    &mut ws,
                    &config,
                    &welcome_bindings,
                    None,
                    &TermCells::new(),
                    &ImageCache::new(),
                    &mut init_syntax_cache,
                    ActiveMods::default(),
                    true,
                    LspStatus::Idle,
                    &mut scratch_char_w,
                )
            })?;
            canvas.invalidate_all();
        }

        let mut state = AppState::new(
            self,
            palette,
            search,
            minibuffer,
            notifications,
            completion,
            which_key,
            canvas_arc,
            images,
            W,
            H,
        );
        let blink_interval = std::time::Duration::from_millis(530);

        loop {
            unsafe { aurea::ffi::ng_platform_poll_events() };

            {
                let ws = lock(state.workspace.as_ref());
                if let Some(active) = ws.active_view().map(|v| v.buffer_id)
                    && state.buffer_mru.first() != Some(&active)
                {
                    state.buffer_mru.retain(|id| *id != active);
                    state.buffer_mru.insert(0, active);
                }
                state.buffer_mru.retain(|id| ws.buffers.contains_key(id));
            }

            let events = window.poll_events();
            let has_text_input = events.iter().any(|event| {
                matches!(event, aurea::WindowEvent::TextInput { text } if text.chars().any(|c| !c.is_control()))
            });
            state.begin_event_batch(has_text_input);
            let mut should_close = false;
            for event in &events {
                if matches!(handle_window_event(event, &mut state), EventResult::Close) {
                    should_close = true;
                }
            }
            if should_close {
                break;
            }

            // `config.reload` requested a rebuild of keymaps/modifiers/autocmds.
            if lock(state.workspace.as_ref()).take_config_reload() {
                state.reload_config();
            }

            if state.take_cursor_activity() {
                state.cursor_visible = true;
                state.last_cursor_blink = std::time::Instant::now();
            } else if state.last_cursor_blink.elapsed() >= blink_interval {
                state.cursor_visible = !state.cursor_visible;
                state.last_cursor_blink = std::time::Instant::now();
                state.needs_redraw = true;
            }

            // --- terminal sync ---
            {
                let mut ws = lock(state.workspace.as_ref());
                if state.terms.sync(
                    &mut ws,
                    &state.config,
                    state.window_width,
                    state.window_height,
                    state.measured_char_w,
                ) {
                    state.needs_redraw = true;
                }
            }

            sync_images(&mut state);
            apply_pending_filetypes(&mut state);

            if lock(state.notifications.as_ref()).tick() {
                state.needs_redraw = true;
            }

            // --- LSP sync: lazily start the server, mirror open Rust buffers,
            // and route diagnostics into the decoration store. ---
            {
                let mut ws = lock(state.workspace.as_ref());
                if state.lsp.sync(&mut ws, &state.config) {
                    state.needs_redraw = true;
                }
            }
            if let Some(result) = state.lsp.take_completion_result() {
                *lock(state.completion.as_ref()) = Some(CompletionState::new(result));
                state.needs_redraw = true;
            }

            // --- Shell job poll: apply any finished `!cmd`/`|cmd` autocommand
            // results (buffer reload/replace + notification) without blocking. ---
            {
                let mut ws = lock(state.workspace.as_ref());
                if state.shell_jobs.poll(&mut ws) {
                    state.needs_redraw = true;
                }
            }

            // Idle-chord timeout: a prefix left pending too long is cancelled so
            // half-typed chords can't trap input. The clock restarts whenever the
            // prefix grows (its length changes), tracked via `chord_pending_seen`.
            {
                if state.chord_pending.is_empty() {
                    state.chord_pending_since = None;
                } else if state.chord_pending.len() != state.chord_pending_seen
                    || state.chord_pending_since.is_none()
                {
                    state.chord_pending_since = Some(std::time::Instant::now());
                }
                state.chord_pending_seen = state.chord_pending.len();

                if let Some(since) = state.chord_pending_since
                    && since.elapsed() >= CHORD_TIMEOUT
                {
                    state.chord_pending.clear();
                    state.chord_pending_since = None;
                    state.chord_pending_seen = 0;
                    state.needs_redraw = true;
                }
            }

            // Bare-modifier which-key hint: holding Ctrl/Meta alone (no pending
            // chord) reveals its bindings after a short delay, so quick chords
            // like C-s don't flash the panel. Recomputed each frame; flips drive
            // a redraw. Overlay suppression is handled at draw time.
            {
                let active = ActiveMods::from_physical(state.live_mods, &state.modmap);
                let eligible =
                    state.chord_pending.is_empty() && bare_modifier_held(active).is_some();
                let now_visible = if eligible {
                    let start = *state
                        .mod_hint_start
                        .get_or_insert_with(std::time::Instant::now);
                    start.elapsed() >= MOD_HINT_DELAY
                } else {
                    state.mod_hint_start = None;
                    false
                };
                if now_visible != state.mod_hint_visible {
                    state.mod_hint_visible = now_visible;
                    state.needs_redraw = true;
                }
            }

            // Recompute the which-key view-model and publish it for the draw
            // callback. A change forces a redraw so the panel appears/updates/
            // disappears promptly.
            {
                let view = compute_which_key(&state);
                let changed = *lock(state.which_key.as_ref()) != view;
                if changed {
                    *lock(state.which_key.as_ref()) = view;
                    state.needs_redraw = true;
                }
            }

            {
                let ws = lock(state.workspace.as_ref());
                let title = window_title(&ws);
                if title != state.last_title {
                    let _ = window.set_title(&title);
                    state.last_title = title;
                }
            }

            if state.needs_redraw {
                let pal = lock(state.palette.as_ref());
                let srch = lock(state.search.as_ref());
                let mb = lock(state.minibuffer.as_ref());
                let comp = lock(state.completion.as_ref());
                let notes = lock(state.notifications.as_ref());
                let mut ws = lock(state.workspace.as_ref());
                let mut canvas = lock(state.canvas.as_ref());
                let imgs = lock(state.images.as_ref());
                let config = state.config.clone();
                let active_mods = ActiveMods::from_physical(state.live_mods, &state.modmap);
                let frame_wk = lock(state.which_key.as_ref()).clone();
                let welcome_bindings = welcome_keymap_rows(&state.keymap, &state.commands);
                canvas.draw(|ctx| {
                    draw_editor(
                        ctx,
                        &mut ws,
                        &config,
                        &welcome_bindings,
                        srch.as_ref(),
                        &state.terms.cells,
                        &imgs,
                        &mut state.syntax_cache,
                        active_mods,
                        state.cursor_visible,
                        state.lsp.status(),
                        &mut state.measured_char_w,
                    )?;
                    if let Some(p) = pal.as_ref() {
                        draw_palette(ctx, p, &config)?;
                    }
                    if let Some(m) = mb.as_ref() {
                        let f = editor_font(&config);
                        let (cw, ch) = (ctx.width() as f32, ctx.height() as f32);
                        draw_minibuffer(ctx, m, &f, cw, ch, STATUS_H)?;
                    }
                    if let Some(c) = comp.as_ref() {
                        draw_completion(ctx, c, &config)?;
                    }
                    if let Some((prefix, entries)) = frame_wk.as_ref()
                        && !entries.is_empty()
                    {
                        let f = editor_font(&config);
                        let (cw, ch) = (ctx.width() as f32, ctx.height() as f32);
                        draw_which_key(ctx, prefix, entries, &f, cw, ch)?;
                    }
                    if !notes.is_empty() {
                        let f = editor_font(&config);
                        let (cw, ch) = (ctx.width() as f32, ctx.height() as f32);
                        notes.draw(ctx, &f, cw, ch)?;
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

impl AppState {
    /// Re-read the user config from disk and rebuild the keymap, modifier map,
    /// and autocommands as one atomic swap, re-running keymap validation and
    /// surfacing its diagnostics. Triggered by the `config.reload` command.
    ///
    /// Appearance (font/theme/render) is intentionally **not** reloaded here:
    /// the draw callback captured its own config/keymap clones at startup, so
    /// those still need a restart. This covers the input/behavior config the
    /// event loop reads live.
    pub(crate) fn reload_config(&mut self) {
        let (config, config_warning) = Config::load_user_with_warning();

        let mut keymap = Keymap::new();
        let report = keymap.add_user_config(&config.keymaps);
        let unknown_commands = keymap.unknown_commands(|name| {
            name.starts_with('|') || name.starts_with('!') || self.commands.contains(name)
        });
        let shadowed_chords = keymap.shadowed_by_longer_chord();
        let modmap = ModifierMap::platform_default().with_overrides(
            config.modifiers.control.as_deref(),
            config.modifiers.meta.as_deref(),
            config.modifiers.super_.as_deref(),
        );
        let autocmds = AutocommandRegistry::from_config(&config.autocmds);

        // Atomic swap: nothing reads a half-updated keymap/config.
        self.modmap = modmap;
        self.keymap = Arc::new(keymap);
        self.autocmds = Arc::new(autocmds);
        self.config = Arc::new(config);
        // A pending chord prefix referred to the old bindings — abandon it.
        self.chord_pending.clear();
        self.chord_pending_since = None;
        self.chord_pending_seen = 0;
        // Indent is a behavioral editor setting the event path reads live.
        {
            let mut ws = lock(self.workspace.as_ref());
            ws.indent = IndentConfig {
                width: self.config.editor.tab_width,
                soft_tabs: self.config.editor.soft_tabs,
            };
        }

        let mut notes = lock(self.notifications.as_ref());
        notes.push(
            NotifyLevel::Success,
            "Config reloaded — keymaps, modifiers, and autocommands updated \
             (appearance changes need a restart)"
                .to_string(),
            Some(6_000),
        );
        if let Some(w) = config_warning {
            notes.push(NotifyLevel::Warn, format!("Config: {w}"), Some(15_000));
        }
        push_keymap_diagnostics(&mut notes, &report, &unknown_commands, &shadowed_chords);
        drop(notes);
        self.needs_redraw = true;
    }
}

fn welcome_keymap_rows(keymap: &Keymap, commands: &CommandRegistry) -> Vec<(String, String)> {
    keymap
        .display_bindings(None, 6)
        .into_iter()
        .map(|(key, command)| (key, commands.display_name(&command)))
        .collect()
}

fn window_title(ws: &Workspace) -> String {
    match ws.active_buffer() {
        Some(buf) => {
            let dirty = if buf.is_dirty() { "*" } else { "" };
            match &buf.kind {
                BufferKind::File(p) | BufferKind::Image(p) => {
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                    format!("Ozone - {}{}", dirty, name)
                }
                BufferKind::Scratch => format!("Ozone - {}scratch", dirty),
                BufferKind::Search => format!("Ozone - {}files", dirty),
                BufferKind::References => format!("Ozone - {}references", dirty),
                BufferKind::FileTree => format!("Ozone - {}tree", dirty),
                BufferKind::Terminal => format!("Ozone - {}terminal", dirty),
            }
        }
        None => "Ozone".to_string(),
    }
}

fn decode_image(path: &std::path::Path) -> Option<Image> {
    // Sniff the format from the content rather than the extension, so a
    // mislabeled or extensionless file still decodes when it's a supported type.
    let rgba = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?
        .to_rgba8();
    let (w, h) = rgba.dimensions();
    Some(Image::new(w, h, rgba.into_raw()))
}

fn set_window_icon(window: &Window) {
    let Ok(image) = image::load_from_memory(include_bytes!("../../assets/icon.png")) else {
        return;
    };
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    let _ = window.set_icon_rgba(rgba.as_raw(), width, height);
}
