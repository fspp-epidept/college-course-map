//! Async run pipeline. `start_run` inserts a runs row in `running` state,
//! returns the run id immediately, and offloads the batched inference loop to
//! a blocking task that ticks `runs.rows_processed` per flushed super-chunk
//! (`FLUSH_SIZE`, EPI-89) so the frontend can poll progress via `get_run`
//! (see `useRun`).
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

/// Compact the WAL this often during a run (EPI-92), at a flushed super-chunk
/// boundary. 60 s of GPU-rate results is a few tens of MB of WAL — compaction
/// costs tens of ms (<0.1% of throughput); without it the WAL grows until app
/// exit and crash-recovery time scales with it.
const CHECKPOINT_INTERVAL: std::time::Duration = std::time::Duration::from_mins(1);

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
/// as `interrupted` AND every recorded model is still the manifest-active row
/// for its digit level AND all of them are actually loaded (EPI-96). The
/// model-family check matters: resuming an old-family run with the currently
/// loaded model would write new-model outputs under the old model's cache
/// key — silent data corruption. Blockers are machine-stable keys the
/// frontend translates.
fn assess_resumability(
    state: &str,
    model_ids: &[i64],
    catalog: &ModelCatalog,
    store: &ModelStore,
) -> (bool, Vec<String>) {
    if state != "interrupted" {
        return (false, Vec::new());
    }
    let mut blockers = Vec::new();
    if model_ids.is_empty() {
        blockers.push("model_superseded".to_owned());
    }
    for model_id in model_ids {
        match catalog.level_of(*model_id) {
            Some(level) => {
                let loaded = store
                    .get()
                    .is_some_and(|registry| registry.by_digit_level(level).is_some());
                if !loaded {
                    // Distinguish "the startup autoload is still churning"
                    // from "no models on this machine": the first resolves
                    // itself in moments and needs no user action, the second
                    // needs the Models panel. Collapsing them told users to
                    // go download models that were already loading.
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
            None => blockers.push("model_superseded".to_owned()),
        }
    }
    blockers.sort_unstable();
    blockers.dedup();
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
                    r.resume_count, m.id, m.model_type, r.model_ids
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
            let _model_id: Option<i64> = row.get(13)?;
            let model_type: Option<String> = row.get(14)?;
            let model_ids_json: String = row.get(15)?;
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
                model_type,
                model_ids_json,
            ))
        })
        .map_err(|e| e.to_string())?;
    rows.map(|item| {
        let (mut summary, model_type, model_ids_json) = item.map_err(|e| e.to_string())?;
        let model_ids: Vec<i64> = serde_json::from_str(&model_ids_json).unwrap_or_default();
        // A single-model run displays its level; multi-model runs (EPI-96)
        // show no level — the UI labels them "all models".
        summary.digit_level = if model_ids.len() == 1 {
            model_type.and_then(|t| t.parse::<u8>().ok())
        } else {
            None
        };
        let (resumable, blockers) =
            assess_resumability(&summary.state, &model_ids, &catalog, &store);
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
) -> Result<Option<(RunDetail, Vec<i64>)>, String> {
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

    // Single-model runs display their level; multi-model runs (EPI-96) show
    // no level and the UI labels them "all models".
    let model_ids: Vec<i64> = serde_json::from_str(&row.model_ids_json).unwrap_or_default();
    let digit_level = if model_ids.len() == 1 {
        resolve_first_model_level(conn, &row.model_ids_json)
    } else {
        None
    };

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
        model_ids,
    )))
}

