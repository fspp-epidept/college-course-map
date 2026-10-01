//! Reset app data (#206). The Settings action can't delete the data folders
//! in place: the database, the log file and the loaded ONNX Runtime pack are
//! all open, and Windows refuses to delete open files. So `request_reset`
//! only writes a marker and relaunches; `apply_pending` runs first thing in
//! the next process — before the log plugin, `AppDb::open` and model loading
//! — and clears the product folders, so startup proceeds as a first run.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::thread;
use std::time::Duration;

use crate::paths;

const MARKER: &str = "reset-pending";
const KEEP_SETTINGS: &str = "keep-settings";

/// `restart` spawns the new process before the old one exits, so the old
/// process can still hold the database and log open for a moment. Retry
/// within this budget before giving up.
const ATTEMPTS: u32 = 20;
const RETRY_DELAY: Duration = Duration::from_millis(250);

/// Write the reset marker and relaunch; the next process does the deleting.
#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn request_reset(app: tauri::AppHandle, keep_settings: bool) -> Result<(), String> {
    let root = paths::data_dir()?;
    fs::create_dir_all(&root).map_err(|e| format!("create {}: {e}", root.display()))?;
    let mode = if keep_settings { KEEP_SETTINGS } else { "all" };
    fs::write(root.join(MARKER), mode).map_err(|e| format!("write reset marker: {e}"))?;
    app.restart();
}

/// Clear the product folders if a reset is pending. Runs before the logger
/// exists, so the outcome is returned for `report` to log once it does.
/// `Ok(false)`: no reset was pending.
pub(crate) fn apply_pending() -> Result<bool, String> {
    let data = paths::data_dir()?;
    let marker = data.join(MARKER);
    let mode = match fs::read_to_string(&marker) {
        Ok(mode) => mode,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(format!("read reset marker: {e}")),
    };
    // Consume the marker before deleting anything: a reset that fails part
    // way must not run again on a later launch and take data created since.
    fs::remove_file(&marker).map_err(|e| format!("remove reset marker: {e}"))?;
    let keep: &[&str] = if mode == KEEP_SETTINGS {
        &paths::CONFIG_ENTRIES
    } else {
        &[]
    };
    // Config first, with the settings exclusions: on macOS it shares a folder
    // with data, and on Windows it is the pre-#205 Roaming folder, so
    // clearing it also takes any data not yet migrated out. There is no
    // cache root (see `paths`); the CoreML cache is the one cache leaf.
    let config = paths::config_dir()?;
    clear_with_retry(&config, keep)?;
    for dir in [data, paths::coreml_cache_dir()?] {
        if dir != config {
            clear_with_retry(&dir, &[])?;
        }
    }
    Ok(true)
}

/// Log the startup reset outcome and turn a failure into a runtime notice.
pub(crate) fn report(outcome: Result<bool, String>) -> Option<String> {
    match outcome {
        Ok(true) => {
            log::info!("startup: app data reset");
            None
        }
        Ok(false) => None,
        Err(e) => {
            log::error!("startup: app data reset failed: {e}");
            Some(format!(
                "Resetting app data did not finish: {e}. Some data may remain; \
                 close the app and delete the college-course-map folders by hand."
            ))
        }
    }
}

fn clear_with_retry(root: &Path, keep: &[&str]) -> Result<(), String> {
    let mut attempt = 1;
    loop {
        match clear(root, keep) {
            Ok(()) => return Ok(()),
            Err(e) if attempt >= ATTEMPTS => return Err(format!("{}: {e}", root.display())),
            Err(_) => {
                attempt += 1;
                thread::sleep(RETRY_DELAY);
            }
        }
    }
}

/// Delete every entry of `root` except the names in `keep`. A missing root
/// is already clear.
fn clear(root: &Path, keep: &[&str]) -> std::io::Result<()> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        if keep.iter().any(|k| entry.file_name() == **k) {
            continue;
        }
        let path = entry.path();
        // `file_type` doesn't follow symlinks, so a linked folder is unlinked,
        // never recursed into.
        if entry.file_type()?.is_dir() {
            fs::remove_dir_all(&path)?;
        } else {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::clear;

    #[test]
    fn clear_keeps_only_listed_entries() -> std::io::Result<()> {
        let root = std::env::temp_dir().join(format!("ccm-reset-test-{}", std::process::id()));
        std::fs::create_dir_all(root.join("themes"))?;
        std::fs::create_dir_all(root.join("models").join("2"))?;
        std::fs::write(root.join("settings.json"), "{}")?;
        std::fs::write(root.join("app.duckdb"), "")?;
        clear(&root, &["settings.json", "themes"])?;
        let mut left: Vec<_> = std::fs::read_dir(&root)?
            .map(|e| e.map(|e| e.file_name()))
            .collect::<Result<_, _>>()?;
        left.sort();
        assert_eq!(left, ["settings.json", "themes"]);
        clear(&root, &[])?;
        assert_eq!(std::fs::read_dir(&root)?.count(), 0);
        std::fs::remove_dir(&root)
    }

    #[test]
    fn clear_missing_root_is_ok() -> std::io::Result<()> {
        clear(&std::env::temp_dir().join("ccm-reset-test-absent"), &[])
    }
}
