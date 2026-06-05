use ozone_config::Config;
use ozone_editor::Workspace;
use ozone_gui::OzoneGui;
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Load user config (~/.config/ozone/config.toml or %APPDATA%\ozone\config.toml),
    // falling back to defaults when absent or malformed.
    let config = Config::load_user();

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
