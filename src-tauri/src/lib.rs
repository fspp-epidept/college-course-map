// Public for `boot::Progress`, which `AppDb::open_at` takes and the examples
// pass as `Progress::none()`.
mod activity;
pub mod boot;
// Public for the resume verification harness (examples/check_resume.rs),
// which drives the real ClassifyPipeline against a scratch database.
pub mod classify;
mod config;
mod courses;
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
mod paths;
mod preflight;
mod profile;
mod reset;
// Public for the dev pack fetcher (examples/runtime_install.rs, EPI-73).
pub mod runtime;
pub mod seed;
#[cfg(unix)]
mod signals;
mod storage;
mod taxonomy;
// The native menu is macOS-only (Windows/Linux use custom in-WebView chrome,
// decision #102); the module itself builds everywhere for its typed event.
mod menu;

use tauri_specta::{Builder, collect_commands, collect_events};

/// Collect the IPC command surface into a tauri-specta builder. Single source of
/// truth for both the runtime `invoke_handler` and the generated `src/bindings.ts`
/// (see #58); the `export_bindings` test renders the bindings from this same builder.
fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            boot::boot_state,
            classify::classify_dataset,
            classify::stop_classification,
            config::list_themes,
            config::read_theme,
            config::read_settings,
            config::write_settings,
            courses::get_classification_coverage,
            courses::list_courses_with_results,
            courses::model_id_for_digit_level,
            datasets::delete_dataset,
            datasets::get_input_profile,
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
            preflight::inspect_csv,
            preflight::validate_import,
            reset::request_reset,
            runtime::download_runtime,
            runtime::relaunch_app,
            runtime::remove_runtime,
            runtime::runtime_status,
            storage::compact_database,
            storage::open_data_dir,
            storage::storage_clear,
            storage::storage_prune,
            storage::storage_status,
            taxonomy::list_ccm_taxonomy,
            taxonomy::open_ccm_reference,
        ])
        .events(collect_events![
            boot::BootStateChanged,
            menu::MenuActionTriggered,
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

    // Plugin order is startup order; see `boot.rs`. Single-instance must be
    // registered first (plugin docs): a second launch exits in its setup,
    // before it can touch the DuckDB file lock (#207), and the running
    // instance surfaces its window instead.
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            use tauri::Manager as _;
            log::info!("second launch: focusing the running instance");
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(boot::plugin())
        .plugin(logging::plugin())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init());

    // macOS keeps native chrome and the native global menu. See decision #102.
    #[cfg(target_os = "macos")]
    let builder = builder
        .menu(menu::build)
        .on_menu_event(|app, event| menu::handle_event(app, &event));

    builder
        .invoke_handler(specta.invoke_handler())
        .setup(move |app| {
            specta.mount_events(app);
            boot::start(app.handle())?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                boot::shutdown(app);
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
