//! Where the app keeps its files: one `college-course-map` product dir per
//! kind, resolved through `dirs` rather than Tauri's identifier-based dirs
//! (decision 2026-05-26). The kinds split so a Windows roaming profile only
//! syncs what should follow the user:
//!
//! - [`config_dir`]: settings and themes. Small; roams (`%APPDATA%`).
//! - [`data_dir`]: database, models, runtime packs, logs. Large and
//!   machine-specific (`%LOCALAPPDATA%`).
//! - [`coreml_cache_dir`]: compiled `CoreML` models. Regenerable; safe to
//!   delete at any time.
//!
//! On macOS and Linux the local and roaming data dirs are the same, so only
//! Windows sees the config/data split. Callers join their own file or subdir
//! names onto the config and data roots. There is deliberately no cache
//! *root*: on Windows `dirs::cache_dir()` is `%LOCALAPPDATA%`, so the cache
//! product dir is the data dir, and deleting it would take the database and
//! models with it. Each cache gets its own leaf function instead.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

const PRODUCT_DIR: &str = "college-course-map";

/// What `config.rs` keeps in the config dir. Before the split, Windows kept
/// data beside these in Roaming; everything else there is data.
const CONFIG_ENTRIES: [&str; 2] = ["settings.json", "themes"];

/// Old home of the `CoreML` compile cache, inside the data dir.
const LEGACY_CACHE_SUBDIR: &str = "cache";

/// `<config>/college-course-map`.
pub(crate) fn config_dir() -> Result<PathBuf, String> {
    product_dir(dirs::config_dir(), "config")
}

/// `<local data>/college-course-map`.
pub(crate) fn data_dir() -> Result<PathBuf, String> {
    product_dir(dirs::data_local_dir(), "data")
}

/// `<cache>/college-course-map/coreml` — compiled `CoreML` models. Derived
/// state: safe to delete at any time.
pub(crate) fn coreml_cache_dir() -> Result<PathBuf, String> {
    Ok(product_dir(dirs::cache_dir(), "cache")?.join("coreml"))
}

/// `<roaming data>/college-course-map` — where 0.5.x and earlier kept data.
/// The same dir as [`data_dir`] on macOS and Linux; on Windows it is also
/// the config dir, so anything cleaning it must keep `settings.json` and
/// `themes/`.
pub(crate) fn legacy_data_dir() -> Result<PathBuf, String> {
    product_dir(dirs::data_dir(), "roaming data")
}

fn product_dir(base: Option<PathBuf>, kind: &str) -> Result<PathBuf, String> {
    base.map(|dir| dir.join(PRODUCT_DIR))
        .ok_or_else(|| format!("no platform {kind} directory available"))
}

/// One-time move of data that 0.5.x and earlier kept in the roaming data dir
/// (`%APPDATA%` on Windows) to [`data_dir`], and removal of the old `CoreML`
/// cache. Must run before anything opens a data path — including the log
/// plugin, which creates `logs/` — so it returns its outcomes (`Ok` = info,
/// `Err` = warning) for the caller to log once the logger is up. Never
/// fails startup: whatever can't be moved stays where it was, is reported,
/// and is retried on the next launch.
///
/// Running before the Tauri builder also means a single-instance plugin,
/// which initializes inside the builder, can't serialize two launches
/// through here; an instance lock has to be taken before this call.
pub(crate) fn migrate_legacy_data() -> Vec<Result<String, String>> {
    let (Ok(legacy), Ok(data)) = (legacy_data_dir(), data_dir()) else {
        return Vec::new();
    };
    migrate(&legacy, &data)
}

