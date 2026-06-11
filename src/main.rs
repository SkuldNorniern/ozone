#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use ozone_config::Config;
use ozone_editor::Workspace;
use ozone_gui::OzoneGui;
use std::path::PathBuf;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();

    // `--reset-config` regenerates the user's config.toml + section files
    // (keymap.toml/autocmd.toml/filetype.toml/lsp.toml) from the shipped
    // defaults, even if they already exist. This is the escape hatch for a
    // config written before the keymap.toml split: such a config has no
    // `[keymap]` at all, so every key (including Ctrl/Meta chords) is unbound.
    if let Some(pos) = args.iter().position(|a| a == "--reset-config") {
        args.remove(pos);
        match Config::reset_user_config() {
            Ok(path) => eprintln!("ozone: reset config to defaults at {}", path.display()),
            Err(e) => eprintln!("ozone: failed to reset config: {e}"),
        }
    }

    // Load user config (~/.config/ozone/config.toml or %APPDATA%\ozone\config.toml),
    // falling back to defaults when absent or malformed.
    let (config, config_warning) = Config::load_user_with_warning();
    if let Some(warning) = config_warning {
        eprintln!("ozone: {warning}");
    }
    #[cfg(debug_assertions)]
    {
        let source = Config::resolved_config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "built-in defaults".to_string());
        eprintln!(
            "ozone: config={} font=\"{}\" size={} mouse={}",
            source, config.editor.font, config.editor.font_size, config.ui.mouse
        );
    }

    let mut workspace = Workspace::new();

    // Open a file, or switch to a directory, if one was passed on the
    // command line. A directory argument (e.g. `ozone .`) becomes the
    // workspace root: shell-running autocommands (`!cmd`) and the file
    // picker resolve relative to it via `std::env::current_dir()`.
    if let Some(path_str) = args.first() {
        let path = PathBuf::from(path_str);
        if path.is_dir() {
            if let Err(e) = std::env::set_current_dir(&path) {
                eprintln!("ozone: cannot open directory: {e}");
            }
        } else if let Err(e) = workspace.open_file(path) {
            eprintln!("ozone: cannot open file: {e}");
        }
    }

    let gui = OzoneGui::with_config(workspace, config);

    if let Err(e) = gui.run() {
        eprintln!("ozone: fatal: {e}");
        std::process::exit(1);
    }
}
