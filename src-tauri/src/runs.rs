//! Async run pipeline. `start_run` inserts a runs row in `running` state,
//! returns the run id immediately, and offloads the per-row inference loop to
//! a blocking task that ticks `runs.rows_processed` after each row so the
//! frontend can poll progress via `get_run` (see `useRun`).
//!
//! Polling-based progress is intentional: `TanStack` Query already drives
//! everything else, and the same data flow that powers the static run detail
//! tab also feeds the live progress meter. Tauri-events for run progress are
//! deferred until the broader async-pipeline pass (#124).

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
};

use chrono::Utc;
use duckdb::params;
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;

use crate::{
    db::AppDb,
    format::{CourseInput, format_input},
    inference::{LoadedModel, ModelStore, classify_batch},
    manifest::ModelCatalog,
};

/// Cancellation flags for in-flight runs, keyed by run id. A run registers an
/// `AtomicBool` when it starts and removes it once it reaches a terminal state;
/// [`pause_run`] flips the flag so the worker stops at its next batch boundary.
/// Managed as Tauri state alongside [`AppDb`] / [`InferenceRegistry`].
#[derive(Default)]
pub(crate) struct RunRegistry {
    flags: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl RunRegistry {
    /// Mutex poisoning here is benign: the only mutated state is a flag map, and
    /// a poisoned guard still holds a consistent map. Recover the inner value
    /// rather than propagate — losing the ability to pause a run on a panic in
    /// some unrelated registry call is worse than a stale lock.
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<AtomicBool>>> {
        self.flags.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Register a fresh (un-cancelled) flag for `run_id` and hand back a clone
    /// for the worker to poll.
    fn register(&self, run_id: &str) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        self.lock().insert(run_id.to_owned(), Arc::clone(&flag));
        flag
    }

    /// Flip the cancellation flag for `run_id`. Returns `false` if no such run
    /// is currently registered (already finished, never started, or wrong id).
    fn cancel(&self, run_id: &str) -> bool {
        match self.lock().get(run_id) {
            Some(flag) => {
                flag.store(true, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    /// Drop the flag once a run reaches a terminal state.
    fn remove(&self, run_id: &str) {
        self.lock().remove(run_id);
    }

    /// Whether a worker is currently registered for `run_id`. Registry
    /// membership is the in-memory truth of "actually executing" — a
    /// `running` DB row without a registered flag is a crash leftover, not
    /// an active run.
    fn is_active(&self, run_id: &str) -> bool {
        self.lock().contains_key(run_id)
    }
}

/// Rows per DB round-trip in the run worker (EPI-89): one cache anti-join
/// query, one Appender open + bulk insert, and one progress `UPDATE runs` per
/// super-chunk of this many rows — the "flush ~1000 results" write-batching
/// ground rule. Deliberately independent of `inference::batch_size` (the
/// ONNX/GPU granularity inside a super-chunk): coupling them left the GPU
/// idle on DB round-trips two-thirds of the time. Progress-update cadence is
/// `FLUSH_SIZE` rows: ~0.35 s on CUDA, ~7 s worst-case on CPU.
/// Public so `check_resume` can derive its kill threshold from the real
/// flush cadence instead of drifting.
pub const FLUSH_SIZE: usize = 1024;

/// One row in the Runs sidebar list. Joined with the dataset title so the UI
/// doesn't need a second IPC call to render a meaningful label.
#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunSummary {
    pub id: String,
    pub dataset_id: String,
    pub dataset_title: String,
    pub description: Option<String>,
    pub state: String,
    /// Resolved from the run's first `model_ids` entry so a list row can say
    /// which model it was without a second IPC call (EPI-69).
    pub digit_level: Option<u8>,
    pub rows_total: Option<i64>,
    pub rows_processed: Option<i64>,
    pub cache_hits: Option<i64>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub last_progress_at: Option<String>,
    pub resume_count: i64,
    /// Whether `resume_run` would accept this run right now (EPI-69).
    /// Computed at read time — never persisted — so external changes (models
    /// swapped, files deleted) are reflected immediately.
    pub resumable: bool,
    /// Machine-stable reasons when an `interrupted` run can't resume
    /// (`model_superseded`, `model_not_loaded`). Empty when resumable, and
    /// for states resume doesn't apply to.
    pub resume_blockers: Vec<String>,
}

/// Full run detail for the run-tab body. Same shape as [`RunSummary`] plus the
/// model digit level (resolved from the JSON `model_ids` array) and the
/// `unique_inputs_done` + `error_message` fields the summary view drops.
#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunDetail {
    pub id: String,
    pub dataset_id: String,
    pub dataset_title: String,
    pub description: Option<String>,
    pub state: String,
    pub digit_level: Option<u8>,
    pub rows_total: Option<i64>,
    pub rows_processed: Option<i64>,
    pub unique_inputs_done: Option<i64>,
    pub cache_hits: Option<i64>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub last_progress_at: Option<String>,
    pub error_message: Option<String>,
    pub execution_provider: Option<String>,
    pub resume_count: i64,
    pub resumable: bool,
    pub resume_blockers: Vec<String>,
}

/// Read-time resumability check (EPI-69). A run is resumable iff it stopped
/// as `interrupted` AND its recorded model is still the manifest-active row
/// for its digit level AND that model is actually loaded. The model-family
/// check matters: resuming an old-family run with the currently loaded model
/// would write new-model outputs under the old model's cache key — silent
/// data corruption. Blockers are machine-stable keys the frontend translates.
fn assess_resumability(
    state: &str,
    model_id: Option<i64>,
    digit_level: Option<u8>,
    catalog: &ModelCatalog,
    store: &ModelStore,
) -> (bool, Vec<String>) {
    if state != "interrupted" {
        return (false, Vec::new());
    }
    let mut blockers = Vec::new();
    match (model_id, digit_level) {
        (Some(id), Some(level)) if catalog.model_id(level) == Some(id) => {
            let loaded = store
                .get()
                .is_some_and(|registry| registry.by_digit_level(level).is_some());
            if !loaded {
                // Distinguish "the startup autoload is still churning" from
                // "no models on this machine": the first resolves itself in
                // moments and needs no user action, the second needs the
                // Models panel. Collapsing them told users to go download
                // models that were already loading.
                blockers.push(
                    if store.is_loading() {
                        "model_loading"
                    } else {
                        "model_not_loaded"
                    }
                    .to_owned(),
                );
            }
        }
        _ => blockers.push("model_superseded".to_owned()),
    }
    (blockers.is_empty(), blockers)
}

/// Actionable "can't run inference yet" error, matching the loading state:
/// during the startup autoload no user action is needed; otherwise the
/// Models panel is the fix.
fn models_unready_message(store: &ModelStore) -> String {
    if store.is_loading() {
        "models are still loading — try again in a moment".to_owned()
    } else {
        "models not loaded — download/load them from the Models panel".to_owned()
    }
}

#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri injects State by value; cannot be taken by reference at the macro layer"
)]
pub(crate) fn list_runs(
    db: State<'_, AppDb>,
    store: State<'_, ModelStore>,
    catalog: State<'_, ModelCatalog>,
) -> Result<Vec<RunSummary>, String> {
    let conn = db.ro()?;
    // Active states float to the top, then most-recent first within each
    // ordering bucket. The frontend further regroups by state but the
    // ordering inside each group should be useful as-is. The LEFT JOIN on
    // models resolves each run's digit level in one pass (`model_ids` always
    // carries exactly one id during the spike; see `RunDetail`).
    let mut stmt = conn
        .prepare(
            "SELECT r.id, r.dataset_id, d.title, r.description, r.state,
                    r.rows_total, r.rows_processed, r.cache_hits,
                    strftime(r.created_at, '%Y-%m-%dT%H:%M:%SZ'),
                    strftime(r.started_at, '%Y-%m-%dT%H:%M:%SZ'),
                    strftime(r.completed_at, '%Y-%m-%dT%H:%M:%SZ'),
                    strftime(r.last_progress_at, '%Y-%m-%dT%H:%M:%SZ'),
                    r.resume_count, m.id, m.model_type
             FROM runs r
             JOIN datasets d ON d.id = r.dataset_id
             LEFT JOIN models m
               ON m.id = TRY_CAST(json_extract_string(r.model_ids, '$[0]') AS BIGINT)
             ORDER BY
                CASE r.state
                    WHEN 'running' THEN 0
                    WHEN 'pending' THEN 1
                    WHEN 'paused'  THEN 2
                    ELSE 3
                END,
                r.created_at DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            let model_id: Option<i64> = row.get(13)?;
            let model_type: Option<String> = row.get(14)?;
            Ok((
                RunSummary {
                    id: row.get(0)?,
                    dataset_id: row.get(1)?,
                    dataset_title: row.get(2)?,
                    description: row.get(3)?,
                    state: row.get(4)?,
                    digit_level: None,
                    rows_total: row.get(5)?,
                    rows_processed: row.get(6)?,
                    cache_hits: row.get(7)?,
                    created_at: row.get(8)?,
                    started_at: row.get(9)?,
                    completed_at: row.get(10)?,
                    last_progress_at: row.get(11)?,
                    resume_count: row.get(12)?,
                    resumable: false,
                    resume_blockers: Vec::new(),
                },
                model_id,
                model_type,
            ))
        })
        .map_err(|e| e.to_string())?;
    rows.map(|item| {
        let (mut summary, model_id, model_type) = item.map_err(|e| e.to_string())?;
        summary.digit_level = model_type.and_then(|t| t.parse::<u8>().ok());
        let (resumable, blockers) = assess_resumability(
            &summary.state,
            model_id,
            summary.digit_level,
            &catalog,
            &store,
        );
        summary.resumable = resumable;
        summary.resume_blockers = blockers;
        Ok(summary)
    })
    .collect()
}

