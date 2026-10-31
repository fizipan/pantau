//! Menu bar item: a text title, no image. Left click toggles the panel; right click opens the menu.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::AppHandle;

use crate::panel;

const TRAY_ID: &str = "main";

pub fn setup(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let menu = Menu::with_items(
        app,
        &[
            &MenuItem::with_id(app, "settings", "Pengaturan…", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "quit", "Keluar Pantau", true, None::<&str>)?,
        ],
    )?;
    TrayIconBuilder::with_id(TRAY_ID)
        .title("CPU --  RAM --")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "settings" => panel::open_settings(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            let app = tray.app_handle();
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                panel::toggle(app);
            }
        })
        .build(app)?;
    Ok(())
}

pub fn set_title(app: &AppHandle, title: &str) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_title(Some(title));
    }
}

/// Where the menu bar item is right now. `None` when macOS has no window for it.
pub fn rect(app: &AppHandle) -> Option<tauri::Rect> {
    app.tray_by_id(TRAY_ID)?.rect().ok().flatten()
}
