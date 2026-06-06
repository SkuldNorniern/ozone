# Ozone

Ozone is a small editor experiment written in Rust. It uses the local Aurea GUI
library, keeps editing state in explicit buffer/editor crates, and starts as a
plain non-modal text editor rather than a Vim clone.

![Ozone editor screenshot](docs/screenshots/ozone-editor.svg)

## Current Shape

Ozone is early, but the GUI path is usable enough for scratch editing and small
files:

- editable text buffers
- file opening from the command line
- dirty buffer tracking in the title and status line
- line numbers
- Catppuccin Mocha style colors
- basic Rust, Markdown, TOML, Oxygen, and plain-text syntax scanning
- undo, redo, save, cursor movement, word movement, and mouse-wheel scrolling

## Build

Ozone expects Aurea next to this repository:

```text
Repos/
  aurea/
  ozone/
```

Build the editor from the Ozone repository root:

```powershell
cargo build --release
```

For faster development checks:

```powershell
cargo check
```

## Run

Open an empty scratch buffer:

```powershell
.\target\release\ozone.exe
```

Open a file:

```powershell
.\target\release\ozone.exe PLAN.md
```

The debug binary also works, but most manual UI testing should use the release
binary so performance and memory behavior are close to the real path.

## How To Use

Ozone is currently always in `EDIT` mode. Click the window if it does not have
focus, then type normally. There is no insert/normal mode switch yet.

The status line shows:

```text
EDIT  file-name*    line:column  UTF-8
```

The `*` means the active buffer has unsaved changes. The window title also shows
the dirty marker.

## Keymaps

| Key | Action |
| --- | --- |
| Text input | Insert text at cursor |
| `Enter` | Insert newline |
| `Tab` | Insert four spaces |
| `Backspace` | Delete character before cursor |
| `Delete` | Delete character after cursor |
| `Left` / `Right` | Move one character |
| `Up` / `Down` | Move one line |
| `Home` / `End` | Move to start/end of line |
| `PageUp` / `PageDown` | Move and scroll one page |
| `Ctrl+Left` / `Ctrl+Right` | Move by word |
| `Ctrl+Home` / `Ctrl+End` | Move to start/end of file |
| `Ctrl+A` | Move to line start |
| `Ctrl+E` | Move to line end |
| `Ctrl+B` | Move left |
| `Ctrl+F` | Move right |
| `Ctrl+P` | Move up |
| `Ctrl+N` | Move down |
| `Ctrl+S` | Save current buffer |
| `Ctrl+Z` | Undo |
| `Ctrl+Y` | Redo |
| Mouse wheel | Scroll |

## Configuration

Ozone loads user configuration from:

- Windows: `%APPDATA%\ozone\config.toml`
- Linux/macOS: `$XDG_CONFIG_HOME/ozone/config.toml`, or `~/.config/ozone/config.toml`

Example:

```toml
[editor]
font = "Consolas"
font_size = 14
line_height = 1.4
tab_width = 4
soft_tabs = true
line_numbers = "absolute"
cursor_style = "bar"
scroll_off = 8
word_wrap = false
trim_trailing_whitespace = true
auto_save = false

[theme]
name = "catppuccin-mocha"
```

Malformed or missing fields fall back to defaults instead of crashing the
editor.

## Workspace Layout

- `src/`: the `ozone` executable entry point
- `ozone-buffer/`: text storage, positions, edits, undo/redo, dirty state, and file persistence
- `ozone-editor/`: workspace state, views, commands, and key-facing editor behavior
- `ozone-gui/`: Aurea-based drawing and input routing
- `ozone-syntax/`: lightweight fallback syntax scanning
- `ozone-config/`: hand-parsed TOML configuration
- `oxygen/`: the older Oxygen language work kept in-tree for now

## Notes

This is not meant to depend on a pile of heavy crates. The config parser uses
`toml`, but Ozone avoids Serde-derived domain models and keeps editor behavior
explicit while the architecture settles.
