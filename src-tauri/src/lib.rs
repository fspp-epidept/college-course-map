pub mod format;
mod config;
// Native menu is macOS-only; Windows/Linux use custom in-WebView chrome (decision #102).
#[cfg(target_os = "macos")]
mod menu;

use tauri_specta::{Builder, collect_commands};

/// Collect the IPC command surface into a tauri-specta builder. Single source of
/// truth for both the runtime `invoke_handler` and the generated `src/bindings.ts`
/// (see #58); the `export_bindings` test renders the bindings from this same builder.
fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new().commands(collect_commands![
        config::list_themes,
        config::read_theme,
        config::read_settings,
        config::write_settings,
    ])
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
#[expect(
    clippy::expect_used,
    reason = "startup failure is unrecoverable; panicking is the canonical Tauri pattern"
)]
pub fn run() {
    let specta = specta_builder();

    let builder = tauri::Builder::default().plugin(tauri_plugin_opener::init());

    // macOS keeps native chrome: the base window config is frameless (for the custom
    // Windows/Linux titlebar), so re-enable decorations at startup and attach the
    // native global menu. See decision #102.
    #[cfg(target_os = "macos")]
    let builder = builder
        .menu(menu::build)
        .on_menu_event(|app, event| menu::handle_event(app, &event));

    builder
        .invoke_handler(specta.invoke_handler())
        .setup(move |app| {
            specta.mount_events(app);
            #[cfg(target_os = "macos")]
            {
                use tauri::Manager as _;
                if let Some(window) = app.get_webview_window("main") {
                    window.set_decorations(true)?;
                }
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use specta_typescript::Typescript;

    /// Render `src/bindings.ts` from the command surface. This is the headless,
    /// CI-friendly generator — run `task gen:bindings` (which runs this test, then
    /// formats the output) after changing any command signature.
    #[test]
    fn export_bindings() -> Result<(), String> {
        super::specta_builder()
            .export(
                // `@ts-nocheck`: the generated file is exempt from the repo's strict
                // `noUnusedLocals`/`any` rules (tauri-specta's runtime helpers trip both).
                // Consumers still get full types from the exported declarations.
                Typescript::default().header("// @ts-nocheck\n"),
                "../src/bindings.ts",
            )
            .map_err(|e| e.to_string())
    }
}
