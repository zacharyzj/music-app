#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() -> anyhow::Result<()> {
    music_app_ui::run()
}