/// Intermediate row shape for the run detail query. Stays private so the
/// public type ([`RunDetail`]) doesn't carry the JSON `model_ids` string.
struct RunRow {
    id: String,
    dataset_id: String,
    dataset_title: String,
    description: Option<String>,
    state: String,
    model_ids_json: String,
    rows_total: Option<i64>,
    rows_processed: Option<i64>,
    unique_inputs_done: Option<i64>,
    cache_hits: Option<i64>,
    created_at: String,
    started_at: Option<String>,
    completed_at: Option<String>,
    last_progress_at: Option<String>,
    error_message: Option<String>,
    execution_provider: Option<String>,
    resume_count: i64,
}

/// Shared SELECT + row-mapping for the run-detail commands. `filter_sql` is a
/// hardcoded WHERE/ORDER tail (never user input) with exactly one `?` bound to
/// `param`. Returns `None` when no row matches. The second tuple element is
/// the run's recorded model id, for the caller's resumability assessment.
fn query_run_detail(
    conn: &duckdb::Connection,
    filter_sql: &str,
    param: &str,
) -> Result<Option<(RunDetail, Option<i64>)>, String> {
    let sql = format!(
        "SELECT r.id, r.dataset_id, d.title, r.description, r.state, r.model_ids,
                r.rows_total, r.rows_processed, r.unique_inputs_done, r.cache_hits,
                strftime(r.created_at, '%Y-%m-%dT%H:%M:%SZ'),
                strftime(r.started_at, '%Y-%m-%dT%H:%M:%SZ'),
                strftime(r.completed_at, '%Y-%m-%dT%H:%M:%SZ'),
                strftime(r.last_progress_at, '%Y-%m-%dT%H:%M:%SZ'),
                r.error_message, r.execution_provider, r.resume_count
         FROM runs r
         JOIN datasets d ON d.id = r.dataset_id
         {filter_sql}"
    );
    let mut stmt = stmt_err(conn.prepare(&sql), "prepare run detail")?;
    let mut rows = stmt_err(stmt.query(params![param]), "query run detail")?;
    let Some(row) = stmt_err(rows.next(), "read run detail")? else {
        return Ok(None);
    };
    let row = stmt_err(
        (|| -> Result<RunRow, duckdb::Error> {
            Ok(RunRow {
                id: row.get(0)?,
                dataset_id: row.get(1)?,
                dataset_title: row.get(2)?,
                description: row.get(3)?,
                state: row.get(4)?,
                model_ids_json: row.get(5)?,
                rows_total: row.get(6)?,
                rows_processed: row.get(7)?,
                unique_inputs_done: row.get(8)?,
                cache_hits: row.get(9)?,
                created_at: row.get(10)?,
                started_at: row.get(11)?,
                completed_at: row.get(12)?,
                last_progress_at: row.get(13)?,
                error_message: row.get(14)?,
                execution_provider: row.get(15)?,
                resume_count: row.get(16)?,
            })
        })(),
        "map run detail",
    )?;

    // Resolve the model + digit level from the first id in `model_ids` (JSON
    // array). Runs always carry exactly one model id during the spike, so
    // taking first() is fine until multi-model runs land.
    let (model_id, digit_level) = resolve_model(conn, &row.model_ids_json).unwrap_or((None, None));

    Ok(Some((
        RunDetail {
            id: row.id,
            dataset_id: row.dataset_id,
            dataset_title: row.dataset_title,
            description: row.description,
            state: row.state,
            digit_level,
            rows_total: row.rows_total,
            rows_processed: row.rows_processed,
            unique_inputs_done: row.unique_inputs_done,
            cache_hits: row.cache_hits,
            created_at: row.created_at,
            started_at: row.started_at,
            completed_at: row.completed_at,
            last_progress_at: row.last_progress_at,
            error_message: row.error_message,
            execution_provider: row.execution_provider,
            resume_count: row.resume_count,
            resumable: false,
            resume_blockers: Vec::new(),
        },
        model_id,
    )))
}

