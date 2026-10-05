//! Settings → Storage (#200, #201): what the app keeps on disk and the
//! actions that give space back — pruning cached classifications nothing
//! uses, compacting the database, and clearing leftover files.
//!
//! Sizes are measured here; only the summary crosses IPC. Paths come from
//! the modules that own them (`paths.rs` and the per-module helpers), never
//! from `dirs` directly. `session.lock`, the reset marker and `.reset-trash`
//! are other modules' files and are never listed or deleted.
//!
//! The cache has no per-bucket byte figures on purpose: most of the database
//! file is indexes, and `DuckDB` reports no per-index size, so any such
//! number would be invented. Buckets are reported in rows, and the database
//! line carries the one exact figure — the space a compaction would return.

use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager as _, State};

use crate::{
    activity::{Activity, Maintenance},
    boot::{self, Boot, Services},
    db,
    manifest::ModelCatalog,
    runs::RunRegistry,
};

#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageStatus {
    /// The data folder everything below lives in (except the `CoreML` cache).
    pub data_dir: String,
    pub database: DatabaseUsage,
    pub cache: CacheUsage,
    pub models: DirUsage,
    /// Downloaded runtime packs, one entry per ONNX Runtime version folder.
    pub runtimes: Vec<RuntimeUsage>,
    /// Compiled `CoreML` models; `None` off macOS, where none are made.
    pub coreml_cache: Option<DirUsage>,
    pub logs: DirUsage,
    /// WALs set aside after a failed replay, oldest first.
    pub set_aside_wals: Vec<FileUsage>,
    /// Pre-upgrade database backups. Each is as large as the database was.
    pub database_backups: Vec<FileUsage>,
    /// Why prune and compact can't start right now, or `None` when they can.
    pub busy: Option<String>,
}

#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DatabaseUsage {
    pub path: String,
    pub file_bytes: u64,
    pub wal_bytes: u64,
    /// Exact: the free blocks inside the file, which a compaction returns.
    pub reclaimable_bytes: u64,
}

/// Cached classifications, counted in rows.
#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CacheUsage {
    pub total: i64,
    /// Computed by a model revision this build no longer ships.
    pub superseded: i64,
    /// For an input that no dataset contains any more.
    pub unreferenced: i64,
}

#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DirUsage {
    pub path: String,
    pub bytes: u64,
}

#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimeUsage {
    pub ort_version: String,
    /// Whether this is the version the app loads packs from.
    pub current: bool,
    pub path: String,
    pub bytes: u64,
}

#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileUsage {
    pub name: String,
    pub bytes: u64,
    pub modified_at: Option<String>,
}

/// Which cached classifications [`storage_prune`] removes.
#[derive(Type, Deserialize, Debug, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PruneScope {
    /// Results of model revisions this build no longer ships. Nothing in the
    /// app can show or export them.
    SupersededModels,
    /// Results for inputs no dataset contains. They would be recomputed if
    /// the same courses were imported again.
    Unreferenced,
}

/// Leftover files [`storage_clear`] deletes.
#[derive(Type, Deserialize, Debug, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClearTarget {
    SetAsideWals,
    DatabaseBackups,
    CoremlCache,
}

/// Total size of the regular files under `dir`. Symlinks are not followed
/// and a missing folder is empty.
fn dir_bytes(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => dir_bytes(&entry.path()),
            Ok(kind) if kind.is_file() => entry.metadata().map_or(0, |m| m.len()),
            _ => 0,
        })
        .sum()
}

fn dir_usage(dir: &Path) -> DirUsage {
    DirUsage {
        path: dir.display().to_string(),
        bytes: dir_bytes(dir),
    }
}

fn file_usage(path: &Path) -> FileUsage {
    let meta = fs::metadata(path).ok();
    FileUsage {
        name: path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
        bytes: meta.as_ref().map_or(0, fs::Metadata::len),
        modified_at: meta
            .and_then(|m| m.modified().ok())
            .map(|time| DateTime::<Utc>::from(time).to_rfc3339()),
    }
}

/// One entry per version folder under the runtimes root.
fn runtime_usage(root: &Path, current_version: &str) -> Vec<RuntimeUsage> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut versions: Vec<RuntimeUsage> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| {
            let ort_version = entry.file_name().to_string_lossy().into_owned();
            RuntimeUsage {
                current: ort_version == current_version,
                ort_version,
                path: entry.path().display().to_string(),
                bytes: dir_bytes(&entry.path()),
            }
        })
        .collect();
    versions.sort_by(|a, b| a.ort_version.cmp(&b.ort_version));
    versions
}

