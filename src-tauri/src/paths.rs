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
    fs,
    io::{self, Read as _, Write as _},
    path::{Path, PathBuf},
};

use crate::boot::Progress;

const PRODUCT_DIR: &str = "college-course-map";

/// What `config.rs` keeps in the config dir. Before the split, Windows kept
/// data beside these in Roaming; everything else there is data.
const CONFIG_ENTRIES: [&str; 2] = ["settings.json", "themes"];

/// Old home of the `CoreML` compile cache, inside the data dir. Deleted on
/// every platform: the cache now lives under [`coreml_cache_dir`].
const LEGACY_CACHE_SUBDIR: &str = "cache";

/// Deleted rather than moved, and only on Windows, where they sit in Roaming
/// (elsewhere the legacy dir is the live data dir). Old logs aren't worth
/// moving (decision 2026-10-02); a `session.lock` there is stale, because
/// the live one sits beside the database in [`data_dir`].
const LEGACY_DISCARD: [&str; 2] = ["logs", "session.lock"];

/// Copy buffer, and so the cancel/progress granularity of a large file.
const COPY_CHUNK: usize = 1 << 20;

/// Suffix of an in-progress cross-volume copy, beside its target.
const STAGING_SUFFIX: &str = ".migrating";

/// Suffix of a source whose copy finished, set aside for deletion.
const MIGRATED_SUFFIX: &str = ".migrated";

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
/// (`%APPDATA%` on Windows) to [`data_dir`]. The old `CoreML` cache and, on
/// Windows, the old logs are deleted instead. Must run before the database,
/// models, or runtime packs are opened; it returns its outcomes (`Ok` = info,
/// `Err` = warning) for the caller to log. Never fails startup: whatever
/// can't be moved stays where it was, is reported, and is retried on the
/// next launch.
///
/// A cross-volume copy reports bytes through `progress` and stops between
/// chunks once it is cancelled, discarding its staged copy; the entries not
/// yet moved are retried on the next launch.
pub(crate) fn migrate_legacy_data(progress: &Progress<'_>) -> Vec<Result<String, String>> {
    let (Ok(legacy), Ok(data)) = (legacy_data_dir(), data_dir()) else {
        return Vec::new();
    };
    migrate(&legacy, &data, progress)
}

fn migrate(legacy: &Path, data: &Path, progress: &Progress<'_>) -> Vec<Result<String, String>> {
    let mut report = Vec::new();
    discard(&legacy.join(LEGACY_CACHE_SUBDIR), "old cache", &mut report);
    if legacy == data {
        return report;
    }
    for name in LEGACY_DISCARD {
        discard(&legacy.join(name), "old", &mut report);
    }
    // Leftovers of an interrupted run: a staged copy never renamed into
    // place is incomplete, and a set-aside source is already copied.
    sweep(data, STAGING_SUFFIX, &mut report);
    sweep(legacy, MIGRATED_SUFFIX, &mut report);
    // A fresh install has no legacy dir; nothing to report.
    let Ok(entries) = fs::read_dir(legacy) else {
        return report;
    };
    let names: Vec<_> = entries
        .flatten()
        .map(|entry| entry.file_name())
        .filter(|name| !CONFIG_ENTRIES.iter().any(|c| name == *c))
        .filter(|name| !name.to_string_lossy().ends_with(MIGRATED_SUFFIX))
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
        if progress.cancelled() {
            break;
        }
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
            move_entry(&from, &to, progress)
                .map(|how| format!("{how} {} to {}", from.display(), to.display()))
                .map_err(|e| format!("{} not moved: {e}", from.display()))
        });
    }
    report
}

/// Delete `path` if it exists, reporting the outcome as `what`.
fn discard(path: &Path, what: &str, report: &mut Vec<Result<String, String>>) {
    if path.exists() {
        report.push(match remove(path) {
            Ok(()) => Ok(format!("removed {what} {}", path.display())),
            Err(e) => Err(format!("{what} {} not removed: {e}", path.display())),
        });
    }
}

/// Delete every entry in `dir` whose name ends with `suffix`.
fn sweep(dir: &Path, suffix: &str, report: &mut Vec<Result<String, String>>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().ends_with(suffix) {
            discard(&entry.path(), "leftover", report);
        }
    }
}

/// Rename `from` to `to`, or copy across when they sit on different volumes
/// (a redirected Roaming folder). Any other rename failure — a file still
/// held open, a permission error — is returned as is: copying a locked
/// database could produce a torn copy.
fn move_entry(from: &Path, to: &Path, progress: &Progress<'_>) -> Result<&'static str, String> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    match fs::rename(from, to) {
        Ok(()) => Ok("moved"),
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
            copy_across(from, to, progress).map(|()| "copied")
        }
        Err(e) => Err(e.to_string()),
    }
}

