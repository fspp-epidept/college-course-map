mod config;
mod courses;
mod csv_io;
mod datasets;
pub mod db;
mod export;
pub mod format;
mod import;
pub mod inference;
mod logging;
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
            logging::open_logs_dir,
            metrics::list_metrics,
            models::cancel_download,
            models::download_models,
            models::load_models,
            models::models_status,
            models::reload_models,
            runtime::download_runtime,
            runtime::relaunch_app,
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
    logging::install_panic_hook();
    let specta = specta_builder();

    // The log plugin goes first so every later step — DB open, runtime
    // pack, model autoload — lands in the file (EPI-109).
    let builder = tauri::Builder::default()
        .plugin(logging::plugin())
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
            // A WAL set aside at open (EPI-105) is reported through the
            // runtime notices below — the one startup-conditions surface the
            // Settings UI already renders.
            let db_notice = db.recovery_notice().map(str::to_owned);
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
                    log::info!("startup: swept {swept} orphaned running run(s) to interrupted");
                }
                let catalog = manifest::resolve_model_rows(&conn, manifest::load()?)?;
                // Fold the startup writes (migrations, sweep, manifest rows)
                // into the main file now (EPI-105): a crash later in this
                // session then orphans only what was written after this
                // point, and the next open has that much less WAL to replay.
                if let Err(e) = conn.execute_batch("CHECKPOINT") {
                    log::warn!("startup: checkpoint skipped: {e}");
                }
                catalog
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
                let mut state = runtime::startup(&settings, &resource_dir)?;
                log::info!(
                    "startup: ONNX Runtime {} loaded from pack '{}'",
                    state.ort_version,
                    state.pack_id
                );
                state.notices.extend(db_notice);
                app.manage(state);
            }
            // Models load lazily (EPI-3/EPI-56): the store starts empty and a
            // background thread fills it when the manifest files are already
            // on disk (always, for airgap; post-download for connected).
            // Commands that need models error cleanly until then.
            app.manage(inference::ModelStore::default());
            // Download in-flight guard + progress snapshots (EPI-74) — managed
            // before autoload so models_status can always resolve it.
            app.manage(models::DownloadState::default());
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
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // Tauri leaves `run` via `process::exit`, so managed state is
            // never dropped and DuckDB never gets its close-time checkpoint.
            // Do it explicitly (EPI-105): a clean exit leaves no WAL for the
            // next launch to replay. Best effort — an in-flight run's flush
            // holds the writer briefly; a checkpoint refused here just leaves
            // the WAL for the next open, as before.
            if let tauri::RunEvent::Exit = event {
                use tauri::Manager as _;
                if let Some(db) = app.try_state::<db::AppDb>()
                    && let Err(e) = db.checkpoint()
                {
                    log::warn!("exit: checkpoint skipped: {e}");
                }
            }
        });
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
