#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use ozone_config::Config;
use ozone_editor::Workspace;
use ozone_gui::OzoneGui;
use std::path::PathBuf;

/// Parse a boolean-ish CLI/env value: `1/true/on/yes` → true, `0/false/off/no`
/// → false, anything else → `None` (ignored, so a typo doesn't flip the toggle).
fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Some(true),
        "0" | "false" | "off" | "no" => Some(false),
        _ => None,
    }
}

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
    let (mut config, config_warning) = Config::load_user_with_warning();

    // Hardware-acceleration override, precedence: CLI flag > OZONE_GPU env >
    // config `[ui] hardware_acceleration`. GPU still only engages in a build with
    // the `zengpu` feature; otherwise the GUI falls back to CPU and says so.
    if let Some(v) = std::env::var("OZONE_GPU").ok().and_then(|v| parse_bool(&v)) {
        config.ui.hardware_acceleration = v;
    }
    if let Some(pos) = args.iter().position(|a| a == "--gpu") {
        args.remove(pos);
        config.ui.hardware_acceleration = true;
    }
    if let Some(pos) = args.iter().position(|a| a == "--no-gpu") {
        args.remove(pos);
        config.ui.hardware_acceleration = false;
    }
    let mut startup_warnings: Vec<String> = Vec::new();
    if let Some(warning) = config_warning {
        // Keep the stderr line for console launches, but also carry it into the
        // GUI: a windowed release build (`windows_subsystem = "windows"`) has no
        // console, so stderr alone would silently swallow config problems.
        eprintln!("ozone: {warning}");
        startup_warnings.push(format!("Config: {warning}"));
    }
    #[cfg(debug_assertions)]
    {
        let source = Config::resolved_config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "built-in defaults".to_string());
        eprintln!(
            "ozone: config={} font=\"{}\" size={} mouse={} gpu={}",
            source,
            config.editor.font,
            config.editor.font_size,
            config.ui.mouse,
            config.ui.hardware_acceleration
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

    let gui = OzoneGui::with_config(workspace, config).with_startup_warnings(startup_warnings);

    if let Err(e) = gui.run() {
        eprintln!("ozone: fatal: {e}");
        std::process::exit(1);
    }
}