/// Stamp read-time resumability onto a mapped detail row.
fn finish_run_detail(
    (mut detail, model_id): (RunDetail, Option<i64>),
    catalog: &ModelCatalog,
    store: &ModelStore,
) -> RunDetail {
    let (resumable, blockers) =
        assess_resumability(&detail.state, model_id, detail.digit_level, catalog, store);
    detail.resumable = resumable;
    detail.resume_blockers = blockers;
    detail
}

fn stmt_err<T, E: std::fmt::Display>(res: Result<T, E>, ctx: &str) -> Result<T, String> {
    res.map_err(|e| format!("{ctx}: {e}"))
}

#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn get_run(
    id: String,
    db: State<'_, AppDb>,
    store: State<'_, ModelStore>,
    catalog: State<'_, ModelCatalog>,
) -> Result<RunDetail, String> {
    let conn = db.ro()?;
    query_run_detail(&conn, "WHERE r.id = ?", &id)?
        .map(|found| finish_run_detail(found, &catalog, &store))
        .ok_or_else(|| format!("run {id}: not found"))
}

/// Most recent run for a dataset, or `None` if the dataset has never been
/// classified. The dataset tab's run surface card derives from this — backend
/// state, not component memory — so it survives tab close/reopen and app
/// restart (EPI-68).
#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn get_latest_run(
    dataset_id: String,
    db: State<'_, AppDb>,
    store: State<'_, ModelStore>,
    catalog: State<'_, ModelCatalog>,
) -> Result<Option<RunDetail>, String> {
    let conn = db.ro()?;
    Ok(query_run_detail(
        &conn,
        "WHERE r.dataset_id = ? ORDER BY r.created_at DESC LIMIT 1",
        &dataset_id,
    )?
    .map(|found| finish_run_detail(found, &catalog, &store)))
}