/// The model ids this build's manifest resolves to.
fn active_model_ids(catalog: &ModelCatalog) -> Vec<i64> {
    catalog
        .levels()
        .into_iter()
        .filter_map(|level| catalog.model_id(level))
        .collect()
}

const UNREFERENCED: &str =
    "NOT EXISTS (SELECT 1 FROM courses c WHERE c.content_hash = ir.content_hash)";

/// `ir.model_id NOT IN (…)` over the active ids: numbers from the catalog,
/// never input. With no active model nothing counts as superseded.
fn superseded(active: &[i64]) -> String {
    if active.is_empty() {
        return "FALSE".to_owned();
    }
    let ids: Vec<String> = active.iter().map(i64::to_string).collect();
    format!("ir.model_id NOT IN ({})", ids.join(", "))
}

fn prune_condition(scope: PruneScope, active: &[i64]) -> String {
    match scope {
        PruneScope::SupersededModels => superseded(active),
        PruneScope::Unreferenced => UNREFERENCED.to_owned(),
    }
}

fn cache_usage(conn: &duckdb::Connection, active: &[i64]) -> Result<CacheUsage, String> {
    let count = |condition: &str| -> Result<i64, String> {
        conn.query_row(
            &format!("SELECT COUNT(*) FROM inference_results ir WHERE {condition}"),
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("count cached classifications: {e}"))
    };
    Ok(CacheUsage {
        total: count("TRUE")?,
        superseded: count(&superseded(active))?,
        unreferenced: count(UNREFERENCED)?,
    })
}

/// Free space inside the database file, in bytes.
fn reclaimable_bytes(conn: &duckdb::Connection) -> Result<u64, String> {
    let bytes: i64 = conn
        .query_row(
            "SELECT CAST(free_blocks * block_size AS BIGINT) FROM pragma_database_size()
             WHERE database_name = current_database()",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("read database size: {e}"))?;
    Ok(u64::try_from(bytes).unwrap_or(0))
}

/// `Err` with the reason while an import or a run is working. Call with the
/// read-write connection held when the answer gates a claim of the
/// maintenance slot (activity.rs).
fn ensure_no_live_work(conn: &duckdb::Connection, runs: &RunRegistry) -> Result<(), String> {
    let importing: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM datasets WHERE import_state = 'importing'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("check imports: {e}"))?;
    if importing > 0 {
        return Err("An import is running. Wait for it to finish.".to_owned());
    }
    crate::runs::ensure_no_active_run(conn, runs)
}

/// What the app keeps on disk. Off the main thread: it walks the model and
/// runtime folders and counts the cache.
#[tauri::command]
#[specta::specta]
pub(crate) async fn storage_status(app: AppHandle) -> Result<StorageStatus, String> {
    tauri::async_runtime::spawn_blocking(move || status(&app))
        .await
        .map_err(|e| format!("storage task panicked: {e}"))?
}

fn status(app: &AppHandle) -> Result<StorageStatus, String> {
    let Services { db, catalog, .. } = boot::services(app)?;
    let activity = app.state::<Activity>();
    let path = db.path();
    let (cache, reclaimable_bytes, busy) = {
        let conn = db.ro()?;
        let busy = activity
            .ensure_idle()
            .and_then(|()| ensure_no_live_work(&conn, &app.state::<RunRegistry>()))
            .err();
        (
            cache_usage(&conn, &active_model_ids(catalog))?,
            reclaimable_bytes(&conn)?,
            busy,
        )
    };
    let len = |file: PathBuf| fs::metadata(file).map_or(0, |m| m.len());
    let runtimes_root = crate::runtime::runtimes_root()?;
    let ort_version = crate::runtime::load_manifest()?.ort_version;
    Ok(StorageStatus {
        data_dir: crate::paths::data_dir()?.display().to_string(),
        database: DatabaseUsage {
            path: path.display().to_string(),
            file_bytes: len(path.to_path_buf()),
            wal_bytes: len(db::wal_path(path)),
            reclaimable_bytes,
        },
        cache,
        models: dir_usage(&crate::inference::models_root()?),
        runtimes: runtime_usage(&runtimes_root, &ort_version),
        coreml_cache: if cfg!(target_os = "macos") {
            Some(dir_usage(&crate::paths::coreml_cache_dir()?))
        } else {
            None
        },
        logs: dir_usage(&crate::logging::logs_dir()?),
        set_aside_wals: db::set_aside_wals(path)
            .iter()
            .map(|file| file_usage(file))
            .collect(),
        database_backups: db::backups(path)
            .iter()
            .map(|file| file_usage(file))
            .collect(),
        busy,
    })
}

