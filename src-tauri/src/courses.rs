//! Paginated courses listing for the dataset detail view. Each row is joined
//! left against `inference_results` for an optional model so the same query
//! powers both the unclassified preview and the post-run results browser.

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;

use crate::db::AppDb;

/// Hard cap on `limit`. Without this a malicious / buggy caller could ask for
/// the whole dataset; bounding here keeps a single IPC response cheap.
const MAX_PAGE_SIZE: u32 = 500;

#[derive(Type, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListCoursesRequest {
    pub dataset_id: String,
    /// Optional model id for the joined classification + probability columns.
    /// `None` means the joined columns come back as `null`.
    pub model_id: Option<i64>,
    pub offset: u32,
    pub limit: u32,
}

#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CourseRow {
    pub id: i64,
    pub row_index: i64,
    pub subject_code: Option<String>,
    pub catalog_number: Option<String>,
    pub course_title: Option<String>,
    pub content_hash: String,
    /// Classification label from `inference_results` for the requested model;
    /// `None` when there's no result yet (or no `model_id` was requested).
    pub classification: Option<String>,
    pub probability: Option<f64>,
}

#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CoursePage {
    pub rows: Vec<CourseRow>,
    pub total: i64,
}

#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn list_courses_with_results(
    req: ListCoursesRequest,
    db: State<'_, AppDb>,
) -> Result<CoursePage, String> {
    let limit = req.limit.clamp(1, MAX_PAGE_SIZE);
    let conn = db.ro()?;

    // Use the cached `datasets.row_count` rather than a `COUNT(*)` against
    // the courses table — for a multi-million-row dataset the scan dominates
    // page-load time and the cached value is authoritative for file-source
    // datasets (set by `mark_ready` after the Appender finishes). Falls back
    // to a real count when the cached value is NULL.
    let cached: Option<i64> = conn
        .query_row(
            "SELECT row_count FROM datasets WHERE id = ?",
            [&req.dataset_id],
            |row| row.get(0),
        )
        .ok();
    let total: i64 = match cached {
        Some(n) if n >= 0 => n,
        _ => conn
            .query_row(
                "SELECT COUNT(*) FROM courses WHERE dataset_id = ?",
                [&req.dataset_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("count courses: {e}"))?,
    };

    // Step 1: page the courses table on its own. The (dataset_id, row_index)
    // index supports the filter + ordering; the LIMIT keeps the work bounded
    // to PAGE_SIZE rows regardless of dataset size. Doing the JOIN here
    // instead used to push DuckDB into a 2M-row scan + hash-join plan on
    // freshly imported data, which is what was hanging the tab.
    let mut stmt = conn
        .prepare(
            "SELECT id, row_index, subject_code, catalog_number,
                    course_title, content_hash
             FROM courses
             WHERE dataset_id = ?
             ORDER BY row_index
             LIMIT ? OFFSET ?",
        )
        .map_err(|e| format!("prepare list courses: {e}"))?;

    let rows = stmt
        .query_map(
            duckdb::params![req.dataset_id, i64::from(limit), i64::from(req.offset)],
            |row| {
                Ok(CourseRow {
                    id: row.get(0)?,
                    row_index: row.get(1)?,
                    subject_code: row.get(2)?,
                    catalog_number: row.get(3)?,
                    course_title: row.get(4)?,
                    content_hash: row.get(5)?,
                    classification: None,
                    probability: None,
                })
            },
        )
        .map_err(|e| format!("query courses: {e}"))?;

    let mut collected: Vec<CourseRow> = rows
        .collect::<Result<_, _>>()
        .map_err(|e| format!("collect courses: {e}"))?;

    // Step 2: if a model was requested, bulk-look-up classifications for the
    // content hashes in this page. The inference_results PK is
    // (model_id, content_hash), so this is a constant-cost index probe per
    // row — far cheaper than evaluating a join across the whole courses
    // partition for every page.
    if let Some(model_id) = req.model_id
        && !collected.is_empty()
    {
        use std::collections::HashMap;
        let mut placeholders = String::with_capacity(collected.len() * 2);
        for i in 0..collected.len() {
            if i > 0 {
                placeholders.push(',');
            }
            placeholders.push('?');
        }
        let sql = format!(
            "SELECT content_hash, classification, probability
             FROM inference_results
             WHERE model_id = ? AND content_hash IN ({placeholders})"
        );
        let mut params: Vec<&dyn duckdb::ToSql> = Vec::with_capacity(collected.len() + 1);
        params.push(&model_id);
        for row in &collected {
            params.push(&row.content_hash);
        }
        let mut lookup_stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("prepare inference lookup: {e}"))?;
        let mut by_hash: HashMap<String, (String, Option<f64>)> = HashMap::new();
        let lookup_rows = lookup_stmt
            .query_map(duckdb::params_from_iter(params), |r| {
                let h: String = r.get(0)?;
                let c: String = r.get(1)?;
                let p: Option<f64> = r.get(2)?;
                Ok((h, (c, p)))
            })
            .map_err(|e| format!("query inference lookup: {e}"))?;
        for entry in lookup_rows {
            let (h, v) = entry.map_err(|e| format!("inference lookup row: {e}"))?;
            by_hash.insert(h, v);
        }
        for row in &mut collected {
            if let Some((label, prob)) = by_hash.get(&row.content_hash) {
                row.classification = Some(label.clone());
                row.probability = *prob;
            }
        }
    }

    Ok(CoursePage {
        rows: collected,
        total,
    })
}

/// Convenience IPC: return the seeded `models.id` for a given digit level so
/// the frontend can request joined results without owning the surrogate id
/// space. Returns `None` if no row matches.
#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn model_id_for_digit_level(
    digit_level: u8,
    db: State<'_, AppDb>,
) -> Result<Option<i64>, String> {
    let conn = db.ro()?;
    let id: Option<i64> = conn
        .query_row(
            "SELECT id FROM models WHERE model_type = ? ORDER BY id LIMIT 1",
            [digit_level.to_string()],
            |row| row.get(0),
        )
        .ok();
    Ok(id)
}