/// `(models.id, digit_level)` for the run's first `model_ids` entry.
fn resolve_model(
    conn: &duckdb::Connection,
    model_ids_json: &str,
) -> Result<(Option<i64>, Option<u8>), String> {
    let ids: Vec<i64> =
        serde_json::from_str(model_ids_json).map_err(|e| format!("parse model_ids: {e}"))?;
    let Some(first) = ids.first() else {
        return Ok((None, None));
    };
    let model_type: Option<String> = conn
        .query_row(
            "SELECT model_type FROM models WHERE id = ?",
            params![first],
            |row| row.get(0),
        )
        .ok();
    Ok((Some(*first), model_type.and_then(|s| s.parse::<u8>().ok())))
}

#[derive(Type, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartRunRequest {
    pub dataset_id: String,
    /// 2, 4, or 6. Maps to a row in the `models` table on the Rust side; the
    /// spike avoids forcing the frontend to know surrogate model ids.
    pub digit_level: u8,
}

/// Response from `start_run`: the run has been queued and is already updating
/// its own row. The frontend polls `get_run(run_id)` from here.
#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartRunResponse {
    pub run_id: String,
    pub rows_total: i64,
}

/// Reject when another worker is actually executing. A `running` row only
/// counts if its worker is registered — a row without a flag is a crash
/// leftover, which must not wedge the app ([`sweep_orphaned_runs`] flips
/// those to `interrupted` at startup).
fn ensure_no_active_run(conn: &duckdb::Connection, registry: &RunRegistry) -> Result<(), String> {
    let mut stmt = conn
        .prepare(
            "SELECT r.id, d.title FROM runs r
             JOIN datasets d ON d.id = r.dataset_id
             WHERE r.state = 'running'",
        )
        .map_err(|e| format!("prepare active-run check: {e}"))?;
    let running: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|e| format!("query active-run check: {e}"))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("collect active-run check: {e}"))?;
    if let Some((_, title)) = running.into_iter().find(|(id, _)| registry.is_active(id)) {
        return Err(format!(
            "a classification run is already active on \u{201c}{title}\u{201d} — pause it or wait for it to finish"
        ));
    }
    Ok(())
}

/// Crash recovery, run once at startup before any command can fire (EPI-38).
/// A `running` row in a fresh process is by definition orphaned — its worker
/// died with the previous process. Flip to `interrupted` (resumable): the
/// totals on the row are whatever the last batch flush committed, which is
/// consistent by construction, and everything flushed lives in the cache, so
/// resume skips it. Returns how many rows were swept.
pub fn sweep_orphaned_runs(conn: &duckdb::Connection) -> Result<usize, String> {
    conn.execute(
        "UPDATE runs SET state = 'interrupted', last_progress_at = ? WHERE state = 'running'",
        params![Utc::now().to_rfc3339()],
    )
    .map_err(|e| format!("sweep orphaned runs: {e}"))
}

#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn start_run(
    req: StartRunRequest,
    app: AppHandle,
    db: State<'_, AppDb>,
    store: State<'_, ModelStore>,
    catalog: State<'_, ModelCatalog>,
    runs: State<'_, RunRegistry>,
) -> Result<StartRunResponse, String> {
    // Fail fast if models aren't ready — caller gets a synchronous error
    // rather than a "queued then mysteriously failed" run. The store starts
    // empty on a connected-build first run (EPI-56) until download + load.
    let Some(registry) = store.get() else {
        return Err(models_unready_message(&store));
    };
    if registry.by_digit_level(req.digit_level).is_none() {
        return Err(format!(
            "no model loaded for digit_level={}",
            req.digit_level
        ));
    }

    let conn = db.rw()?;

    // Verify the dataset still exists before we try to FK-reference it from a
    // new runs row. Stale frontend state (e.g. a tab persisted across a
    // db:clear-data) would otherwise surface the raw DuckDB FK violation,
    // which is unhelpful — turn it into an actionable error.
    let dataset_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM datasets WHERE id = ?",
            params![req.dataset_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("check dataset {}: {e}", req.dataset_id))?;
    if dataset_exists == 0 {
        return Err(format!(
            "dataset {} no longer exists (the tab may be stale — close it and reopen from the sidebar)",
            req.dataset_id
        ));
    }

    // One run at a time, app-wide (EPI-68 decision, 2026-07-03; queued runs
    // are EPI-70). The check runs on the held RW connection, so it's atomic
    // with the INSERT below against concurrent start_run calls.
    ensure_no_active_run(&conn, &runs)?;

    // Resolve the models-table row through the manifest catalog — never by
    // guessing with SQL, which would happily pick a stale row from an
    // earlier model family.
    let model_id: i64 = catalog
        .model_id(req.digit_level)
        .ok_or_else(|| format!("manifest has no model for digit_level {}", req.digit_level))?;

    // A run always covers the whole dataset (EPI-66); the count is the
    // progress meter's denominator.
    let course_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM courses WHERE dataset_id = ?",
            params![req.dataset_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("count courses for dataset {}: {e}", req.dataset_id))?;
    if course_count == 0 {
        return Err(format!(
            "dataset {} has no courses to classify",
            req.dataset_id
        ));
    }
    let rows_total = course_count;

    let run_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO runs
            (id, dataset_id, description, state, model_ids,
             rows_total, rows_processed,
             unique_inputs_total, unique_inputs_done, cache_hits,
             created_at, started_at, last_progress_at, execution_provider)
         VALUES (?, ?, ?, 'running', ?,
                 ?, 0,
                 ?, 0, 0,
                 ?, ?, ?, ?)",
        params![
            run_id,
            req.dataset_id,
            format!("Spike run: {}-digit", req.digit_level),
            serde_json::to_string(&[model_id]).map_err(|e| e.to_string())?,
            rows_total,
            rows_total,
            now,
            now,
            now,
            registry.execution_provider().as_str(),
        ],
    )
    .map_err(|e| format!("insert runs: {e}"))?;

    // Release the RW lock before spawning so the background task can re-acquire
    // it without blocking on `db.rw()` inside the closure.
    drop(conn);

    // Register the cancellation flag before the worker starts so a pause request
    // arriving immediately after this returns can't race the registration.
    let cancel = runs.register(&run_id);

    let task = RunTask {
        app: app.clone(),
        pipeline: RunPipeline {
            dataset_id: req.dataset_id,
            run_id: run_id.clone(),
            model_id,
            digit_level: req.digit_level,
            computed_at: now,
            cancel,
        },
    };
    // spawn_blocking owns the synchronous ORT calls. tauri::async_runtime
    // dispatches the closure onto the runtime's blocking pool, which doesn't
    // starve the async executor that handles other IPC calls (notably the
    // polling `get_run`).
    tauri::async_runtime::spawn_blocking(move || task.run());

    Ok(StartRunResponse { run_id, rows_total })
}

