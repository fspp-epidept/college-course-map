//! Dataset commands: the listing the Datasets activity consumes, the stored
//! input profile, and delete (#199). The import flow lives in `import.rs`.

use duckdb::OptionalExt;
use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, Manager as _, State};

use crate::{
    activity::{Activity, Maintenance},
    boot::{self, Boot},
    classify::{Classification, ClassifyRegistry, ClassifyState},
    db::AppDb,
    profile::InputProfile,
};

/// Stored `import_state` of a dataset whose delete has started.
const DELETING: &str = "deleting";
/// Reported (never stored) for a `deleting` dataset with no delete in flight.
const DELETE_INCOMPLETE: &str = "delete_incomplete";

/// One row in the Datasets activity tab. Timestamps are serialized as ISO-8601
/// strings rather than `chrono::DateTime` so we don't need a specta-chrono
/// integration just yet — the frontend treats them as opaque sortable strings.
#[derive(Type, Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DatasetSummary {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) source_kind: String,
    pub(crate) imported_at: String,
    /// Read straight from `datasets.row_count`, which the import worker keeps
    /// up to date (live ticks during streaming, finalized in `mark_ready`).
    /// Datasets are otherwise immutable, so there's no fallback `COUNT(*)`.
    pub(crate) row_count: i64,
    /// `importing` while the background worker is still streaming rows in,
    /// `ready` when complete, `failed` when the worker errored or the app
    /// closed mid-import. `deleting` while [`delete_dataset`] is at work on
    /// it, and `delete_incomplete` when a delete was cut off (the app closed)
    /// and is waiting to be finished. The last is computed at read time from
    /// the stored `deleting` plus the maintenance gate, never stored.
    pub(crate) import_state: String,
    pub(crate) import_error: Option<String>,
    pub(crate) classification: Classification,
}

#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri injects State by value; cannot be taken by reference at the macro layer"
)]
pub(crate) fn list_datasets(
    boot: State<'_, Boot>,
    activity: State<'_, Activity>,
    jobs: State<'_, ClassifyRegistry>,
) -> Result<Vec<DatasetSummary>, String> {
    list(&*boot.ready()?.db.ro()?, &activity, &jobs)
}

fn list(
    conn: &duckdb::Connection,
    activity: &Activity,
    jobs: &ClassifyRegistry,
) -> Result<Vec<DatasetSummary>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT d.id,
                    d.title,
                    d.source_kind,
                    strftime(d.imported_at, '%Y-%m-%dT%H:%M:%SZ') AS imported_at,
                    COALESCE(d.row_count, 0)                      AS row_count,
                    COALESCE(d.import_state, 'ready')             AS import_state,
                    d.import_error,
                    COALESCE(d.classify_state, 'idle')            AS classify_state,
                    d.classify_error,
                    d.classify_ep,
                    strftime(d.classify_updated_at, '%Y-%m-%dT%H:%M:%SZ')
             FROM datasets d
             ORDER BY d.imported_at DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            let classify_state: String = row.get(7)?;
            Ok((
                DatasetSummary {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    source_kind: row.get(2)?,
                    imported_at: row.get(3)?,
                    row_count: row.get(4)?,
                    import_state: row.get(5)?,
                    import_error: row.get(6)?,
                    classification: Classification {
                        state: ClassifyState::Idle,
                        error: row.get(8)?,
                        execution_provider: row.get(9)?,
                        updated_at: row.get(10)?,
                        progress: None,
                    },
                },
                classify_state,
            ))
        })
        .map_err(|e| e.to_string())?;
    rows.map(|item| {
        let (mut dataset, classify_state) = item.map_err(|e| e.to_string())?;
        if dataset.import_state == DELETING && !activity.is_deleting(&dataset.id) {
            DELETE_INCOMPLETE.clone_into(&mut dataset.import_state);
        }
        dataset.classification.state = ClassifyState::parse(&classify_state)?;
        dataset.classification.progress = jobs.progress(&dataset.id);
        Ok(dataset)
    })
    .collect()
}

