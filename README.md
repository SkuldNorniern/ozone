# Ozone

Ozone is a small Rust editor built around explicit editor, buffer, syntax, and
GUI crates. It is a plain text editor first: no Vim-style modes, no hidden
global editor state, and no large plugin runtime while the architecture is still
settling.

![Ozone editor screenshot](assets/screenshots/ozone-editor.png)

## Current Shape

Ozone is early, but the desktop GUI path is usable for scratch editing,
navigation, and small project work:

- scratch-buffer launch screen with a sample of the active keymap
- editable text buffers with undo, redo, dirty tracking, and save/save-all
- command-line file opening
- workspace file picker, open-buffer picker, and file tree
- in-buffer search/replace and literal workspace search
- line numbers, cursor-line highlight, selections, scrollbars, and mouse-wheel scrolling
- pane splits, pane focus movement, and buffer cycling
- syntax scanning for Rust, Markdown, TOML, Oxygen, and plain text
- PNG/JPEG image preview buffers
- configurable themes, keymaps, filetype defaults, modifier maps, and autocommands

Some pieces are intentionally still thin. Terminal buffers currently render a
placeholder surface, and LSP configuration is parsed but the LSP runtime remains
deferred.

## Build

Ozone is a Cargo workspace. The local Aurea GUI library is expected at
`./aurea/crates/aurea`, which is how the workspace dependency is wired today.

Build the editor from the repository root:

```sh
cargo build --release
```

For faster development checks:

```sh
cargo check
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
| `Ctrl+S` / `Cmd+S` | Save current buffer |
| `Ctrl+K Ctrl+S` | Save all buffers |
| `Ctrl+Z` / `Cmd+Z` | Undo |
| `Ctrl+Y` / `Cmd+Shift+Z` | Redo |
| `Ctrl+P` / `Cmd+P` | Open workspace file picker |
| `Ctrl+X B` | Open buffer picker |
| `Meta+X` or `Ctrl+Shift+P` / `Cmd+Shift+P` | Open command palette |
| `Ctrl+Shift+E` / `Cmd+Shift+E` | Open file tree |
| `Meta+F` | Search current buffer |
| `Meta+H` | Search and replace current buffer |
| `Ctrl+Shift+F` / `Cmd+Shift+F` | Search workspace |
| `Meta+G` | Go to line |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / previous buffer |
| `Ctrl+Shift+Right` / `Ctrl+Shift+Down` | Split pane right / down |
| `Ctrl+Shift+W` | Close active pane |
| `Ctrl+Meta+Arrow` | Focus pane in that direction |
| `Ctrl+-` / `Ctrl+=` | Jump back / forward |
| Mouse wheel | Scroll |

On macOS, `Cmd` maps to Ozone's `super` modifier. The `control`, `meta`, and
`super` mappings can be changed in config.

## Configuration

Ozone loads user configuration from:

- Windows: `%APPDATA%\ozone\config.toml`
- Linux/macOS: `$XDG_CONFIG_HOME/ozone/config.toml`, or `~/.config/ozone/config.toml`

Every field is optional. Missing or malformed values fall back to defaults
instead of preventing the editor from starting.

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

[[keymap]]
keys = "ctrl+shift+p"
command = "command.palette"

[[filetype]]
name = "markdown"
word_wrap = true
tab_width = 2

[[autocmd]]
event = "buffer.pre-save"
pattern = "*"
command = "edit.trim-trailing-whitespace"
```

Bundled themes currently include `brewery-stout`, `brewery-wine`, and
`catppuccin-mocha`. A theme can also be selected by path.

## Workspace Layout

- `src/`: executable entry point
- `ozone-buffer/`: text storage, positions, edits, undo/redo, dirty state, and persistence
- `ozone-editor/`: workspace state, views, commands, keymaps, events, and UI intents
- `ozone-gui/`: Aurea-based drawing, overlays, input routing, and window integration
- `ozone-syntax/`: lightweight fallback syntax scanning
- `ozone-config/`: hand-shaped configuration loading and validation
- `ozone-term/`: terminal grid and PTY support under construction
- `themes/`: bundled color themes
- `packaging/`: platform packaging metadata and icons
- `aurea/`: local GUI toolkit dependency used by this workspace

## Notes

Ozone avoids a large dependency stack while the editor model is still in motion.
The config parser uses `toml`, but editor behavior stays in explicit Rust domain
types instead of generated Serde models.
