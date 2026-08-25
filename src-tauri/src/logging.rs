//! Diagnostic log file (EPI-109). Every 0.4.0 field bug needed testers to
//! run `ProcDump` or redirect stderr to recover a panic message, because the
//! release exe has no console and the app's breadcrumbs were `eprintln!`.
//! The `log` facade now writes to `<data>/college-course-map/logs/app.log`
//! (product-dir convention, not Tauri's identifier-based log dir) and to
//! stdout for dev runs.
//!
//! Content policy — diagnostics only, so a log can be attached to a bug
//! report without review: startup steps, pack/EP resolution, model load
//! outcomes, run and import lifecycle keyed by id and counts, errors, and
//! panics. Never course text or any CSV cell, model outputs, or per-row /
//! per-batch lines — a 2M-row run must not grow the file. ONNX Runtime's own
//! messages arrive through `tracing` (the `tracing/log` bridge) at warn and
//! above, so EP registration and session failures are captured verbatim.

use std::path::PathBuf;

use log::LevelFilter;
use tauri_plugin_log::{RotationStrategy, Target, TargetKind, TimezoneStrategy};

const PRODUCT_DIR: &str = "college-course-map";
const LOGS_SUBDIR: &str = "logs";
const LOG_NAME: &str = "app";

/// Rotate when the file exceeds this; `KeepOne` deletes the previous file,
/// so disk use is bounded at about twice this figure.
const MAX_FILE_SIZE: u128 = 2_000_000;

/// `<data>/college-course-map/logs`.
pub(crate) fn logs_dir() -> Result<PathBuf, String> {
    dirs::data_dir()
        .map(|dir| dir.join(PRODUCT_DIR).join(LOGS_SUBDIR))
        .ok_or_else(|| "no platform data directory available".to_owned())
}

/// The log plugin, registered first so every later startup step is
/// captured. A missing data directory only costs the file target — stdout
/// still works — and is reported the one way that can't depend on the
/// logger.
pub(crate) fn plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    let mut builder = tauri_plugin_log::Builder::new()
        .clear_targets()
        .target(Target::new(TargetKind::Stdout))
        .level(LevelFilter::Info)
        .level_for("ort", LevelFilter::Warn)
        .max_file_size(MAX_FILE_SIZE)
        .rotation_strategy(RotationStrategy::KeepOne)
        .timezone_strategy(TimezoneStrategy::UseLocal);
    match logs_dir() {
        Ok(path) => {
            builder = builder.target(Target::new(TargetKind::Folder {
                path,
                file_name: Some(LOG_NAME.to_owned()),
            }));
        }
        Err(e) => eprintln!("logging: no log file: {e}"),
    }
    builder.build()
}

/// Record panics before the default hook prints them — Tauri's top-level
/// `expect` on a setup failure (the EPI-105 shape) is a panic, and a
/// window-flash crash must still leave a line in the file.
pub(crate) fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("panic: {info}");
        default_hook(info);
    }));
}

/// Open the logs folder in the platform file manager (Settings → About).
/// Rust-side opener call: no capability widening for the `WebView`.
#[tauri::command]
#[specta::specta]
pub(crate) fn open_logs_dir() -> Result<(), String> {
    let dir = logs_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    tauri_plugin_opener::open_path(&dir, None::<&str>).map_err(|e| e.to_string())
}