/// The cross-volume move. Every step either finishes or leaves a state the
/// next launch's [`sweep`]s resolve, so a kill or power loss at any point
/// loses nothing:
///
/// 1. Copy to `<to>.migrating`, flushing each file to disk. Killed or
///    cancelled here: the source is intact and the staged copy is removed
///    (now, or by the next launch's sweep).
/// 2. Rename the staged copy to `to`. Atomic on one volume.
/// 3. Rename the source to `<from>.migrated`. Atomic on one volume; only a
///    kill between 2 and 3 leaves both copies, reported as left in place.
/// 4. Delete the set-aside source. Killed here: the remainder is swept.
fn copy_across(from: &Path, to: &Path, progress: &Progress<'_>) -> Result<(), String> {
    let staging = with_suffix(to, STAGING_SUFFIX);
    let mut copied = Copied {
        done: 0,
        total: size(from).map_err(|e| format!("measure: {e}"))?,
        progress,
    };
    if let Err(e) = copy(from, &staging, &mut copied) {
        let _ = remove(&staging);
        return Err(format!("copy failed: {e}"));
    }
    fs::rename(&staging, to).map_err(|e| format!("rename {}: {e}", staging.display()))?;
    let set_aside = with_suffix(from, MIGRATED_SUFFIX);
    fs::rename(from, &set_aside)
        .map_err(|e| format!("copied, but the old copy could not be set aside: {e}"))?;
    remove(&set_aside)
        .map_err(|e| format!("copied, but the old copy was not removed (retried next launch): {e}"))
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Bytes copied so far out of the entry's total, reported as they land.
struct Copied<'p, 'b> {
    done: u64,
    total: u64,
    progress: &'p Progress<'b>,
}

/// Total size of the files under `path`.
fn size(path: &Path) -> io::Result<u64> {
    if path.is_dir() {
        fs::read_dir(path)?.try_fold(0, |sum, entry| Ok(sum + size(&entry?.path())?))
    } else {
        Ok(fs::metadata(path)?.len())
    }
}

/// Recursive copy that flushes each file to disk before returning, so the
/// source is never deleted while its copy is still only in the OS cache.
/// `File::create` opens for writing, which Windows' `FlushFileBuffers`
/// (behind `sync_all`) requires. Checks for cancel before every chunk.
fn copy(from: &Path, to: &Path, copied: &mut Copied<'_, '_>) -> io::Result<()> {
    if from.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy(&entry.path(), &to.join(entry.file_name()), copied)?;
        }
        return Ok(());
    }
    let mut src = fs::File::open(from)?;
    let mut dst = fs::File::create(to)?;
    let mut buf = vec![0; COPY_CHUNK];
    loop {
        if copied.progress.cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let n = src.read(&mut buf)?;
        let Some(chunk) = buf.get(..n).filter(|chunk| !chunk.is_empty()) else {
            break;
        };
        dst.write_all(chunk)?;
        copied.done += n as u64;
        copied.progress.report(copied.done, copied.total);
    }
    dst.sync_all()
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

    use super::{Copied, copy, copy_across, migrate, move_entry};
    use crate::boot::{Boot, Progress};

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
    /// settings and themes stay, and the old cache and logs are dropped
    /// rather than moved (the live logs dir is never touched).
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

        let report = migrate(&legacy, &data, &Progress::none());

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
        assert!(!legacy.join("logs").exists());
        assert!(report.iter().all(Result::is_ok), "{report:?}");

        // Second launch: nothing left to do.
        let again = migrate(&legacy, &data, &Progress::none());
        assert!(again.is_empty(), "{again:?}");
        let _ = fs::remove_dir_all(&root);
        Ok(())
    }

    /// macOS/Linux: same dir, so only the old cache goes; `logs/` there is
    /// the live logs dir and stays.
    #[test]
    fn same_dir_only_drops_cache() -> Result<(), String> {
        let root = scratch("same")?;
        write(&root.join("app.duckdb"), "db")?;
        write(&root.join("logs/app.log"), "live")?;
        write(&root.join("cache/coreml/blob"), "compiled")?;

        let report = migrate(&root, &root, &Progress::none());

        assert!(root.join("app.duckdb").exists());
        assert!(root.join("logs/app.log").exists());
        assert!(!root.join("cache").exists());
        assert_eq!(report.len(), 1, "{report:?}");
        let _ = fs::remove_dir_all(&root);
        Ok(())
    }

    /// No legacy dir at all (fresh install): silent no-op.
    #[test]
    fn fresh_install_reports_nothing() -> Result<(), String> {
        let root = scratch("fresh")?;
        assert!(
            migrate(
                &root.join("missing"),
                &root.join("local"),
                &Progress::none()
            )
            .is_empty()
        );
        let _ = fs::remove_dir_all(&root);
        Ok(())
    }

    /// The cross-volume fallback copies a tree intact.
    #[test]
    fn copy_preserves_tree() -> Result<(), String> {
        let root = scratch("copy")?;
        write(&root.join("src/a/b.bin"), "b")?;
        write(&root.join("src/c.txt"), "c")?;
        let progress = Progress::none();
        let mut copied = Copied {
            done: 0,
            total: 2,
            progress: &progress,
        };
        copy(&root.join("src"), &root.join("dst"), &mut copied).map_err(|e| e.to_string())?;
        assert_eq!(copied.done, 2);
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

        let report = migrate(&legacy, &data, &Progress::none());

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
        assert!(move_entry(&root.join("roaming/app.duckdb"), &to, &Progress::none()).is_err());
        assert!(!to.exists());
        assert!(!root.join("local/app.duckdb.migrating").exists());
        let _ = fs::remove_dir_all(&root);
        Ok(())
    }

    /// Leftovers of an interrupted cross-volume move are swept first: an
    /// unfinished staged copy is discarded and the entry moved again from
    /// its intact source; a set-aside source is deleted, never moved.
    #[test]
    fn sweeps_interrupted_moves() -> Result<(), String> {
        let root = scratch("sweep")?;
        let legacy = root.join("roaming");
        let data = root.join("local");
        write(&legacy.join("models/two/model.onnx"), "onnx")?;
        write(&data.join("models.migrating/two/model.onnx"), "partial")?;
        write(&legacy.join("runtimes.migrated/pack/lib.so"), "copied")?;

        let report = migrate(&legacy, &data, &Progress::none());

        assert!(!data.join("models.migrating").exists());
        assert_eq!(
            fs::read_to_string(data.join("models/two/model.onnx")).map_err(|e| e.to_string())?,
            "onnx"
        );
        assert!(!legacy.join("runtimes.migrated").exists());
        assert!(!data.join("runtimes.migrated").exists());
        assert!(report.iter().all(Result::is_ok), "{report:?}");
        let _ = fs::remove_dir_all(&root);
        Ok(())
    }

    /// The copy path ends with only the target: no staged copy, no source,
    /// no set-aside source.
    #[test]
    fn copy_across_leaves_only_target() -> Result<(), String> {
        let root = scratch("across")?;
        write(&root.join("roaming/models/two/model.onnx"), "onnx")?;
        fs::create_dir_all(root.join("local")).map_err(|e| e.to_string())?;

        copy_across(
            &root.join("roaming/models"),
            &root.join("local/models"),
            &Progress::none(),
        )?;

        assert_eq!(
            fs::read_to_string(root.join("local/models/two/model.onnx"))
                .map_err(|e| e.to_string())?,
            "onnx"
        );
        assert!(!root.join("local/models.migrating").exists());
        assert!(!root.join("roaming/models").exists());
        assert!(!root.join("roaming/models.migrated").exists());
        let _ = fs::remove_dir_all(&root);
        Ok(())
    }

    /// Cancelled: the copy stops before its first chunk, removes its staged
    /// copy and keeps the source; the migration moves no further entry.
    #[test]
    fn cancel_discards_staging_and_stops() -> Result<(), String> {
        let root = scratch("cancel")?;
        let legacy = root.join("roaming");
        let data = root.join("local");
        write(&legacy.join("models/two/model.onnx"), "onnx")?;
        write(&legacy.join("app.duckdb"), "db")?;
        fs::create_dir_all(&data).map_err(|e| e.to_string())?;
        let boot = Boot::cancelled();
        let progress = Progress::of(&boot);

        assert!(copy_across(&legacy.join("models"), &data.join("models"), &progress).is_err());
        assert!(!data.join("models.migrating").exists());
        assert!(!data.join("models").exists());
        assert!(legacy.join("models/two/model.onnx").exists());

        let report = migrate(&legacy, &data, &progress);
        assert!(report.is_empty(), "{report:?}");
        assert!(legacy.join("app.duckdb").exists());
        assert!(!data.join("app.duckdb").exists());
        let _ = fs::remove_dir_all(&root);
        Ok(())
    }
}