/// The input profile the import worker stored on the dataset (profile.rs),
/// or `None` when the dataset is unknown or predates the profile. A stored
/// profile that fails to parse is an error, not `None`: the UI must not
/// present a broken profile as "not available".
#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri injects State by value; cannot be taken by reference at the macro layer"
)]
pub(crate) fn get_input_profile(
    dataset_id: String,
    boot: State<'_, Boot>,
) -> Result<Option<InputProfile>, String> {
    let conn = boot.ready()?.db.ro()?;
    let json: Option<String> = conn
        .query_row(
            "SELECT input_profile::VARCHAR FROM datasets WHERE id = ?",
            [&dataset_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .flatten();
    json.map(|j| serde_json::from_str(&j).map_err(|e| format!("parse input profile: {e}")))
        .transpose()
}

/// Delete a dataset with its courses, and its `source_files` row
/// when no other dataset uses it (#199). The original CSV on disk is never
/// touched, and neither is the results cache: classifications are keyed by
/// model and input, not by dataset, and are reused if the same courses are
/// imported again.
///
/// Refused while the dataset is importing or classifying, while
/// another dataset was derived from it, or while other maintenance runs.
/// Slow on a large dataset (seconds per million courses), so it runs on the
/// blocking pool. Deleting a dataset that is already gone is not an error,
/// and deleting one left `delete_incomplete` finishes the job.
#[tauri::command]
#[specta::specta]
pub(crate) async fn delete_dataset(app: AppHandle, dataset_id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let db = &boot::services(&app)?.db;
        delete(
            db,
            &app.state::<Activity>(),
            &app.state::<ClassifyRegistry>(),
            &dataset_id,
        )
    })
    .await
    .map_err(|e| format!("delete task panicked: {e}"))?
}

/// The whole delete. `DuckDB` can't delete a parent row and its children in
/// one transaction (the foreign-key check doesn't see the uncommitted child
/// deletes), so it is three steps, children first: no orphaned course can
/// exist at any point, and a crash between steps leaves a dataset marked
/// `deleting` that the next call finishes.
fn delete(
    db: &AppDb,
    activity: &Activity,
    jobs: &ClassifyRegistry,
    dataset_id: &str,
) -> Result<(), String> {
    let Some(_guard) = claim(db, activity, jobs, dataset_id)? else {
        return Ok(());
    };
    delete_children(db, dataset_id)?;
    delete_row(db, dataset_id)?;
    log::info!("dataset {dataset_id}: deleted");
    Ok(())
}