/// Resume an `interrupted` run (EPI-38). Same worker, same run id: the row
/// flips back to `running` with `resume_count` bumped, and the pipeline
/// selects only the courses still missing a result for this model — progress
/// continues from where it stopped (`total - remaining`), never from zero.
/// Idempotent by construction: anything already in `inference_results` is
/// never recomputed.
#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn resume_run(
    run_id: String,
    app: AppHandle,
    db: State<'_, AppDb>,
    store: State<'_, ModelStore>,
    catalog: State<'_, ModelCatalog>,
    runs: State<'_, RunRegistry>,
) -> Result<StartRunResponse, String> {
    let conn = db.rw()?;

    let (state, dataset_id, model_ids_json, rows_total): (String, String, String, Option<i64>) =
        conn.query_row(
            "SELECT state, dataset_id, model_ids, rows_total FROM runs WHERE id = ?",
            params![run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|e| format!("run {run_id}: {e}"))?;

    if state != "interrupted" {
        return Err(format!(
            "run is {state}, not interrupted — only interrupted runs can resume"
        ));
    }
    ensure_no_active_run(&conn, &runs)?;

    // Same checks assess_resumability advertises (EPI-69) — the command is
    // the enforcement point, the flags are the preview.
    let (model_id, digit_level) = resolve_model(&conn, &model_ids_json)?;
    let (Some(model_id), Some(digit_level)) = (model_id, digit_level) else {
        return Err("run has no resolvable model".to_owned());
    };
    if catalog.model_id(digit_level) != Some(model_id) {
        return Err(
            "this run's model is no longer the app-active model — start a new run instead \
             (its finished classifications stay in the cache)"
                .to_owned(),
        );
    }
    let Some(registry) = store.get() else {
        return Err(models_unready_message(&store));
    };
    if registry.by_digit_level(digit_level).is_none() {
        return Err(models_unready_message(&store));
    }

    let now = Utc::now().to_rfc3339();
    // execution_provider reflects where the run's inference *last* executed —
    // a resume may land on a different EP than the original attempt (pack
    // downloaded since, settings reordered), so record the current one.
    conn.execute(
        "UPDATE runs SET state = 'running', resume_count = resume_count + 1,
                error_message = NULL, last_progress_at = ?, execution_provider = ?
         WHERE id = ?",
        params![now, registry.execution_provider().as_str(), run_id],
    )
    .map_err(|e| format!("mark run resuming: {e}"))?;
    drop(conn);

    let cancel = runs.register(&run_id);
    let task = RunTask {
        app: app.clone(),
        pipeline: RunPipeline {
            dataset_id,
            run_id: run_id.clone(),
            model_id,
            digit_level,
            computed_at: now,
            cancel,
        },
    };
    tauri::async_runtime::spawn_blocking(move || task.run());

    Ok(StartRunResponse {
        run_id,
        rows_total: rows_total.unwrap_or(0),
    })
}

/// Request a graceful pause of an in-flight run. The worker stops at its next
/// batch boundary — after the current batch's results and progress are flushed
/// in the usual transaction — and finalizes the run as `interrupted` (resumable
/// later; see EPI-38/EPI-39). Returns `true` if a running worker was signalled,
/// `false` if the run wasn't active (already terminal, or unknown id).
#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn pause_run(run_id: String, registry: State<'_, RunRegistry>) -> bool {
    registry.cancel(&run_id)
}

