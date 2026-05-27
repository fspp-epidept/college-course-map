pub mod format;
// Native menu is macOS-only; Windows/Linux use custom in-WebView chrome (decision #102).
#[cfg(target_os = "macos")]
mod menu;

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {name}! You've been greeted from Rust!")
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
#[expect(
    clippy::expect_used,
    reason = "startup failure is unrecoverable; panicking is the canonical Tauri pattern"
)]
pub fn run() {
    let builder = tauri::Builder::default().plugin(tauri_plugin_opener::init());

    // macOS keeps native chrome: the base window config is frameless (for the custom
    // Windows/Linux titlebar), so re-enable decorations at startup and attach the
    // native global menu. See decision #102.
    #[cfg(target_os = "macos")]
    let builder = builder
        .menu(menu::build)
        .on_menu_event(|app, event| menu::handle_event(app, &event))
        .setup(|app| {
            use tauri::Manager as _;
            if let Some(window) = app.get_webview_window("main") {
                window.set_decorations(true)?;
            }
            Ok(())
        });

    builder
        .invoke_handler(tauri::generate_handler![greet])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
