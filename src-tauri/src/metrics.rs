//! Read-only aggregates for the Overview landing card grid. One IPC call
//! returns the whole set; the frontend invalidates on mutations that change a
//! row count (`import_csv`, `start_run`, dataset delete).

use serde::Serialize;
use specta::Type;
use tauri::State;

use crate::db::AppDb;

#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppMetrics {
    pub datasets: i64,
    pub courses: i64,
    pub runs: i64,
    pub completed_runs: i64,
    /// Distinct `(model_id, content_hash)` rows in `inference_results`.
    pub classifications: i64,
    /// Sum of `runs.cache_hits` divided by sum of `runs.rows_processed`, both
    /// across all runs. `None` when no rows have been processed yet (avoids a
    /// noisy 0% on a fresh DB).
    pub cache_hit_rate: Option<f64>,
}

#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri injects State by value; cannot be taken by reference at the macro layer"
)]
pub(crate) fn list_metrics(db: State<'_, AppDb>) -> Result<AppMetrics, String> {
    let conn = db.ro()?;

    let datasets: i64 = conn
        .query_row("SELECT COUNT(*) FROM datasets", [], |row| row.get(0))
        .map_err(|e| format!("count datasets: {e}"))?;
    let courses: i64 = conn
        .query_row("SELECT COUNT(*) FROM courses", [], |row| row.get(0))
        .map_err(|e| format!("count courses: {e}"))?;
    let runs: i64 = conn
        .query_row("SELECT COUNT(*) FROM runs", [], |row| row.get(0))
        .map_err(|e| format!("count runs: {e}"))?;
    let completed_runs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM runs WHERE state = 'completed'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("count completed runs: {e}"))?;
    let classifications: i64 = conn
        .query_row("SELECT COUNT(*) FROM inference_results", [], |row| {
            row.get(0)
        })
        .map_err(|e| format!("count inference_results: {e}"))?;

    // Cache hit rate is sum-over-sum (not avg-of-rates) so a 10-row run and a
    // 1000-row run weigh proportionally, matching how the user thinks about
    // "what fraction of work was cached".
    let (processed_sum, hits_sum): (Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT
                CAST(SUM(COALESCE(rows_processed, 0)) AS BIGINT),
                CAST(SUM(COALESCE(cache_hits, 0))    AS BIGINT)
             FROM runs",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| format!("aggregate cache rate: {e}"))?;

    // 2^52 row counts are not a concern here; the precision-loss lint flags
    // the i64-to-f64 cast but the integers we ratio are vanishingly small
    // relative to the f64 mantissa.
    #[expect(clippy::cast_precision_loss, reason = "row counts won't approach 2^52")]
    let cache_hit_rate = match (processed_sum, hits_sum) {
        (Some(p), Some(h)) if p > 0 => Some(h as f64 / p as f64),
        _ => None,
    };

    Ok(AppMetrics {
        datasets,
        courses,
        runs,
        completed_runs,
        classifications,
        cache_hit_rate,
    })
}
