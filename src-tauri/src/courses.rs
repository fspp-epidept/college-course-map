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

    let total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM courses WHERE dataset_id = ?",
            [&req.dataset_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("count courses: {e}"))?;

    // The join is parameterised over model_id; the IS NULL guard yields a
    // single SQL statement that works for both "with results" and "preview
    // only" callers, instead of two separate prepared queries.
    let mut stmt = conn
        .prepare(
            "SELECT c.id, c.row_index, c.subject_code, c.catalog_number,
                    c.course_title, c.content_hash,
                    ir.classification, ir.probability
             FROM courses c
             LEFT JOIN inference_results ir
                 ON ir.content_hash = c.content_hash
                AND ir.model_id = ?
             WHERE c.dataset_id = ?
             ORDER BY c.row_index
             LIMIT ? OFFSET ?",
        )
        .map_err(|e| format!("prepare list courses: {e}"))?;

    let model_id_param: Option<i64> = req.model_id;
    let rows = stmt
        .query_map(
            duckdb::params![
                model_id_param,
                req.dataset_id,
                i64::from(limit),
                i64::from(req.offset),
            ],
            |row| {
                Ok(CourseRow {
                    id: row.get(0)?,
                    row_index: row.get(1)?,
                    subject_code: row.get(2)?,
                    catalog_number: row.get(3)?,
                    course_title: row.get(4)?,
                    content_hash: row.get(5)?,
                    classification: row.get(6)?,
                    probability: row.get(7)?,
                })
            },
        )
        .map_err(|e| format!("query courses: {e}"))?;

    let collected: Vec<CourseRow> = rows
        .collect::<Result<_, _>>()
        .map_err(|e| format!("collect courses: {e}"))?;

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
