mod config;
mod csv_io;
mod datasets;
pub mod db;
pub mod format;
mod import;
pub mod inference;
mod metrics;
mod runs;
pub mod seed;
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
        csv_io::preview_csv,
        datasets::list_datasets,
        import::import_csv,
        metrics::list_metrics,
        runs::start_run,
    ])
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
#[expect(
    clippy::expect_used,
    reason = "startup failure is unrecoverable; panicking is the canonical Tauri pattern"
)]
pub fn run() {
    let specta = specta_builder();

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init());

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
            use tauri::Manager as _;
            specta.mount_events(app);
            // Open DuckDB + apply migrations before the first command can fire.
            // Failing here is unrecoverable (no app without storage), so we
            // surface the error and let Tauri short-circuit setup.
            let db = db::AppDb::open().map_err(|e| format!("open database: {e}"))?;
            app.manage(db);
            // Load all three digit-level models at startup. Slow (~5-15 s on
            // CPU for ~1.5 GB total) but unavoidable for the spike: the
            // synchronous `start_run` IPC expects the registry to be ready.
            let registry =
                inference::load_all_models().map_err(|e| format!("load inference models: {e}"))?;
            app.manage(registry);
            #[cfg(target_os = "macos")]
            {
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
    use specta_typescript::{BigIntExportBehavior, Typescript};

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
                //
                // `bigint(Number)`: Tauri's IPC layer hands i64/u64 to JS via
                // serde_json, which encodes them as JSON numbers. Row counts and
                // surrogate ids won't approach 2^53 in this app's lifetime, so the
                // simpler `number` mapping is preferable to BigInt or string. If we
                // ever introduce a true >2^53 field, switch that specific column to
                // u128 / string and revisit.
                Typescript::default()
                    .header("// @ts-nocheck\n")
                    .bigint(BigIntExportBehavior::Number),
                "../src/bindings.ts",
            )
            .map_err(|e| e.to_string())
    }
}