/// Tauri-side wrapper for the background worker: resolves managed state,
/// hands the pipeline its model + database handles, and cleans up the
/// registry when the run reaches a terminal state. Owned values only so the
/// spawned closure has no borrowed state to outlive.
struct RunTask {
    app: AppHandle,
    pipeline: RunPipeline,
}

impl RunTask {
    fn run(self) {
        let db = self.app.state::<AppDb>();
        let outcome = (|| {
            // Clone the registry Arc out of the store once — the worker keeps
            // this snapshot for the whole run even if the store changes later.
            let registry = self
                .app
                .state::<ModelStore>()
                .get()
                .ok_or_else(|| "models not loaded (worker)".to_owned())?;
            let model = registry
                .by_digit_level(self.pipeline.digit_level)
                .ok_or_else(|| {
                    format!(
                        "no model loaded for digit_level={} (worker)",
                        self.pipeline.digit_level
                    )
                })?;
            self.pipeline.execute(&db, model)
        })();
        self.pipeline.finalize(&db, outcome);

        // Run is terminal whichever branch ran: drop its cancel flag so the
        // registry doesn't leak an entry per completed run.
        self.app
            .state::<RunRegistry>()
            .remove(&self.pipeline.run_id);
    }
}

/// One selected course row: `(content_hash, subject_code, catalog_number,
/// course_title)` — exactly the fields the loop formats and hashes against.
type SelectedCourse = (String, String, String, String);

/// The inference loop and its persistence, decoupled from Tauri managed state
/// so the resume verification harness (`examples/check_resume.rs`, EPI-39)
/// can drive the *real* pipeline — same batching, same flush transactions,
/// same pause semantics — against a scratch database.
#[derive(Debug)]
pub struct RunPipeline {
    pub dataset_id: String,
    pub run_id: String,
    pub model_id: i64,
    pub digit_level: u8,
    /// Stamped as `computed_at` on every inference row this leg writes.
    pub computed_at: String,
    /// Set true by [`pause_run`]; polled at each batch boundary to stop early.
    pub cancel: Arc<AtomicBool>,
}

impl RunPipeline {
    /// Finalize the runs row from the loop's outcome. Catches every branch —
    /// completed, interrupted, failed — so the row always leaves `running`
    /// unless the process itself dies (which the startup sweep repairs).
    pub fn finalize(&self, db: &AppDb, outcome: Result<RunOutcome, String>) {
        let finished_at = Utc::now().to_rfc3339();

        let Ok(conn) = db.rw() else {
            // Mutex poisoning is unrecoverable; the run row stays in
            // 'running' which is technically wrong, but the alternative
            // (panic in a worker thread) is worse. The cancel flag is left in
            // the registry — also unrecoverable, and harmless.
            eprintln!("run {}: rw mutex poisoned", self.run_id);
            return;
        };
        match outcome {
            Ok(o) if o.interrupted => {
                // Graceful pause: in-flight work was already flushed by the last
                // batch's transaction. Persist the running totals and stop in
                // `interrupted` (resumable) without stamping completed_at.
                if let Err(e) = conn.execute(
                    "UPDATE runs SET state='interrupted',
                        rows_processed=?,
                        unique_inputs_total=?, unique_inputs_done=?, cache_hits=?,
                        last_progress_at=?
                     WHERE id=?",
                    params![
                        i64::try_from(o.processed).unwrap_or(i64::MAX),
                        i64::try_from(o.unique_done + o.cache_hits).unwrap_or(i64::MAX),
                        i64::try_from(o.unique_done).unwrap_or(i64::MAX),
                        i64::try_from(o.cache_hits).unwrap_or(i64::MAX),
                        &finished_at,
                        &self.run_id,
                    ],
                ) {
                    eprintln!("run {}: interrupt finalize: {e}", self.run_id);
                }
            }
            Ok(o) => {
                if let Err(e) = conn.execute(
                    "UPDATE runs SET state='completed',
                        rows_processed=?,
                        unique_inputs_total=?, unique_inputs_done=?, cache_hits=?,
                        completed_at=?, last_progress_at=?
                     WHERE id=?",
                    params![
                        i64::try_from(o.processed).unwrap_or(i64::MAX),
                        i64::try_from(o.unique_done + o.cache_hits).unwrap_or(i64::MAX),
                        i64::try_from(o.unique_done).unwrap_or(i64::MAX),
                        i64::try_from(o.cache_hits).unwrap_or(i64::MAX),
                        &finished_at,
                        &finished_at,
                        &self.run_id,
                    ],
                ) {
                    eprintln!("run {}: finalize: {e}", self.run_id);
                }
            }
            Err(err) => {
                let _ = conn.execute(
                    "UPDATE runs SET state='failed', error_message=?, completed_at=? WHERE id=?",
                    params![&err, &finished_at, &self.run_id],
                );
            }
        }
    }

