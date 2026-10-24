use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager,
};

// bring the main window back from the tray: show it in the taskbar again,
// unminimize it, and give it focus.
fn restore(app: &AppHandle) {
    if let Some(window) = app.get_window("main") {
        let _ = window.set_skip_taskbar(false);
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

// the tray icon is always present. the window starts in the taskbar and only
// moves to the tray when it is minimized from the bar.
pub fn init(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show Sharkord", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    let icon = app.default_window_icon().cloned().unwrap_or_else(|| {
        Image::from_bytes(include_bytes!("../icons/icon.png")).expect("icon.png loads")
    });

    TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("Sharkord")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => restore(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                restore(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

// minimize the window and hide it from the taskbar so it lives in the tray.
#[tauri::command]
pub fn minimize_to_tray(app: AppHandle) {
    if let Some(window) = app.get_window("main") {
        let _ = window.set_skip_taskbar(true);
        let _ = window.minimize();
    }
}
