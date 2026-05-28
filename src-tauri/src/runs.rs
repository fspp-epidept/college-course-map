//! Synchronous run pipeline. The spike-grade IPC entry point classifies every
//! course in a dataset against one model, exercising the `(model_id,
//! content_hash)` cache so a re-run on the same dataset is mostly free.
//!
//! Deliberately simple: no progress events, no cancellation, no batching.
//! The real run engine (#37–#47) replaces this with a background task that
//! streams Tauri events; this command is here so the spike demo has a single
//! synchronous "Classify" button that produces real CCM codes end-to-end.

use std::time::Instant;

use chrono::Utc;
use duckdb::params;
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;
use uuid::Uuid;

use crate::{
    db::AppDb,
    format::{CourseInput, format_input},
    inference::{self, InferenceRegistry, classify},
};

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
    pub rows_total: Option<i64>,
    pub rows_processed: Option<i64>,
    pub cache_hits: Option<i64>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub last_progress_at: Option<String>,
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
}

#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri injects State by value; cannot be taken by reference at the macro layer"
)]
pub(crate) fn list_runs(db: State<'_, AppDb>) -> Result<Vec<RunSummary>, String> {
    let conn = db.ro()?;
    // Active states float to the top, then most-recent first within each
    // ordering bucket. The frontend further regroups by state but the
    // ordering inside each group should be useful as-is.
    let mut stmt = conn
        .prepare(
            "SELECT r.id, r.dataset_id, d.title, r.description, r.state,
                    r.rows_total, r.rows_processed, r.cache_hits,
                    strftime(r.created_at, '%Y-%m-%dT%H:%M:%SZ'),
                    strftime(r.started_at, '%Y-%m-%dT%H:%M:%SZ'),
                    strftime(r.completed_at, '%Y-%m-%dT%H:%M:%SZ'),
                    strftime(r.last_progress_at, '%Y-%m-%dT%H:%M:%SZ')
             FROM runs r
             JOIN datasets d ON d.id = r.dataset_id
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
            Ok(RunSummary {
                id: row.get(0)?,
                dataset_id: row.get(1)?,
                dataset_title: row.get(2)?,
                description: row.get(3)?,
                state: row.get(4)?,
                rows_total: row.get(5)?,
                rows_processed: row.get(6)?,
                cache_hits: row.get(7)?,
                created_at: row.get(8)?,
                started_at: row.get(9)?,
                completed_at: row.get(10)?,
                last_progress_at: row.get(11)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
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
}

#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn get_run(id: String, db: State<'_, AppDb>) -> Result<RunDetail, String> {
    let conn = db.ro()?;
    let row = conn
        .query_row(
            "SELECT r.id, r.dataset_id, d.title, r.description, r.state, r.model_ids,
                    r.rows_total, r.rows_processed, r.unique_inputs_done, r.cache_hits,
                    strftime(r.created_at, '%Y-%m-%dT%H:%M:%SZ'),
                    strftime(r.started_at, '%Y-%m-%dT%H:%M:%SZ'),
                    strftime(r.completed_at, '%Y-%m-%dT%H:%M:%SZ'),
                    strftime(r.last_progress_at, '%Y-%m-%dT%H:%M:%SZ'),
                    r.error_message, r.execution_provider
             FROM runs r
             JOIN datasets d ON d.id = r.dataset_id
             WHERE r.id = ?",
            params![id],
            |row| {
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
                })
            },
        )
        .map_err(|e| format!("run {id}: {e}"))?;

    // Resolve the digit level from the first id in `model_ids` (JSON array).
    // Runs always carry exactly one model id during the spike, so taking
    // first() is fine until multi-model runs land.
    let digit_level = resolve_digit_level(&conn, &row.model_ids_json).unwrap_or(None);

    Ok(RunDetail {
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
    })
}

fn resolve_digit_level(
    conn: &duckdb::Connection,
    model_ids_json: &str,
) -> Result<Option<u8>, String> {
    let ids: Vec<i64> =
        serde_json::from_str(model_ids_json).map_err(|e| format!("parse model_ids: {e}"))?;
    let Some(first) = ids.first() else {
        return Ok(None);
    };
    let model_type: Option<String> = conn
        .query_row(
            "SELECT model_type FROM models WHERE id = ?",
            params![first],
            |row| row.get(0),
        )
        .ok();
    Ok(model_type.and_then(|s| s.parse::<u8>().ok()))
}

/// 50 keeps the synchronous loop under ~10 s on CPU; the dialog default. The
/// caller may override.
const DEFAULT_LIMIT: u32 = 50;

#[derive(Type, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartRunRequest {
    pub dataset_id: String,
    /// 2, 4, or 6. Maps to a row in the `models` table on the Rust side; the
    /// spike avoids forcing the frontend to know surrogate model ids.
    pub digit_level: u8,
    pub limit: Option<u32>,
}

#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunResult {
    pub run_id: String,
    pub rows_processed: u64,
    pub unique_inputs_done: u64,
    pub cache_hits: u64,
    pub duration_ms: u64,
}

