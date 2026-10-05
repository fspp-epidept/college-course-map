//! Read-only aggregates for the Overview landing card grid. One IPC call
//! returns the whole set; the frontend invalidates on mutations that change a
//! row count (`import_csv`, a classification finishing, dataset delete).

use serde::Serialize;
use specta::Type;
use tauri::State;

use crate::boot::Boot;

#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppMetrics {
    pub datasets: i64,
    pub courses: i64,
    /// Distinct `(model_id, content_hash)` rows in `inference_results`.
    pub classifications: i64,
}

#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri injects State by value; cannot be taken by reference at the macro layer"
)]
pub(crate) fn list_metrics(boot: State<'_, Boot>) -> Result<AppMetrics, String> {
    let conn = boot.ready()?.db.ro()?;

    let datasets: i64 = conn
        .query_row("SELECT COUNT(*) FROM datasets", [], |row| row.get(0))
        .map_err(|e| format!("count datasets: {e}"))?;
    let courses: i64 = conn
        .query_row("SELECT COUNT(*) FROM courses", [], |row| row.get(0))
        .map_err(|e| format!("count courses: {e}"))?;
    let classifications: i64 = conn
        .query_row("SELECT COUNT(*) FROM inference_results", [], |row| {
            row.get(0)
        })
        .map_err(|e| format!("count inference_results: {e}"))?;

    Ok(AppMetrics {
        datasets,
        courses,
        classifications,
    })
}
