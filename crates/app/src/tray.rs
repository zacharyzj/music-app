use super::*;
use slint::winit_030::WinitWindowAccessor;

fn create(ui: &AppWindow) -> Result<PlayerTray> {
    let tray = PlayerTray::new()?;
    let window = ui.as_weak();
    tray.on_restore_window(move || {
        if let Some(ui) = window.upgrade()
            && ui.show().is_ok()
        {
            ui.window().set_minimized(false);
            ui.window()
                .with_winit_window(|native| native.focus_window());
        }
    });
    tray.on_quit_player(|| {
        let _ = slint::quit_event_loop();
    });
    tray.show()?;
    Ok(tray)
}

pub(super) fn install(ui: &AppWindow, db: &Library, root: PathBuf) {
    let enabled = db.get("close-to-tray") == true;
    let tray = RefCell::new(if enabled { create(ui).ok() } else { None });
    ui.set_supports_tray(true);
    ui.set_close_to_tray(tray.borrow().is_some());
    let window = ui.as_weak();
    ui.on_select_close_behavior(move |enabled| {
        let Some(ui) = window.upgrade() else { return };
        let apply = || -> Result<()> {
            let new_tray = if enabled { Some(create(&ui)?) } else { None };
            Library::open(&root)?.set("close-to-tray", json!(enabled))?;
            tray.replace(new_tray);
            ui.set_close_to_tray(enabled);
            Ok(())
        };
        if let Err(e) = apply() {
            ui.invoke_notify_row(format!("无法更改关闭窗口设置 · {e}").into());
        }
    });
    // Slint hides a window on close. A visible SystemTrayIcon keeps the event loop alive
    // until its Quit menu item is used. Without the icon, the last window closes the app.
}

#[cfg(all(test, feature = "ui-smoke"))]
mod tests {
    use super::*;

    #[test]
    fn close_behavior_is_applied_and_persisted_from_the_settings_control() -> Result<()> {
        let _clock = crate::test_ui::test_clock()?;
        let dir = tempfile::tempdir()?;
        let db = Library::open(dir.path())?;
        let window = AppWindow::new()?;
        install(&window, &db, dir.path().to_owned());
        assert!(window.get_supports_tray() && !window.get_close_to_tray());
        window.invoke_select_close_behavior(true);
        assert!(window.get_close_to_tray() && db.get("close-to-tray") == true);
        let reopened = AppWindow::new()?;
        install(&reopened, &db, dir.path().to_owned());
        assert!(reopened.get_close_to_tray());
        reopened.invoke_select_close_behavior(false);
        assert!(!reopened.get_close_to_tray() && db.get("close-to-tray") == false);
        Ok(())
    }
}