/// Remove the cached classifications in `scope` and return how many went
/// ([`StorageStatus::cache`] says beforehand how many that is). Refused
/// while an import or run is working or other maintenance runs; the space
/// comes back when the database is next compacted.
#[tauri::command]
#[specta::specta]
pub(crate) async fn storage_prune(app: AppHandle, scope: PruneScope) -> Result<i64, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let Services { db, catalog, .. } = boot::services(&app)?;
        prune(
            db,
            &app.state::<Activity>(),
            &app.state::<RunRegistry>(),
            scope,
            &active_model_ids(catalog),
        )
    })
    .await
    .map_err(|e| format!("prune task panicked: {e}"))?
}

fn prune(
    db: &db::AppDb,
    activity: &Activity,
    runs: &RunRegistry,
    scope: PruneScope,
    active: &[i64],
) -> Result<i64, String> {
    let conn = db.rw()?;
    ensure_no_live_work(&conn, runs)?;
    let _guard = activity.begin(Maintenance::PruningCache)?;
    let removed = conn
        .execute(
            &format!(
                "DELETE FROM inference_results ir WHERE {}",
                prune_condition(scope, active)
            ),
            [],
        )
        .map_err(|e| format!("remove cached classifications: {e}"))?;
    if let Err(e) = conn.execute_batch("CHECKPOINT") {
        log::warn!("storage: checkpoint after prune skipped: {e}");
    }
    log::info!("storage: removed {removed} cached classification(s) ({scope:?})");
    Ok(i64::try_from(removed).unwrap_or(i64::MAX))
}

/// Compact the database (#201): write a fresh copy holding only the live
/// rows, then relaunch; the next start puts the copy in place of the old
/// file before opening it (`db.rs`). Refused while an import or run is
/// working or other maintenance runs. From the moment the copy starts until
/// the app exits nothing may write, so the maintenance slot is never given
/// back on success. Closing the app during the copy just abandons it.
#[tauri::command]
#[specta::specta]
pub(crate) async fn compact_database(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let db = &boot::services(&app)?.db;
        let activity = app.state::<Activity>();
        let guard = {
            let rw = db.rw()?;
            ensure_no_live_work(&rw, &app.state::<RunRegistry>())?;
            activity.begin(Maintenance::Compacting)?
        };
        // The copy gets its own connection: holding the read-write one for
        // its whole length would block the exit checkpoint.
        let conn = db.connect()?;
        db::stage_compaction(&conn, db.path()).map_err(|e| {
            format!("The database could not be compacted: {e}. Nothing was changed.")
        })?;
        drop(conn);
        guard.hold_until_exit();
        log::info!("storage: compacted copy staged; relaunching to put it in place");
        app.request_restart();
        Ok(())
    })
    .await
    .map_err(|e| format!("compaction task panicked: {e}"))?
}

/// Delete the leftover files of one kind.
#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn storage_clear(target: ClearTarget, boot: State<'_, Boot>) -> Result<(), String> {
    let path = boot.ready()?.db.path();
    match target {
        ClearTarget::SetAsideWals => remove_files(&db::set_aside_wals(path)),
        ClearTarget::DatabaseBackups => remove_files(&db::backups(path)),
        // The leaf only: there is no cache root to delete (paths.rs).
        ClearTarget::CoremlCache => {
            let dir = crate::paths::coreml_cache_dir()?;
            match fs::remove_dir_all(&dir) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    Err(format!("clear {}: {e}", dir.display()))
                }
                _ => Ok(()),
            }
        }
    }
}

fn remove_files(files: &[PathBuf]) -> Result<(), String> {
    for file in files {
        fs::remove_file(file).map_err(|e| format!("delete {}: {e}", file.display()))?;
        log::info!("storage: deleted {}", file.display());
    }
    Ok(())
}