/// Step 1: check the dataset can go, take the maintenance slot and mark the
/// row `deleting`, all under the read-write connection. `None` when the
/// dataset doesn't exist.
fn claim<'a>(
    db: &AppDb,
    activity: &'a Activity,
    jobs: &ClassifyRegistry,
    dataset_id: &str,
) -> Result<Option<crate::activity::MaintenanceGuard<'a>>, String> {
    let conn = db.rw()?;
    let state: Option<Option<String>> = conn
        .query_row(
            "SELECT import_state FROM datasets WHERE id = ?",
            [dataset_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("dataset {dataset_id}: {e}"))?;
    let Some(state) = state else {
        return Ok(None);
    };
    if state.as_deref() == Some("importing") {
        return Err(
            "This dataset is still importing. Wait for the import to finish before deleting it."
                .to_owned(),
        );
    }
    // The registry is the truth of a job executing: `classify_dataset`
    // registers under this same read-write connection.
    if jobs.is_active(dataset_id) {
        return Err("This dataset is classifying. Stop it before deleting the dataset.".to_owned());
    }
    let dependent: Option<String> = conn
        .query_row(
            "SELECT title FROM datasets WHERE parent_dataset_id = ? OR supersedes_id = ? LIMIT 1",
            [dataset_id, dataset_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("check datasets derived from {dataset_id}: {e}"))?;
    if let Some(title) = dependent {
        return Err(format!(
            "\u{201c}{title}\u{201d} was created from this dataset. Delete it first."
        ));
    }
    let guard = activity.begin(Maintenance::DeletingDataset(dataset_id.to_owned()))?;
    conn.execute(
        "UPDATE datasets SET import_state = ?, import_error = NULL WHERE id = ?",
        [DELETING, dataset_id],
    )
    .map_err(|e| format!("mark dataset {dataset_id} deleting: {e}"))?;
    Ok(Some(guard))
}

/// Step 2: the dataset's courses.
fn delete_children(db: &AppDb, dataset_id: &str) -> Result<(), String> {
    let courses = db
        .rw()?
        .execute("DELETE FROM courses WHERE dataset_id = ?", [dataset_id])
        .map_err(|e| format!("delete courses of dataset {dataset_id}: {e}"))?;
    log::info!("dataset {dataset_id}: deleted {courses} course(s)");
    Ok(())
}

/// Step 3: the dataset row, then its source file row if nothing else uses
/// it, then a checkpoint so the deletes are folded into the main file.
fn delete_row(db: &AppDb, dataset_id: &str) -> Result<(), String> {
    let conn = db.rw()?;
    let source_file: Option<i64> = conn
        .query_row(
            "SELECT source_file_id FROM datasets WHERE id = ?",
            [dataset_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("dataset {dataset_id}: {e}"))?
        .flatten();
    conn.execute("DELETE FROM datasets WHERE id = ?", [dataset_id])
        .map_err(|e| format!("delete dataset {dataset_id}: {e}"))?;
    if let Some(id) = source_file {
        conn.execute(
            "DELETE FROM source_files
             WHERE id = ? AND NOT EXISTS (SELECT 1 FROM datasets WHERE source_file_id = ?)",
            [id, id],
        )
        .map_err(|e| format!("delete source file {id}: {e}"))?;
    }
    if let Err(e) = conn.execute_batch("CHECKPOINT") {
        log::warn!("dataset {dataset_id}: checkpoint after delete skipped: {e}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc};

    use super::{DELETE_INCOMPLETE, claim, delete, delete_children, list};
    use crate::{
        activity::{Activity, Maintenance},
        boot::Progress,
        classify::{ClassifyRegistry, Job},
        db::AppDb,
    };

    /// Two datasets sharing one source file, each with courses, and one
    /// cached result — on a scratch database file.
    fn seeded(name: &str) -> Result<AppDb, String> {
        let root: PathBuf =
            std::env::temp_dir().join(format!("ccm-datasets-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let db = AppDb::open_at(root.join("app.duckdb"), "test", &Progress::none())?;
        db.rw()?
            .execute_batch(
                "INSERT INTO source_files (id, path, display_name, imported_at, imported_hash)
                 VALUES (1, 'a.csv', 'a', now(), 'h');
                 INSERT INTO datasets
                    (id, title, source_kind, source_file_id, imported_at, row_count, import_state)
                 VALUES ('a', 'A', 'file', 1, now(), 2, 'ready'),
                        ('b', 'B', 'file', 1, now(), 1, 'ready');
                 INSERT INTO courses (dataset_id, row_index, content_hash)
                 VALUES ('a', 0, 'h0'), ('a', 1, 'h1'), ('b', 0, 'h0');
                 INSERT INTO models (id, hf_repo, hf_revision, model_type, precision)
                 VALUES (1, 'r', 'v', '6', 'f32');
                 INSERT INTO inference_results
                    (model_id, content_hash, classification, computed_at)
                 VALUES (1, 'h0', '11.0701', now());",
            )
            .map_err(|e| e.to_string())?;
        Ok(db)
    }

    fn count(db: &AppDb, sql: &str) -> Result<i64, String> {
        db.rw()?
            .query_row(sql, [], |r| r.get(0))
            .map_err(|e| format!("{sql}: {e}"))
    }

    /// Deleting a dataset removes its courses and leaves the other
    /// dataset, the shared source file and the results cache alone. The
    /// source file goes with the last dataset that used it.
    #[test]
    fn delete_removes_one_dataset_and_keeps_the_cache() -> Result<(), String> {
        let db = seeded("delete")?;
        let (activity, jobs) = (Activity::default(), ClassifyRegistry::default());

        delete(&db, &activity, &jobs, "a")?;
        activity.ensure_idle()?;
        assert_eq!(count(&db, "SELECT COUNT(*) FROM datasets")?, 1);
        assert_eq!(count(&db, "SELECT COUNT(*) FROM courses")?, 1);
        assert_eq!(count(&db, "SELECT COUNT(*) FROM source_files")?, 1);
        assert_eq!(count(&db, "SELECT COUNT(*) FROM inference_results")?, 1);

        delete(&db, &activity, &jobs, "b")?;
        assert_eq!(count(&db, "SELECT COUNT(*) FROM datasets")?, 0);
        assert_eq!(count(&db, "SELECT COUNT(*) FROM source_files")?, 0);
        assert_eq!(count(&db, "SELECT COUNT(*) FROM inference_results")?, 1);
        // Already gone: not an error.
        delete(&db, &activity, &jobs, "b")
    }

    /// A dataset that is importing, is classifying, has a dataset
    /// derived from it, or is asked for while other maintenance runs is
    /// refused, and nothing is deleted.
    #[test]
    fn delete_is_refused_while_the_dataset_is_in_use() -> Result<(), String> {
        let db = seeded("refused")?;
        let (activity, jobs) = (Activity::default(), ClassifyRegistry::default());
        let refused = |why: &str| -> Result<(), String> {
            let err = delete(&db, &activity, &jobs, "a").err().ok_or("deleted")?;
            assert!(err.contains(why), "{err}");
            assert_eq!(count(&db, "SELECT COUNT(*) FROM courses")?, 3);
            Ok(())
        };
        let set = |sql: &str| -> Result<(), String> {
            db.rw()?.execute_batch(sql).map_err(|e| e.to_string())
        };

        set("UPDATE datasets SET import_state = 'importing' WHERE id = 'a'")?;
        refused("still importing")?;
        set("UPDATE datasets SET import_state = 'ready' WHERE id = 'a'")?;

        let job = Arc::new(Job::default());
        jobs.register("a", &job)?;
        refused("is classifying")?;
        jobs.remove("a", &job);

        // Inserted, not an UPDATE of `b`: `DuckDB` rewrites a row whose
        // indexed column changes, which the courses referencing `b` forbid.
        set("INSERT INTO datasets
                (id, title, source_kind, parent_dataset_id, imported_at, row_count, import_state)
             VALUES ('c', 'C', 'derived', 'a', now(), 0, 'ready')")?;
        refused("created from this dataset")?;
        set("DELETE FROM datasets WHERE id = 'c'")?;

        let other = activity.begin(Maintenance::DeletingDataset("b".to_owned()))?;
        refused("busy deleting a dataset")?;
        drop(other);

        delete(&db, &activity, &jobs, "a")
    }

    /// A delete cut off after its children went leaves the dataset
    /// `deleting`: reported as `deleting` while the delete holds the slot,
    /// `delete_incomplete` once nothing does, and finished by calling
    /// delete again.
    #[test]
    fn interrupted_delete_is_reported_and_can_be_finished() -> Result<(), String> {
        let db = seeded("interrupted")?;
        let (activity, jobs) = (Activity::default(), ClassifyRegistry::default());
        let state_of_a = || -> Result<String, String> {
            list(&*db.rw()?, &activity, &jobs)?
                .into_iter()
                .find(|d| d.id == "a")
                .map(|d| d.import_state)
                .ok_or_else(|| "dataset a missing".to_owned())
        };

        let guard = claim(&db, &activity, &jobs, "a")?.ok_or("dataset a missing")?;
        delete_children(&db, "a")?;
        assert_eq!(state_of_a()?, "deleting");
        // The process dies here: the slot is gone, the row is not.
        drop(guard);
        assert_eq!(state_of_a()?, DELETE_INCOMPLETE);
        assert_eq!(count(&db, "SELECT COUNT(*) FROM datasets")?, 2);

        delete(&db, &activity, &jobs, "a")?;
        assert_eq!(count(&db, "SELECT COUNT(*) FROM datasets")?, 1);
        assert_eq!(count(&db, "SELECT COUNT(*) FROM courses")?, 1);
        Ok(())
    }
}
