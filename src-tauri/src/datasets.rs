//! Datasets read commands. Write commands (import flow, derived-dataset
//! creation) land alongside this module as Phase 7 work picks up; for now
//! this is just the listing endpoint that the Datasets activity tab consumes.

use serde::Serialize;
use specta::Type;
use tauri::State;

use crate::db::AppDb;

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
    /// `ready` when complete, `failed` when the worker errored.
    pub(crate) import_state: String,
    pub(crate) import_error: Option<String>,
}

#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri injects State by value; cannot be taken by reference at the macro layer"
)]
pub(crate) fn list_datasets(db: State<'_, AppDb>) -> Result<Vec<DatasetSummary>, String> {
    let conn = db.ro()?;
    let mut stmt = conn
        .prepare(
            "SELECT d.id,
                    d.title,
                    d.source_kind,
                    strftime(d.imported_at, '%Y-%m-%dT%H:%M:%SZ') AS imported_at,
                    COALESCE(d.row_count, 0)                      AS row_count,
                    COALESCE(d.import_state, 'ready')             AS import_state,
                    d.import_error
             FROM datasets d
             ORDER BY d.imported_at DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(DatasetSummary {
                id: row.get(0)?,
                title: row.get(1)?,
                source_kind: row.get(2)?,
                imported_at: row.get(3)?,
                row_count: row.get(4)?,
                import_state: row.get(5)?,
                import_error: row.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}