/// Open the data folder in the platform file manager. Rust-side opener call:
/// no capability widening for the `WebView`.
#[tauri::command]
#[specta::specta]
pub(crate) fn open_data_dir() -> Result<(), String> {
    let dir = crate::paths::data_dir()?;
    tauri_plugin_opener::open_path(&dir, None::<&str>).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::{PruneScope, cache_usage, dir_bytes, prune};
    use crate::{
        activity::{Activity, Maintenance},
        boot::Progress,
        db::AppDb,
        runs::RunRegistry,
    };

    /// Model 1 is superseded, model 2 active. Hash `kept` is in a dataset,
    /// `gone` is not. One result per (model, hash).
    fn seeded(name: &str) -> Result<AppDb, String> {
        let root =
            std::env::temp_dir().join(format!("ccm-storage-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let db = AppDb::open_at(root.join("app.duckdb"), "test", &Progress::none())?;
        db.rw()?
            .execute_batch(
                "INSERT INTO datasets (id, title, source_kind, imported_at, row_count, import_state)
                 VALUES ('ds', 't', 'file', now(), 1, 'ready');
                 INSERT INTO courses (dataset_id, row_index, content_hash) VALUES ('ds', 0, 'kept');
                 INSERT INTO models (id, hf_repo, hf_revision, model_type, precision)
                 VALUES (1, 'r', 'old', '6', 'f32'), (2, 'r', 'new', '6', 'f32');
                 INSERT INTO inference_results (model_id, content_hash, classification, computed_at)
                 VALUES (1, 'kept', 'x', now()), (1, 'gone', 'x', now()),
                        (2, 'kept', 'x', now()), (2, 'gone', 'x', now());",
            )
            .map_err(|e| e.to_string())?;
        Ok(db)
    }

    fn left(db: &AppDb) -> Result<Vec<(i64, String)>, String> {
        let conn = db.rw()?;
        let mut stmt = conn
            .prepare("SELECT model_id, content_hash FROM inference_results ORDER BY 1, 2")
            .map_err(|e| e.to_string())?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())
    }

    /// The buckets count what each prune would remove, and each scope
    /// removes exactly its bucket: superseded goes
    /// by model, unreferenced by input, and a result shared with a dataset
    /// that still exists stays.
    #[test]
    fn prune_removes_exactly_its_bucket() -> Result<(), String> {
        let db = seeded("prune")?;
        let (activity, runs, active) = (Activity::default(), RunRegistry::default(), [2_i64]);

        let usage = cache_usage(&*db.rw()?, &active)?;
        assert_eq!(
            (usage.total, usage.superseded, usage.unreferenced),
            (4, 2, 2)
        );

        assert_eq!(
            prune(&db, &activity, &runs, PruneScope::SupersededModels, &active)?,
            2
        );
        assert_eq!(left(&db)?, [(2, "gone".to_owned()), (2, "kept".to_owned())]);
        assert_eq!(
            prune(&db, &activity, &runs, PruneScope::Unreferenced, &active)?,
            1
        );
        assert_eq!(left(&db)?, [(2, "kept".to_owned())]);
        activity.ensure_idle()?;

        // No active model: nothing is superseded, rather than everything.
        assert_eq!(
            prune(&db, &activity, &runs, PruneScope::SupersededModels, &[])?,
            0
        );
        assert_eq!(left(&db)?.len(), 1);
        Ok(())
    }

    /// Pruning is refused while an import or a run is working, or while
    /// other maintenance holds the slot.
    #[test]
    fn prune_is_refused_during_other_work() -> Result<(), String> {
        let db = seeded("prune-refused")?;
        let (activity, runs, active) = (Activity::default(), RunRegistry::default(), [2_i64]);
        let refused = |why: &str| -> Result<(), String> {
            let err = prune(&db, &activity, &runs, PruneScope::Unreferenced, &active)
                .err()
                .ok_or("pruned")?;
            assert!(err.contains(why), "{err}");
            assert_eq!(left(&db)?.len(), 4);
            Ok(())
        };

        db.rw()?
            .execute_batch("UPDATE datasets SET import_state = 'importing'")
            .map_err(|e| e.to_string())?;
        refused("import is running")?;
        db.rw()?
            .execute_batch("UPDATE datasets SET import_state = 'ready'")
            .map_err(|e| e.to_string())?;

        let compacting = activity.begin(Maintenance::Compacting)?;
        refused("compacting the database")?;
        drop(compacting);
        Ok(())
    }

    /// Folder sizes count regular files at any depth; a missing folder is 0.
    #[test]
    fn dir_bytes_sums_nested_files() -> Result<(), String> {
        let root =
            std::env::temp_dir().join(format!("ccm-storage-test-{}-dir", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a/b")).map_err(|e| e.to_string())?;
        std::fs::write(root.join("top"), [0_u8; 10]).map_err(|e| e.to_string())?;
        std::fs::write(root.join("a/b/deep"), [0_u8; 32]).map_err(|e| e.to_string())?;
        assert_eq!(dir_bytes(&root), 42);
        assert_eq!(dir_bytes(&root.join("missing")), 0);
        Ok(())
    }
}
