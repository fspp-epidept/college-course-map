mod config;
mod courses;
mod csv_io;
mod datasets;
pub mod db;
mod export;
pub mod format;
mod import;
pub mod inference;
pub mod manifest;
mod metrics;
mod models;
// Public for the resume verification harness (examples/check_resume.rs,
// EPI-39), which drives the real RunPipeline against a scratch database.
pub mod runs;
// Public for the dev pack fetcher (examples/runtime_install.rs, EPI-73).
pub mod runtime;
pub mod seed;
// Native menu is macOS-only; Windows/Linux use custom in-WebView chrome (decision #102).
#[cfg(target_os = "macos")]
mod menu;

use tauri_specta::{Builder, collect_commands, collect_events};

/// Collect the IPC command surface into a tauri-specta builder. Single source of
/// truth for both the runtime `invoke_handler` and the generated `src/bindings.ts`
/// (see #58); the `export_bindings` test renders the bindings from this same builder.
fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            config::list_themes,
            config::read_theme,
            config::read_settings,
            config::write_settings,
            courses::get_classification_coverage,
            courses::list_courses_with_results,
            courses::model_id_for_digit_level,
            csv_io::preview_csv,
            datasets::list_datasets,
            export::export_results,
            import::import_csv,
            metrics::list_metrics,
            models::download_models,
            models::load_models,
            models::models_status,
            models::reload_models,
            runtime::download_runtime,
            runtime::runtime_status,
            runs::get_latest_run,
            runs::get_run,
            runs::list_runs,
            runs::pause_run,
            runs::resume_run,
            runs::start_run,
        ])
        .events(collect_events![
            models::ModelDownloadProgress,
            models::ModelsStateChanged,
            runtime::RuntimeDownloadProgress,
            runtime::RuntimeStateChanged,
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
            // Resolve the embedded model manifest against the models table so
            // every digit-level → model-id lookup goes through pinned rows
            // (stale rows from earlier families stay put for their cached
            // results but are never selected).
            let catalog = {
                let conn = db.rw().map_err(|e| format!("manifest rows: {e}"))?;
                // Crash recovery (EPI-38): a `running` row in a fresh process
                // is an orphan from a previous one — flip it to `interrupted`
                // (resumable) before any command can observe it.
                let swept = runs::sweep_orphaned_runs(&conn)?;
                if swept > 0 {
                    eprintln!("startup: swept {swept} orphaned running run(s) to interrupted");
                }
                manifest::resolve_model_rows(&conn, manifest::load()?)?
            };
            app.manage(db);
            app.manage(catalog);
            // Load ONNX Runtime (EPI-73): with `load-dynamic` nothing is
            // linked, so the dylib must be loaded before any session exists.
            // Pack choice follows the settings EP priority (GPU pack when
            // installed, bundled CPU pack otherwise) and is fixed for the
            // process lifetime — switching packs requires a relaunch.
            {
                let settings =
                    config::read_settings().map_err(|e| format!("read settings: {e}"))?;
                let resource_dir = app
                    .path()
                    .resource_dir()
                    .map_err(|e| format!("resolve bundle resource dir: {e}"))?;
                let state = runtime::startup(&settings, &resource_dir)?;
                eprintln!(
                    "startup: ONNX Runtime {} loaded from pack '{}'",
                    state.ort_version, state.pack_id
                );
                app.manage(state);
            }
            // Models load lazily (EPI-3/EPI-56): the store starts empty and a
            // background thread fills it when the manifest files are already
            // on disk (always, for airgap; post-download for connected).
            // Commands that need models error cleanly until then.
            app.manage(inference::ModelStore::default());
            models::autoload_if_present(app.handle());
            // Tracks per-run cancellation flags so `pause_run` can signal an
            // in-flight worker (EPI-37).
            app.manage(runs::RunRegistry::default());
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