/// Stamp read-time resumability onto a mapped detail row.
fn finish_run_detail(
    (mut detail, model_ids): (RunDetail, Vec<i64>),
    catalog: &ModelCatalog,
    store: &ModelStore,
) -> RunDetail {
    let (resumable, blockers) = assess_resumability(&detail.state, &model_ids, catalog, store);
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

/// Digit level of the run's first `model_ids` entry, via the models table
/// (works for superseded rows too — display only, never used for execution).
fn resolve_first_model_level(conn: &duckdb::Connection, model_ids_json: &str) -> Option<u8> {
    let ids: Vec<i64> = serde_json::from_str(model_ids_json).ok()?;
    let first = ids.first()?;
    let model_type: String = conn
        .query_row(
            "SELECT model_type FROM models WHERE id = ?",
            params![first],
            |row| row.get(0),
        )
        .ok()?;
    model_type.parse::<u8>().ok()
}

#[derive(Type, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartRunRequest {
    /// A run always classifies the dataset with every manifest model
    /// (EPI-96) — there is no level to pick.
    pub dataset_id: String,
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
    // A run covers every manifest level (EPI-96) — all of them must be
    // loaded before any inference starts, so a run can never half-cover the
    // dataset because one model was still downloading.
    let mut run_models = Vec::new();
    for digit_level in catalog.levels() {
        if registry.by_digit_level(digit_level).is_none() {
            return Err(format!("no model loaded for digit_level={digit_level}"));
        }
        let model_id = catalog
            .model_id(digit_level)
            .ok_or_else(|| format!("manifest has no model for digit_level {digit_level}"))?;
        run_models.push(RunModel {
            model_id,
            digit_level,
        });
    }
    if run_models.is_empty() {
        return Err("manifest defines no models".to_owned());
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

    // A run always covers the whole dataset (EPI-66) with every model
    // (EPI-96); the progress denominator is rows × models.
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
    let rows_total = course_count.saturating_mul(i64::try_from(run_models.len()).unwrap_or(1));

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
            "All models",
            serde_json::to_string(&run_models.iter().map(|m| m.model_id).collect::<Vec<_>>())
                .map_err(|e| e.to_string())?,
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
            models: run_models,
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
    // the enforcement point, the flags are the preview. Every recorded model
    // must still be the manifest-active row for its level and be loaded.
    let model_ids: Vec<i64> =
        serde_json::from_str(&model_ids_json).map_err(|e| format!("parse model_ids: {e}"))?;
    if model_ids.is_empty() {
        return Err("run has no resolvable model".to_owned());
    }
    let Some(registry) = store.get() else {
        return Err(models_unready_message(&store));
    };
    let mut run_models = Vec::with_capacity(model_ids.len());
    for model_id in model_ids {
        let Some(digit_level) = catalog.level_of(model_id) else {
            return Err(
                "this run's model is no longer the app-active model — start a new run instead \
                 (its finished classifications stay in the cache)"
                    .to_owned(),
            );
        };
        if registry.by_digit_level(digit_level).is_none() {
            return Err(models_unready_message(&store));
        }
        run_models.push(RunModel {
            model_id,
            digit_level,
        });
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
            models: run_models,
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
        let outcome =
            (|| {
                // Clone the registry Arc out of the store once — the worker keeps
                // this snapshot for the whole run even if the store changes later.
                let registry = self
                    .app
                    .state::<ModelStore>()
                    .get()
                    .ok_or_else(|| "models not loaded (worker)".to_owned())?;
                let mut loaded = Vec::with_capacity(self.pipeline.models.len());
                for run_model in &self.pipeline.models {
                    loaded.push(registry.by_digit_level(run_model.digit_level).ok_or_else(
                        || {
                            format!(
                                "no model loaded for digit_level={} (worker)",
                                run_model.digit_level
                            )
                        },
                    )?);
                }
                self.pipeline.execute(&db, &loaded)
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
/// One model a run classifies with, resolved at start (EPI-96).
#[derive(Debug, Clone, Copy)]
pub struct RunModel {
    pub model_id: i64,
    pub digit_level: u8,
}

#[derive(Debug)]
pub struct RunPipeline {
    pub dataset_id: String,
    pub run_id: String,
    /// Models this run covers, in classification order (EPI-96). Each level
    /// runs to completion before the next starts, so a resumed run skips
    /// finished levels by construction — their anti-join selects nothing.
    pub models: Vec<RunModel>,
    /// Stamped as `computed_at` on every inference row this leg writes.
    pub computed_at: String,
    /// Set true by [`pause_run`]; polled at each batch boundary to stop early.
    pub cancel: Arc<AtomicBool>,
}

/// Progress counters written by every flush, kept together so the flush path
/// takes one coherent snapshot instead of a parameter list.
struct ProgressSnapshot {
    processed: u64,
    cache_hits: u64,
    unique_done: u64,
    unique_total: u64,
}

/// Per-level missing-work stats, measured once at leg start (EPI-91): rows
/// referencing a not-yet-cached hash, and the distinct hashes themselves.
/// Row-level progress within a level is interpolated from these (exact at
/// level boundaries) instead of walked row by row.
#[derive(Clone, Copy)]
struct LevelPlan {
    missing_rows: u64,
    missing_unique: u64,
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
                        i64::try_from(o.unique_total).unwrap_or(i64::MAX),
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
                        i64::try_from(o.unique_total).unwrap_or(i64::MAX),
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

    /// The batched inference loop (EPI-91/96). Per model, the *distinct*
    /// missing inputs are materialized once (anti-join + GROUP BY hash into a
    /// temp table) and consumed in content-hash keyset windows of
    /// `FLUSH_SIZE`, so the Rust heap never holds more than one window and no
    /// duplicate input is ever walked, formatted, or re-checked against the
    /// cache. Row-level progress is interpolated from per-level stats
    /// measured at leg start — exact at level boundaries — instead of walked.
    /// Resume continues where it stopped by construction: finished levels
    /// (and finished hashes within a level) drop out of the anti-join.
    ///
    /// `loaded` must align with `self.models`, one loaded model per entry.
    pub fn execute(&self, db: &AppDb, loaded: &[&LoadedModel]) -> Result<RunOutcome, String> {
        if loaded.len() != self.models.len() {
            return Err("loaded models do not align with the run's models".to_owned());
        }
        let course_count = self.course_count(db)?;
        let mut plans = Vec::with_capacity(self.models.len());
        for run_model in &self.models {
            plans.push(self.level_stats(db, run_model.model_id)?);
        }
        // Unique counters accumulate across resume legs (EPI-90): the
        // persisted `unique_inputs_done` is by definition what this run
        // computed in earlier legs (every flush writes it). Row counters are
        // derived: covered rows per level + the classified fraction of its
        // missing rows; `cache_hits` keeps the historical identity
        // `processed = unique_done + cache_hits`.
        let mut unique_done = self.prior_unique_done(db)?;
        let unique_total = unique_done + plans.iter().map(|p| p.missing_unique).sum::<u64>();
        let mut level_done: Vec<u64> = vec![0; self.models.len()];
        let processed_rows = |level_done: &[u64]| -> u64 {
            plans
                .iter()
                .zip(level_done)
                .map(|(plan, done)| {
                    let covered = course_count.saturating_sub(plan.missing_rows);
                    let advanced = (plan.missing_rows * done)
                        .checked_div(plan.missing_unique)
                        .unwrap_or(0);
                    covered + advanced
                })
                .sum()
        };
        let snapshot = |level_done: &[u64], unique_done: u64| -> ProgressSnapshot {
            let processed = processed_rows(level_done);
            ProgressSnapshot {
                processed,
                cache_hits: processed.saturating_sub(unique_done),
                unique_done,
                unique_total,
            }
        };

        // Surface the starting position immediately — before the first batch
        // computes — so a resumed run's progress bar never reads zero.
        {
            let conn = db.rw()?;
            self.flush_progress(&conn, &snapshot(&level_done, unique_done))?;
        }

        let mut last_checkpoint = std::time::Instant::now();
        for (index, (run_model, model)) in self.models.iter().zip(loaded).enumerate() {
            let Some(plan) = plans.get(index).copied() else {
                continue;
            };
            if plan.missing_unique == 0 {
                continue;
            }
            let batch = crate::inference::batch_size(model.resolved_ep);
            self.materialize_misses(db, run_model.model_id)?;
            let mut cursor = String::new();
            loop {
                // One window = one flush unit (EPI-89): a single DB
                // round-trip for the read and one for the write per
                // FLUSH_SIZE distinct inputs, while the ONNX calls inside run
                // at the per-EP batch size (EPI-82).
                let window = next_miss_window(db, &cursor)?;
                let Some(last) = window.last() else {
                    break;
                };
                let next_cursor = last.0.clone();
                let (misses, classifications, cancelled) =
                    self.classify_window(model, batch, &window)?;

                // Flush whatever classified — on pause that's a prefix of the
                // window (the misses/classifications zip truncates to it);
                // the skipped remainder stays missing and the next leg's
                // anti-join picks it up.
                unique_done += classifications.len() as u64;
                if let Some(done) = level_done.get_mut(index) {
                    *done += classifications.len() as u64;
                }
                let progress = snapshot(&level_done, unique_done);
                self.flush_batch(db, run_model.model_id, &misses, &classifications, &progress)?;
                if cancelled {
                    return Ok(RunOutcome {
                        processed: progress.processed,
                        unique_done,
                        unique_total,
                        cache_hits: progress.cache_hits,
                        interrupted: true,
                    });
                }

                // Periodic WAL compaction (EPI-92), best-effort: a plain
                // CHECKPOINT errors harmlessly if a concurrent read
                // transaction is open — the next boundary retries.
                if last_checkpoint.elapsed() >= CHECKPOINT_INTERVAL {
                    let result = db.rw().and_then(|conn| {
                        conn.execute_batch("CHECKPOINT").map_err(|e| e.to_string())
                    });
                    if let Err(e) = result {
                        eprintln!("run {}: periodic checkpoint skipped: {e}", self.run_id);
                    }
                    last_checkpoint = std::time::Instant::now();
                }

                cursor = next_cursor;
            }
            self.drop_misses(db);
        }
        let progress = snapshot(&level_done, unique_done);
        Ok(RunOutcome {
            processed: progress.processed,
            unique_done,
            unique_total,
            cache_hits: progress.cache_hits,
            interrupted: false,
        })
    }

    /// Format, length-bucket, and classify one window of distinct missing
    /// inputs. The window is distinct by construction — no cache check, no
    /// dedupe set. Length-bucketing (EPI-82): sorted inputs make each ONNX
    /// sub-batch near-uniform so `BatchLongest` padding does almost no wasted
    /// work (+14% CUDA, +16% CPU measured); order is semantically free
    /// because flush pairs by index and cache keys are content hashes. The
    /// pause flag is honored between sub-batches, so pause latency stays one
    /// ONNX call; on pause the returned classifications are a prefix of the
    /// returned misses.
    fn classify_window<'w>(
        &self,
        model: &LoadedModel,
        batch: usize,
        window: &'w [SelectedCourse],
    ) -> Result<(Vec<Miss<'w>>, Vec<crate::inference::Classification>, bool), String> {
        let mut misses: Vec<Miss<'w>> = window
            .iter()
            .map(|(content_hash, subject, catalog, title)| Miss {
                content_hash: content_hash.as_str(),
                input: format_input(&CourseInput {
                    subject_code: subject.clone(),
                    catalog_number: catalog.clone(),
                    course_title: title.clone(),
                }),
            })
            .collect();
        misses.sort_by_key(|m| m.input.len());
        let miss_refs: Vec<&str> = misses.iter().map(|m| m.input.as_str()).collect();

        let mut classifications = Vec::with_capacity(misses.len());
        let mut cancelled = false;
        for sub in miss_refs.chunks(batch) {
            let batch_results =
                classify_batch(model, sub).map_err(|e| format!("classify_batch: {e}"))?;
            classifications.extend(batch_results);
            // Relaxed is fine — we only need eventual visibility of the flag,
            // not ordering against other memory.
            if self.cancel.load(Ordering::Relaxed) {
                cancelled = true;
                break;
            }
        }
        Ok((misses, classifications, cancelled))
    }

    fn course_count(&self, db: &AppDb) -> Result<u64, String> {
        let conn = db.ro()?;
        let total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM courses WHERE dataset_id = ?",
                params![&self.dataset_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("count courses: {e}"))?;
        Ok(u64::try_from(total).unwrap_or(0))
    }

    /// Missing-work stats for one model, measured once at leg start (EPI-91).
    fn level_stats(&self, db: &AppDb, model_id: i64) -> Result<LevelPlan, String> {
        let conn = db.ro()?;
        let (rows, unique): (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), COUNT(DISTINCT c.content_hash)
                 FROM courses c
                 WHERE c.dataset_id = ?
                   AND NOT EXISTS (
                       SELECT 1 FROM inference_results ir
                       WHERE ir.model_id = ? AND ir.content_hash = c.content_hash
                   )",
                params![&self.dataset_id, model_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| format!("level stats: {e}"))?;
        Ok(LevelPlan {
            missing_rows: u64::try_from(rows).unwrap_or(0),
            missing_unique: u64::try_from(unique).unwrap_or(0),
        })
    }

    /// Materialize one level's distinct missing inputs into a temp table on
    /// the RW connection (EPI-91/67) — one anti-join pass, then windowed
    /// reads keep the Rust heap bounded by `FLUSH_SIZE`. `arg_min` takes all
    /// representative fields from the same (lowest `row_index`) row, so the
    /// formatted input is exactly what that row would produce.
    fn materialize_misses(&self, db: &AppDb, model_id: i64) -> Result<(), String> {
        let conn = db.rw()?;
        conn.execute(
            "CREATE OR REPLACE TEMP TABLE run_misses AS
             SELECT c.content_hash,
                    arg_min(c.subject_code, c.row_index) AS subject_code,
                    arg_min(c.catalog_number, c.row_index) AS catalog_number,
                    arg_min(c.course_title, c.row_index) AS course_title
             FROM courses c
             WHERE c.dataset_id = ?
               AND NOT EXISTS (
                   SELECT 1 FROM inference_results ir
                   WHERE ir.model_id = ? AND ir.content_hash = c.content_hash
               )
             GROUP BY c.content_hash",
            params![&self.dataset_id, model_id],
        )
        .map_err(|e| format!("materialize misses: {e}"))?;
        Ok(())
    }

    /// Best-effort cleanup of the level's temp table; the next
    /// `materialize_misses` replaces it anyway.
    fn drop_misses(&self, db: &AppDb) {
        if let Ok(conn) = db.rw()
            && let Err(e) = conn.execute_batch("DROP TABLE IF EXISTS run_misses")
        {
            eprintln!("run {}: drop misses temp table: {e}", self.run_id);
        }
    }

    /// The run row's persisted `unique_inputs_done` — the resume-safe count
    /// of rows this run computed in earlier legs (EPI-90).
    fn prior_unique_done(&self, db: &AppDb) -> Result<u64, String> {
        let conn = db.ro()?;
        let done: i64 = conn
            .query_row(
                "SELECT unique_inputs_done FROM runs WHERE id = ?",
                params![&self.run_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("read unique_inputs_done: {e}"))?;
        Ok(u64::try_from(done).unwrap_or(0))
    }

    /// Single RW-mutex acquire per window: insert the window's
    /// classifications for `model_id` via the Appender, then tick run
    /// progress. The `misses`/`classifications` zip truncates to the
    /// classified prefix, which is exactly right for a pause landing mid-
    /// window.
    fn flush_batch(
        &self,
        db: &AppDb,
        model_id: i64,
        misses: &[Miss<'_>],
        classifications: &[crate::inference::Classification],
        progress: &ProgressSnapshot,
    ) -> Result<(), String> {
        let conn = db.rw()?;

        let digit_level = self
            .models
            .iter()
            .find(|m| m.model_id == model_id)
            .map_or(0, |m| m.digit_level);
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
                        model_id,
                        miss.content_hash,
                        crate::inference::normalize_ccm_code(&classification.label, digit_level),
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

        self.flush_progress(&conn, progress)
    }

    /// Single progress UPDATE on an already-held connection. Writes the whole
    /// snapshot — including `unique_inputs_done` (EPI-90) and
    /// `unique_inputs_total` (EPI-91) — in the same transaction as the cache
    /// inserts, so every flush persists a resume-safe, crash-consistent
    /// state.
    fn flush_progress(
        &self,
        conn: &duckdb::Connection,
        progress: &ProgressSnapshot,
    ) -> Result<(), String> {
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE runs SET rows_processed=?, cache_hits=?, unique_inputs_done=?,
                unique_inputs_total=?, last_progress_at=? WHERE id=?",
            params![
                i64::try_from(progress.processed).unwrap_or(i64::MAX),
                i64::try_from(progress.cache_hits).unwrap_or(i64::MAX),
                i64::try_from(progress.unique_done).unwrap_or(i64::MAX),
                i64::try_from(progress.unique_total).unwrap_or(i64::MAX),
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
    /// Total distinct inputs this run must compute across all its models:
    /// prior legs' done + what was missing at this leg's start (EPI-91).
    pub unique_total: u64,
    pub cache_hits: u64,
    pub interrupted: bool,
}

/// Per-row state for a cache-missed input within one batch. Borrows the
/// content hash from the worker's owned window Vec so `flush_batch` can pair
/// it with the corresponding classification without an extra clone.
struct Miss<'a> {
    content_hash: &'a str,
    input: String,
}

/// Next `FLUSH_SIZE` distinct missing inputs past `cursor`. Reads on the RW
/// connection — temp tables are per-connection, and the RO handle is a
/// different connection.
fn next_miss_window(db: &AppDb, cursor: &str) -> Result<Vec<SelectedCourse>, String> {
    let conn = db.rw()?;
    let mut stmt = conn
        .prepare(
            "SELECT content_hash, subject_code, catalog_number, course_title
             FROM run_misses WHERE content_hash > ? ORDER BY content_hash LIMIT ?",
        )
        .map_err(|e| format!("prepare miss window: {e}"))?;
    stmt.query_map(
        params![cursor, i64::try_from(FLUSH_SIZE).unwrap_or(i64::MAX)],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )
    .map_err(|e| format!("query miss window: {e}"))?
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| format!("collect miss window: {e}"))
}