    /// The batched inference loop. Selects only courses **without** a cached
    /// result for this model (anti-join) and starts the progress counters at
    /// `total - remaining` — everything already cached is accounted for up
    /// front instead of re-walked row by row. This is what makes resume
    /// *continue* from where it stopped (EPI-38) rather than visibly restart
    /// at zero, and makes a fully-cached run complete near-instantly.
    pub fn execute(&self, db: &AppDb, model: &LoadedModel) -> Result<RunOutcome, String> {
        let (total, rows) = self.select_missing_courses(db)?;
        let remaining = rows.len() as u64;
        let mut processed = total.saturating_sub(remaining);
        let mut cache_hits = processed;
        let mut unique_done = 0_u64;

        // Surface the starting position immediately — before the first batch
        // computes — so a resumed run's progress bar never reads zero.
        {
            let conn = db.rw()?;
            self.flush_progress(&conn, processed, cache_hits)?;
        }

        // Two granularities, deliberately decoupled (EPI-89): DB work (cache
        // anti-join query, Appender insert, progress UPDATE) runs once per
        // FLUSH_SIZE super-chunk — the "~1000 results per flush"
        // write-batching ground rule — while the ONNX call inside runs at the
        // per-EP batch size (EPI-82). At GPU speeds, per-ONNX-chunk DB
        // round-trips left the GPU idle most of the time (~1k rows/s in-app
        // vs ~2.9k standalone, measured 2026-07-28).
        let batch = crate::inference::batch_size(model.resolved_ep);
        for chunk in rows.chunks(FLUSH_SIZE) {
            // 1. Bulk cache check — catches hashes computed earlier in *this*
            //    leg (duplicate course content across super-chunks). The
            //    anti-join snapshot was taken before the loop started, so it
            //    can't see them.
            let cached = self.cache_hit_batch(db, chunk)?;

            // 2. Format only the misses, deduplicating within the super-chunk:
            //    two copies of the same course would otherwise both classify
            //    and collide on the cache's (model_id, content_hash) primary
            //    key at flush. The second copy rides on the first's result, so
            //    it counts as a hit. `Miss` borrows `content_hash` from
            //    `chunk` so flush_batch can pair it with its classification
            //    without indexing back into the chunk later.
            let mut seen_in_chunk = std::collections::HashSet::new();
            let misses: Vec<Miss<'_>> = chunk
                .iter()
                .zip(cached.iter())
                .filter(|((content_hash, ..), hit)| {
                    !**hit && seen_in_chunk.insert(content_hash.as_str())
                })
                .map(|((content_hash, subject, catalog, title), _)| Miss {
                    content_hash: content_hash.as_str(),
                    input: format_input(&CourseInput {
                        subject_code: subject.clone(),
                        catalog_number: catalog.clone(),
                        course_title: title.clone(),
                    }),
                })
                .collect();
            let miss_refs: Vec<&str> = misses.iter().map(|m| m.input.as_str()).collect();

            // 3. ONNX session calls in per-EP sub-batches, honoring a pause
            //    request between calls so pause latency stays one ONNX call,
            //    not one super-chunk.
            let mut classifications = Vec::with_capacity(misses.len());
            let mut cancelled = false;
            for sub in miss_refs.chunks(batch) {
                let batch_results =
                    classify_batch(model, sub).map_err(|e| format!("classify_batch: {e}"))?;
                classifications.extend(batch_results);
                // Relaxed is fine — we only need eventual visibility of the
                // flag, not ordering against other memory.
                if self.cancel.load(Ordering::Relaxed) {
                    cancelled = true;
                    break;
                }
            }

            // Pause mid-super-chunk: flush the classified prefix (the
            // misses/classifications zip in flush_batch truncates to it)
            // without advancing row progress — the resume anti-join re-counts
            // those rows as cache hits, so counters stay consistent by
            // construction, same as crash recovery.
            if cancelled && classifications.len() < misses.len() {
                unique_done += classifications.len() as u64;
                self.flush_batch(db, &misses, &classifications, processed, cache_hits)?;
                return Ok(RunOutcome {
                    processed,
                    unique_done,
                    cache_hits,
                    interrupted: true,
                });
            }

            // 4. Bulk insert via Appender + single progress UPDATE, both under
            //    one RW-mutex acquire per super-chunk.
            let chunk_hits = chunk.len() - misses.len();
            let processed_after = processed + chunk.len() as u64;
            let cache_hits_after = cache_hits + chunk_hits as u64;
            self.flush_batch(
                db,
                &misses,
                &classifications,
                processed_after,
                cache_hits_after,
            )?;

            processed = processed_after;
            cache_hits = cache_hits_after;
            unique_done += classifications.len() as u64;

            if cancelled {
                return Ok(RunOutcome {
                    processed,
                    unique_done,
                    cache_hits,
                    interrupted: true,
                });
            }
        }
        Ok(RunOutcome {
            processed,
            unique_done,
            cache_hits,
            interrupted: false,
        })
    }

