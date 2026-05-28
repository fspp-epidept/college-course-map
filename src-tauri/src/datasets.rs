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
    /// `COUNT(*)` from `courses` for this dataset — recomputed each call so the
    /// `datasets.row_count` cached column staying in sync isn't load-bearing.
    pub(crate) row_count: i64,
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
                    COALESCE(COUNT(c.id), 0)                      AS row_count
             FROM datasets d
             LEFT JOIN courses c ON c.dataset_id = d.id
             GROUP BY d.id, d.title, d.source_kind, d.imported_at
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
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}