fn migrate(legacy: &Path, data: &Path) -> Vec<Result<String, String>> {
    let mut report = Vec::new();
    let old_cache = legacy.join(LEGACY_CACHE_SUBDIR);
    if old_cache.exists() {
        report.push(match fs::remove_dir_all(&old_cache) {
            Ok(()) => Ok(format!("removed old cache {}", old_cache.display())),
            Err(e) => Err(format!(
                "old cache {} not removed: {e}",
                old_cache.display()
            )),
        });
    }
    if legacy == data {
        return report;
    }
    // A fresh install has no legacy dir; nothing to report.
    let Ok(entries) = fs::read_dir(legacy) else {
        return report;
    };
    let names: Vec<_> = entries
        .flatten()
        .map(|entry| entry.file_name())
        .filter(|name| !CONFIG_ENTRIES.iter().any(|c| name == *c))
        .collect();
    // Entries whose target already exists stay put, and so do their
    // companions (`app.duckdb` holds back `app.duckdb.wal`), so a WAL never
    // lands beside a database it doesn't belong to. Decided before anything
    // moves, so readdir order doesn't matter.
    let blocked: Vec<String> = names
        .iter()
        .filter(|name| data.join(name).exists())
        .map(|name| format!("{}.", name.to_string_lossy()))
        .collect();
    for name in names {
        let from = legacy.join(&name);
        let to = data.join(&name);
        let held = blocked
            .iter()
            .any(|prefix| name.to_string_lossy().starts_with(prefix.as_str()));
        report.push(if to.exists() || held {
            Err(format!(
                "{} left in place: {} or the file it belongs to already exists",
                from.display(),
                to.display()
            ))
        } else {
            move_entry(&from, &to)
                .map(|how| format!("{how} {} to {}", from.display(), to.display()))
                .map_err(|e| format!("{} not moved: {e}", from.display()))
        });
    }
    report
}

/// Rename `from` to `to`, or copy then delete when they sit on different
/// volumes (a redirected Roaming folder). Any other rename failure — a file
/// still held open, a permission error — is returned as is: copying a
/// locked database could produce a torn copy that then blocks every retry.
/// The copy lands under a staging name first, so an interrupted copy is
/// never mistaken for a finished one and the next launch retries it.
fn move_entry(from: &Path, to: &Path) -> Result<&'static str, String> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    match fs::rename(from, to) {
        Ok(()) => return Ok("moved"),
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {}
        Err(e) => return Err(e.to_string()),
    }
    let mut staging = to.as_os_str().to_owned();
    staging.push(".migrating");
    let staging = PathBuf::from(staging);
    if staging.exists() {
        remove(&staging).map_err(|e| format!("clear stale {}: {e}", staging.display()))?;
    }
    if let Err(e) = copy(from, &staging) {
        let _ = remove(&staging);
        return Err(format!("copy failed: {e}"));
    }
    fs::rename(&staging, to).map_err(|e| format!("rename {}: {e}", staging.display()))?;
    remove(from).map_err(|e| format!("copied, but the old copy was not removed: {e}"))?;
    Ok("copied")
}

fn copy(from: &Path, to: &Path) -> io::Result<()> {
    if from.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(from, to).map(|_| ())
    }
}