    /// `(dataset course count, courses with no cached result for this model)`
    /// in row order. The anti-join is what keeps a resumed (or re-run) leg
    /// from materializing and re-walking work that's already done; it also
    /// shrinks the in-memory selection to the actual remainder (relevant to
    /// EPI-67).
    fn select_missing_courses(&self, db: &AppDb) -> Result<(u64, Vec<SelectedCourse>), String> {
        let conn = db.rw()?;
        let total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM courses WHERE dataset_id = ?",
                params![&self.dataset_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("count courses: {e}"))?;
        let mut stmt = conn
            .prepare(
                "SELECT c.content_hash, c.subject_code, c.catalog_number, c.course_title
                 FROM courses c
                 WHERE c.dataset_id = ?
                   AND NOT EXISTS (
                       SELECT 1 FROM inference_results ir
                       WHERE ir.model_id = ? AND ir.content_hash = c.content_hash
                   )
                 ORDER BY c.row_index",
            )
            .map_err(|e| format!("prepare select courses: {e}"))?;
        let rows = stmt
            .query_map(params![&self.dataset_id, &self.model_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .map_err(|e| format!("query courses: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("collect courses: {e}"))?;
        Ok((u64::try_from(total).unwrap_or(0), rows))
    }

    /// Returns a `Vec<bool>` aligned with `chunk`: true if the
    /// `(model_id, content_hash)` pair is already in `inference_results`.
    fn cache_hit_batch(&self, db: &AppDb, chunk: &[SelectedCourse]) -> Result<Vec<bool>, String> {
        if chunk.is_empty() {
            return Ok(Vec::new());
        }
        let conn = db.ro()?;
        let mut placeholders = String::with_capacity(chunk.len() * 2);
        for i in 0..chunk.len() {
            if i > 0 {
                placeholders.push(',');
            }
            placeholders.push('?');
        }
        let sql = format!(
            "SELECT content_hash FROM inference_results
             WHERE model_id = ? AND content_hash IN ({placeholders})"
        );
        let mut sql_params: Vec<&dyn duckdb::ToSql> = Vec::with_capacity(chunk.len() + 1);
        sql_params.push(&self.model_id);
        for (h, ..) in chunk {
            sql_params.push(h);
        }
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("prepare cache_hit_batch: {e}"))?;
        let hits: std::collections::HashSet<String> = stmt
            .query_map(duckdb::params_from_iter(sql_params), |r| {
                r.get::<_, String>(0)
            })
            .map_err(|e| format!("query cache_hit_batch: {e}"))?
            .collect::<Result<_, _>>()
            .map_err(|e| format!("collect cache_hit_batch: {e}"))?;
        Ok(chunk.iter().map(|(h, ..)| hits.contains(h)).collect())
    }

    /// Single RW-mutex acquire per batch: insert all miss classifications via
    /// the Appender, then tick run progress. `processed_after` /
    /// `cache_hits_after` are the post-batch running totals, written verbatim
    /// to the runs row.
    fn flush_batch(
        &self,
        db: &AppDb,
        misses: &[Miss<'_>],
        classifications: &[crate::inference::Classification],
        processed_after: u64,
        cache_hits_after: u64,
    ) -> Result<(), String> {
        let conn = db.rw()?;

        if !classifications.is_empty() {
            let mut appender = conn
                .appender_with_columns(
                    "inference_results",
                    &[
                        "model_id",
                        "content_hash",
                        "classification",
                        "probability",
                        "logit_argmax",
                        "computed_at",
                        "computed_by_run",
                    ],
                )
                .map_err(|e| format!("open inference appender: {e}"))?;
            for (miss, classification) in misses.iter().zip(classifications.iter()) {
                // Codes persist in canonical zero-padded form (the model's
                // id2label strings are float-mangled); probability is the
                // softmax confidence, logit_argmax the raw research signal —
                // see docs/model-confidence.md.
                appender
                    .append_row(params![
                        self.model_id,
                        miss.content_hash,
                        crate::inference::normalize_ccm_code(
                            &classification.label,
                            self.digit_level
                        ),
                        f64::from(classification.probability),
                        f64::from(classification.logit_argmax),
                        self.computed_at.as_str(),
                        self.run_id.as_str(),
                    ])
                    .map_err(|e| format!("inference appender append_row: {e}"))?;
            }
            appender
                .flush()
                .map_err(|e| format!("inference appender flush: {e}"))?;
        }

        self.flush_progress(&conn, processed_after, cache_hits_after)
    }

    /// Single progress UPDATE on an already-held connection.
    fn flush_progress(
        &self,
        conn: &duckdb::Connection,
        processed: u64,
        cache_hits: u64,
    ) -> Result<(), String> {
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE runs SET rows_processed=?, cache_hits=?, last_progress_at=? WHERE id=?",
            params![
                i64::try_from(processed).unwrap_or(i64::MAX),
                i64::try_from(cache_hits).unwrap_or(i64::MAX),
                &now,
                &self.run_id,
            ],
        )
        .map_err(|e| format!("update progress: {e}"))?;
        Ok(())
    }
}

/// Running totals returned by [`RunPipeline::execute`]. `interrupted` is true
/// when the loop stopped early on a pause request rather than exhausting the
/// dataset.
#[derive(Debug)]
pub struct RunOutcome {
    pub processed: u64,
    pub unique_done: u64,
    pub cache_hits: u64,
    pub interrupted: bool,
}

/// Per-row state for a cache-missed input within one batch. Borrows the
/// content hash from the worker's owned `rows` Vec so `flush_batch` can pair it
/// with the corresponding classification without an extra clone.
struct Miss<'a> {
    content_hash: &'a str,
    input: String,
}
