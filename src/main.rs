#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use ozone_config::Config;
use ozone_editor::Workspace;
use ozone_gui::OzoneGui;
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().collect();

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

    // Open a file if one was passed on the command line
    if let Some(path_str) = args.get(1) {
        let path = PathBuf::from(path_str);
        if let Err(e) = workspace.open_file(path) {
            eprintln!("ozone: cannot open file: {e}");
        }
    }

    let gui = OzoneGui::with_config(workspace, config);

    if let Err(e) = gui.run() {
        eprintln!("ozone: fatal: {e}");
        std::process::exit(1);
    }
}