fn remove(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use super::{copy, migrate, move_entry};

    fn scratch(name: &str) -> Result<PathBuf, String> {
        let root = std::env::temp_dir().join(format!("ccm-paths-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        Ok(root)
    }

    fn write(path: &PathBuf, body: &str) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(path, body).map_err(|e| e.to_string())
    }

    /// The Windows case: config and data share the legacy dir. Data moves,
    /// settings and themes stay, an existing target is never overwritten,
    /// and the old cache is dropped rather than moved.
    #[test]
    fn moves_data_and_leaves_config() -> Result<(), String> {
        let root = scratch("split")?;
        let legacy = root.join("roaming");
        let data = root.join("local");
        write(&legacy.join("settings.json"), "{}")?;
        write(&legacy.join("themes/mine.json"), "{}")?;
        write(&legacy.join("app.duckdb"), "db")?;
        write(&legacy.join("models/two/model.onnx"), "onnx")?;
        write(&legacy.join("cache/coreml/blob"), "compiled")?;
        write(&legacy.join("logs/app.log"), "old")?;
        write(&data.join("logs/app.log"), "new")?;

        let report = migrate(&legacy, &data);

        assert!(legacy.join("settings.json").exists());
        assert!(legacy.join("themes/mine.json").exists());
        assert!(!data.join("settings.json").exists());
        assert!(!legacy.join("app.duckdb").exists());
        assert_eq!(
            fs::read_to_string(data.join("app.duckdb")).map_err(|e| e.to_string())?,
            "db"
        );
        assert!(data.join("models/two/model.onnx").exists());
        assert!(!legacy.join("cache").exists());
        assert!(!data.join("cache").exists());
        assert_eq!(
            fs::read_to_string(data.join("logs/app.log")).map_err(|e| e.to_string())?,
            "new"
        );
        assert!(legacy.join("logs/app.log").exists());
        assert_eq!(
            report.iter().filter(|r| r.is_err()).count(),
            1,
            "{report:?}"
        );

        // Second launch: nothing left to move but the conflicting logs dir.
        let again = migrate(&legacy, &data);
        assert_eq!(again.len(), 1, "{again:?}");
        let _ = fs::remove_dir_all(&root);
        Ok(())
    }

    /// macOS/Linux: same dir, so only the old cache goes.
    #[test]
    fn same_dir_only_drops_cache() -> Result<(), String> {
        let root = scratch("same")?;
        write(&root.join("app.duckdb"), "db")?;
        write(&root.join("cache/coreml/blob"), "compiled")?;

        let report = migrate(&root, &root);

        assert!(root.join("app.duckdb").exists());
        assert!(!root.join("cache").exists());
        assert_eq!(report.len(), 1, "{report:?}");
        let _ = fs::remove_dir_all(&root);
        Ok(())
    }

    /// No legacy dir at all (fresh install): silent no-op.
    #[test]
    fn fresh_install_reports_nothing() -> Result<(), String> {
        let root = scratch("fresh")?;
        assert!(migrate(&root.join("missing"), &root.join("local")).is_empty());
        let _ = fs::remove_dir_all(&root);
        Ok(())
    }

    /// The cross-volume fallback copies a tree intact.
    #[test]
    fn copy_preserves_tree() -> Result<(), String> {
        let root = scratch("copy")?;
        write(&root.join("src/a/b.bin"), "b")?;
        write(&root.join("src/c.txt"), "c")?;
        copy(&root.join("src"), &root.join("dst")).map_err(|e| e.to_string())?;
        assert_eq!(
            fs::read_to_string(root.join("dst/a/b.bin")).map_err(|e| e.to_string())?,
            "b"
        );
        assert!(root.join("dst/c.txt").exists());
        let _ = fs::remove_dir_all(&root);
        Ok(())
    }

    /// A database that can't move holds back its WAL, so the WAL never
    /// lands beside a different database; unrelated entries still move.
    #[test]
    fn blocked_entry_holds_back_companions() -> Result<(), String> {
        let root = scratch("companions")?;
        let legacy = root.join("roaming");
        let data = root.join("local");
        write(&legacy.join("app.duckdb"), "old db")?;
        write(&legacy.join("app.duckdb.wal"), "old wal")?;
        write(&legacy.join("models/two/model.onnx"), "onnx")?;
        write(&data.join("app.duckdb"), "new db")?;

        let report = migrate(&legacy, &data);

        assert!(legacy.join("app.duckdb.wal").exists());
        assert!(!data.join("app.duckdb.wal").exists());
        assert!(data.join("models/two/model.onnx").exists());
        assert_eq!(
            report.iter().filter(|r| r.is_err()).count(),
            2,
            "{report:?}"
        );
        let _ = fs::remove_dir_all(&root);
        Ok(())
    }

    /// A rename failure that isn't cross-volume is reported, not copied.
    #[test]
    fn non_cross_device_rename_failure_does_not_copy() -> Result<(), String> {
        let root = scratch("nocopy")?;
        let to = root.join("local/app.duckdb");
        assert!(move_entry(&root.join("roaming/app.duckdb"), &to).is_err());
        assert!(!to.exists());
        assert!(!root.join("local/app.duckdb.migrating").exists());
        let _ = fs::remove_dir_all(&root);
        Ok(())
    }
}
