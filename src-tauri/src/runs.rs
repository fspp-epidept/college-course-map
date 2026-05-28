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