#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn start_run(
    req: StartRunRequest,
    db: State<'_, AppDb>,
    registry: State<'_, InferenceRegistry>,
) -> Result<RunResult, String> {
    let limit = req.limit.unwrap_or(DEFAULT_LIMIT);
    let model = registry
        .by_digit_level(req.digit_level)
        .ok_or_else(|| format!("no model loaded for digit_level={}", req.digit_level))?;

    let conn = db.rw()?;

    // Resolve which models row we'll attribute results to. The seed inserts
    // one row per digit level with `model_type` "2"/"4"/"6"; pick the first
    // (deterministic ORDER BY id) so re-runs accrete to the same row.
    let model_id: i64 = conn
        .query_row(
            "SELECT id FROM models WHERE model_type = ? ORDER BY id LIMIT 1",
            params![req.digit_level.to_string()],
            |row| row.get(0),
        )
        .map_err(|e| format!("lookup model row for digit_level {}: {e}", req.digit_level))?;

    let run_id = Uuid::new_v4().to_string();
    let started = Instant::now();
    let now = Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO runs
            (id, dataset_id, description, state, model_ids,
             rows_total, rows_processed,
             unique_inputs_total, unique_inputs_done, cache_hits,
             created_at, started_at, execution_provider)
         VALUES (?, ?, ?, 'running', ?,
                 NULL, 0,
                 NULL, 0, 0,
                 ?, ?, 'cpu')",
        params![
            run_id,
            req.dataset_id,
            format!("Spike run: {}-digit", req.digit_level),
            serde_json::to_string(&[model_id]).map_err(|e| e.to_string())?,
            now,
            now,
        ],
    )
    .map_err(|e| format!("insert runs: {e}"))?;

    let outcome = run_inner(
        &conn,
        &req.dataset_id,
        limit,
        model_id,
        &run_id,
        model,
        &now,
    );

    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let completed_at = Utc::now().to_rfc3339();

    match outcome {
        Ok((processed, unique_done, cache_hits)) => {
            conn.execute(
                "UPDATE runs SET state='completed',
                    rows_total=?, rows_processed=?,
                    unique_inputs_total=?, unique_inputs_done=?, cache_hits=?,
                    completed_at=?, last_progress_at=?
                 WHERE id=?",
                params![
                    i64::try_from(processed).unwrap_or(i64::MAX),
                    i64::try_from(processed).unwrap_or(i64::MAX),
                    i64::try_from(unique_done + cache_hits).unwrap_or(i64::MAX),
                    i64::try_from(unique_done).unwrap_or(i64::MAX),
                    i64::try_from(cache_hits).unwrap_or(i64::MAX),
                    completed_at,
                    completed_at,
                    run_id,
                ],
            )
            .map_err(|e| format!("finalize runs: {e}"))?;

            Ok(RunResult {
                run_id,
                rows_processed: processed,
                unique_inputs_done: unique_done,
                cache_hits,
                duration_ms,
            })
        }
        Err(err) => {
            let _ = conn.execute(
                "UPDATE runs SET state='failed', error_message=?, completed_at=? WHERE id=?",
                params![&err, completed_at, run_id],
            );
            Err(err)
        }
    }
}

/// Inner loop: select courses, classify, write results. Returns
/// `(rows_processed, unique_inputs_done, cache_hits)`.
fn run_inner(
    conn: &duckdb::Connection,
    dataset_id: &str,
    limit: u32,
    model_id: i64,
    run_id: &str,
    model: &inference::LoadedModel,
    now: &str,
) -> Result<(u64, u64, u64), String> {
    let mut select = conn
        .prepare(
            "SELECT content_hash, subject_code, catalog_number, course_title
             FROM courses
             WHERE dataset_id = ?
             ORDER BY row_index
             LIMIT ?",
        )
        .map_err(|e| format!("prepare select courses: {e}"))?;
    let rows: Vec<(String, String, String, String)> = select
        .query_map(params![dataset_id, i64::from(limit)], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .map_err(|e| format!("query courses: {e}"))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("collect courses: {e}"))?;

    let mut processed = 0_u64;
    let mut unique_done = 0_u64;
    let mut cache_hits = 0_u64;

    let mut hit_check = conn
        .prepare(
            "SELECT 1 FROM inference_results
             WHERE model_id = ? AND content_hash = ? LIMIT 1",
        )
        .map_err(|e| format!("prepare cache check: {e}"))?;
    let mut insert_result = conn
        .prepare(
            "INSERT INTO inference_results
                (model_id, content_hash, classification, probability,
                 computed_at, computed_by_run)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .map_err(|e| format!("prepare insert: {e}"))?;

    for (content_hash, subject_code, catalog_number, course_title) in rows {
        processed += 1;

        let is_hit: Option<i32> = hit_check
            .query_row(params![model_id, &content_hash], |row| row.get(0))
            .ok();
        if is_hit.is_some() {
            cache_hits += 1;
            continue;
        }

        let input = format_input(&CourseInput {
            subject_code,
            catalog_number,
            course_title,
        });
        let classification =
            classify(model, &input).map_err(|e| format!("classify {content_hash}: {e}"))?;

        insert_result
            .execute(params![
                model_id,
                &content_hash,
                &classification.label,
                f64::from(classification.logit_argmax),
                now,
                run_id,
            ])
            .map_err(|e| format!("insert inference_results: {e}"))?;
        unique_done += 1;
    }

    Ok((processed, unique_done, cache_hits))
}
