# Ozone

[![CI](https://github.com/SkuldNorniern/ozone/actions/workflows/ci.yml/badge.svg)](https://github.com/SkuldNorniern/ozone/actions/workflows/ci.yml)
[![Nightly](https://github.com/SkuldNorniern/ozone/actions/workflows/nightly.yml/badge.svg)](https://github.com/SkuldNorniern/ozone/actions/workflows/nightly.yml)
[![Latest Release](https://img.shields.io/github/v/release/SkuldNorniern/ozone?include_prereleases&label=release)](https://github.com/SkuldNorniern/ozone/releases/latest)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

Ozone is a small Rust editor built around explicit editor, buffer, syntax, and
GUI crates. It is made to fit my own editor taste first, while keeping enough
configuration surface for others to reshape keymaps, themes, filetype behavior,
and UI defaults. It is a plain text editor first: no Vim-style modes, no hidden
global editor state, and no large plugin runtime while the architecture is still
settling.

![Ozone editor screenshot](assets/screenshots/ozone-editor.png)
 
## Downloads

Pre-built binaries are published automatically:

| Channel | Trigger | Platforms |
| --- | --- | --- |
| **Nightly** | Every push to `main` | Linux x64 · macOS ARM · macOS x64 · Windows x64 |
| **Release** | `v*` tag | Linux x64 · macOS ARM · macOS x64 · Windows x64 |

→ [All releases](https://github.com/SkuldNorniern/ozone/releases)

## Current Shape

Ozone is early, but the desktop GUI path is usable for scratch editing,
navigation, and small project work:

- scratch-buffer launch screen with a sample of the active keymap
- editable text buffers with undo, redo, dirty tracking, and save/save-all
- LF/CRLF aware: files load as LF internally and keep their original ending on save
- command-line file opening
- workspace file picker, open-buffer picker, and a collapsible, icon-rich file tree
- in-buffer search/replace and literal workspace search
- code folding, text objects, and a document-symbol picker
- keyboard and mouse selection, OS clipboard copy/cut/paste, and select-all
- comment toggling, line duplication, and moving lines or selected blocks
- line numbers, cursor-line highlight, scrollbars, and mouse-wheel scrolling
- pane splits, pane focus movement, buffer cycling, and status-bar buffer dots (click to switch)
- which-key hints — for a pending chord *and* for a bare held modifier
- syntax scanning for Rust, Markdown, TOML, JSON, and plain text
- PTY-backed terminal buffers with a colour VT grid
- image preview buffers (PNG/JPEG/GIF/WebP/BMP/ICO/TGA/PNM/QOI/farbfeld)
- format-on-save and text filters via shell commands (`|cmd` pipe, `!cmd` run-on-file)
- live LSP diagnostics (underline + gutter sign + message) for Rust via rust-analyzer
- configurable themes, keymaps, filetype defaults, modifier maps, and autocommands

Some pieces are intentionally still thin. The LSP client streams diagnostics and
supports completion, hover, and go-to-definition; references, rename, and richer
completion edits are still pending. The editor stays fully useful without a
server. Plugin capability is planned for later, once the command, event, and
configuration surfaces are stable enough to expose cleanly.

## Build

Ozone is a Cargo workspace. All dependencies, including the
[Aurea](https://crates.io/crates/aurea) GUI toolkit, are pulled from crates.io.

```sh
cargo build --release
```

For faster development checks:

```sh
cargo check
```

### Linux

A few system libraries are required for the Vulkan/X11 backend:

```sh
sudo apt-get install -y libvulkan-dev libxcb-xfixes0-dev libxcb-shape0-dev \
  libxkbcommon-dev pkg-config libegl-mesa0
```

## Run

Open the welcome/scratch buffer:

```sh
cargo run --release
```

Open a file:

```sh
cargo run --release -- README.md
```

The debug binary works, but manual UI testing is usually more representative
with the release binary.

## Editing

Ozone is always in edit mode. Click the window if focus is elsewhere, then type
normally. The status line shows the active buffer, dirty marker, cursor position,
pane count, and active modifier state.

```text
*scratch*    1:1    pane 1/1
```

An asterisk after the buffer name means the buffer has unsaved changes. The
window title also carries the dirty marker for file buffers.

## Default Keys

| Key | Action |
| --- | --- |
| Text input | Insert text at cursor |
| `Enter` | Insert newline with indentation |
| `Tab` | Insert one configured indent unit |
| `Backspace` / `Delete` | Delete backward / forward |
| Arrow keys | Move cursor |
| `Home` / `End` | Move to start/end of line |
| `PageUp` / `PageDown` | Move and scroll one page |
| `Ctrl+Left` / `Ctrl+Right` | Move by word |
| `Ctrl+Home` / `Ctrl+End` | Move to start/end of file |
| `Ctrl+A` / `Ctrl+E` | Move to start/end of line |
| `Ctrl+B` / `Ctrl+F` / `Ctrl+P` / `Ctrl+N` | Emacs-style left/right/up/down |
| `Ctrl+S` | Save current buffer |
| `Ctrl+K Ctrl+S` | Save all buffers |
| `Ctrl+Z` / `Ctrl+Y` | Undo / Redo |
| `Ctrl+C` / `Ctrl+X` / `Ctrl+V` | Copy / cut / paste using the OS clipboard |
| `Shift+Arrow` | Extend or retract the selection |
| `Ctrl+Shift+Left` / `Ctrl+Shift+Right` | Extend selection by word |
| `Ctrl+Shift+Home` / `Ctrl+Shift+End` | Extend selection to start/end of file |
| `Ctrl+/` | Toggle line comments for the line or selection |
| `Ctrl+D` | Duplicate the line or selected lines |
| `Meta+Up` / `Meta+Down` | Move the line or selected lines |
| `Ctrl+P` | Open workspace file picker |
| `Meta+O` | Open a file via the native OS file picker |
| `Meta+Shift+O` | Open a folder as the workspace via the native OS folder picker |
| `Meta+B` | Open buffer picker |
| `Ctrl+Shift+O` | Go to symbol (current buffer) |
| `Meta+X` or `Ctrl+Shift+P` | Open command palette |
| `Ctrl+Shift+E` | Open file tree |
| `Meta+F` | Search current buffer |
| `Meta+H` | Search and replace current buffer |
| `Ctrl+Shift+F` | Search workspace |
| `Meta+G` | Go to line |
| `Meta+.` / `Meta+K` / `Ctrl+Space` | LSP definition / hover / completion |
| `Ctrl+K Ctrl+L` | Toggle fold at cursor |
| `Ctrl+K Ctrl+J` / `Ctrl+K Ctrl+0` | Fold all / open all |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / previous buffer |
| `Ctrl+W Right` / `Ctrl+W Down` | Split pane right / down |
| `Ctrl+W C` | Close active pane |
| `Ctrl+Meta+Arrow` | Focus pane in that direction |
| `Ctrl+-` / `Ctrl+=` | Jump back / forward |
| Mouse wheel | Scroll |

These bindings aren't hardcoded — they're written into `keymap.toml` on first
launch and the running keymap is built entirely from config. Edit a command to
rebind a key, or delete a line to unbind it (yes, even a default). See
[Configuration](#configuration).

Modifiers follow Emacs naming: **Control** is Ctrl, **Meta** is Alt on
Windows/Linux and Command on macOS, and **Super** is the Win key or Option.
`Meta+X` opens the palette on Windows/Linux; macOS uses `Cmd+X` for cut, so
`Ctrl+Shift+P` remains the portable palette binding. All mappings can be
reassigned in `[modifiers]` config.

## Configuration

Ozone loads user configuration from:

- Windows: `%APPDATA%\ozone\config.toml`
- Linux/macOS: `$XDG_CONFIG_HOME/ozone/config.toml`, or `~/.config/ozone/config.toml`

If no config file exists, a default template is written automatically on first
launch. Every field is optional; missing or malformed values fall back silently
to defaults.

```toml
[editor]
font = "Consolas"
font_size = 13
line_height = 1.4
tab_width = 4
soft_tabs = true
line_numbers = "relative"
cursor_style = "bar"
scroll_off = 8
word_wrap = false
trim_trailing_whitespace = true
auto_save = false
auto_format = false
jump_list_size = 100

[theme]
name = "brewery-stout"

[ui]
mouse = false

# Keybindings layer over the defaults. Compact form: one line per binding.
# A [keymap.<filetype>] table scopes binds to that filetype. (The verbose
# [[keymap]] array form, with an optional `filetype` field, also works.)
[keymap]
"ctrl+shift+p" = "command.palette"

[keymap.rust]
"ctrl+shift+f" = "lsp.format"

[[filetype]]
name = "markdown"
word_wrap = true
tab_width = 2

[[autocmd]]
event = "buffer.pre-save"
pattern = "*"
command = "edit.trim-trailing-whitespace"

# Format on save. A command beginning with `|` pipes the buffer through a
# stdin/stdout tool (use on buffer.pre-save); one beginning with `!` runs a tool
# that edits the file on disk, then reloads (use on buffer.saved; `%` = path).
[[autocmd]]
event = "buffer.saved"
pattern = "*.rs"
command = "!cargo fmt"        # or "|rustfmt --edition 2021" on buffer.pre-save
```

Bundled themes: `brewery-stout`, `brewery-wine`, `catppuccin-mocha`. A theme can
also be a path to a custom `.toml` file.

### Splitting the config

A large config can be split by concern. Files placed next to `config.toml` are
merged into it, using the same block syntax they would have inline:

```text
~/.config/ozone/
  config.toml      # [editor], [theme], [ui], …
  keymap.toml      # [keymap] / [[keymap]]
  autocmd.toml     # [[autocmd]]
  filetype.toml    # [[filetype]]
  lsp.toml         # [[lsp]]
```

If a section file exists, it **owns** that section — any matching
`[keymap]`/`[[keymap]]`, `[[autocmd]]`, `[[filetype]]`, or `[[lsp]]` block left
in `config.toml` is ignored, not merged with it. This keeps each concern in one
place: edit `keymap.toml` for keybindings, `config.toml` for everything else.
(Themes already live as separate files under `themes/`.)

The full split layout (`config.toml` plus all four section files) is generated
on first launch. If you have a `config.toml` from before this split — no
`[keymap]` at all — Ozone won't silently rewrite it, but every key (including
Ctrl/Meta chords) will be unbound, and it warns on startup. Run:

```sh
ozone --reset-config
```

to regenerate `config.toml` and all section files from the shipped defaults.
This **overwrites** your existing config, so back it up first if you've
customized it.

## Language Server (optional)

Ozone ships a dependency-free LSP client. With a `[[lsp]]` block for a language
and that server on your `PATH`, Ozone starts it the first time you open a
matching file and shows live diagnostics (underline + gutter sign + end-of-line
message). The editor stays fully usable without a server; if the server can't
start, Ozone warns once and carries on.

```toml
[[lsp]]
language = "rust"
server   = "rust-analyzer"   # must be on PATH (e.g. `rustup component add rust-analyzer`)
lazy     = true
```

Completion, hover, and go-to-definition are wired. References and rename remain
planned.

## Workspace Layout

| Crate | Role |
| --- | --- |
| `src/` | Executable entry point |
| `ozone-buffer/` | Text storage, positions, edits, undo/redo, dirty state, persistence |
| `ozone-editor/` | Workspace, views, commands, keymaps, events, UI intents |
| `ozone-gui/` | Aurea-based drawing, overlays, input routing, window integration |
| `ozone-syntax/` | Line-scanner syntax highlighting, symbol extraction, filetype detection (via `taste`) |
| `ozone-config/` | Configuration loading and validation |
| `ozone-term/` | PTY-backed terminal with a colour VT grid emulator |
| `ozone-lsp/` | Dependency-free JSON-RPC LSP client: handshake, reader thread, live diagnostics |
| `themes/` | Bundled color themes |
| `packaging/` | Platform packaging metadata and icons |

## Notes

Ozone avoids a large dependency stack while the editor model is still in motion.
The config parser uses `toml`, but editor behavior stays in explicit Rust domain
types instead of generated Serde models.
