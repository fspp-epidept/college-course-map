//! Paginated courses listing for the dataset detail view. Each row is joined
//! left against `inference_results` for an optional model so the same query
//! powers both the unclassified preview and the post-run results browser.

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;

use crate::boot::{Boot, Services};

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
    /// Key-set cursor: include rows with `row_index >= cursor`. `None` (and 0)
    /// mean "from the start". The frontend hands back the row index of the
    /// last row of a page + 1 to advance. Replaces `OFFSET` because `DuckDB`'s
    /// `TopN` plan for `ORDER BY row_index LIMIT n OFFSET m` scans the whole
    /// partition; the range predicate lets the scan skip to the cursor.
    pub cursor: Option<i64>,
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
    /// Canonical CCM code from `inference_results` for the requested model;
    /// `None` when there's no result yet (or no `model_id` was requested).
    pub classification: Option<String>,
    /// Softmax confidence at argmax, `(0, 1]`. See `docs/model-confidence.md`.
    pub probability: Option<f64>,
    /// Official CCM title for the code, joined from `ccm_taxonomy`. For
    /// 6-digit codes missing from the table, this is the 2-digit parent's
    /// title — `ccm_title_level` says which level matched. Always `None` for
    /// 4-digit codes: the CCM publishes no 4-digit titles (EPI-112).
    pub ccm_title: Option<String>,
    pub ccm_title_short: Option<String>,
    /// Only 6-digit taxonomy rows carry descriptions.
    pub ccm_description: Option<String>,
    /// Digit level the title came from (the model's own level, or 2 for the
    /// parent fallback); `None` when no taxonomy row matched.
    pub ccm_title_level: Option<u8>,
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
    boot: State<'_, Boot>,
) -> Result<CoursePage, String> {
    let limit = req.limit.clamp(1, MAX_PAGE_SIZE);
    let conn = boot.ready()?.db.ro()?;

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

    // Step 1: page the courses table on its own via key-set cursor. The
    // range predicate keeps the scan to the rows at the cursor — no 2M-row
    // TopN like OFFSET would force, even for the first page. No index is
    // involved (0008 dropped it): a page measured ~2 ms on 2.9M rows.
    let cursor = req.cursor.unwrap_or(0);
    let mut stmt = conn
        .prepare(
            "SELECT id, row_index, subject_code, catalog_number,
                    course_title, content_hash
             FROM courses
             WHERE dataset_id = ? AND row_index >= ?
             ORDER BY row_index
             LIMIT ?",
        )
        .map_err(|e| format!("prepare list courses: {e}"))?;

    let rows = stmt
        .query_map(
            duckdb::params![req.dataset_id, cursor, i64::from(limit)],
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
                    ccm_title: None,
                    ccm_title_short: None,
                    ccm_description: None,
                    ccm_title_level: None,
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
        attach_results(&conn, model_id, &mut collected)?;
    }

    Ok(CoursePage {
        rows: collected,
        total,
    })
}

/// Classification + taxonomy fields for one cached result, keyed by hash in
/// [`attach_results`].
struct ResultInfo {
    classification: String,
    probability: Option<f64>,
    title: Option<String>,
    title_short: Option<String>,
    description: Option<String>,
    title_level: Option<u8>,
}

/// Bulk-attach cached classifications (plus their `ccm_taxonomy` titles) to a
/// page of course rows. One index probe per hash against the
/// `(model_id, content_hash)` PK. The model's digit level drives the taxonomy
/// join: exact match at the model's own level, else the 2-digit parent by
/// code prefix for any 6-digit code absent from the table. 4-digit results
/// get no title: the government publishes no 4-digit taxonomy, and a parent
/// title would read as a 4-digit one (EPI-112).
fn attach_results(
    conn: &duckdb::Connection,
    model_id: i64,
    collected: &mut [CourseRow],
) -> Result<(), String> {
    use std::collections::HashMap;

    let digit_level: u8 = conn
        .query_row(
            "SELECT model_type FROM models WHERE id = ?",
            [model_id],
            |row| row.get::<_, String>(0),
        )
        .map_err(|e| format!("model digit lookup: {e}"))?
        .parse()
        .map_err(|e| format!("non-numeric model_type: {e}"))?;

    let mut placeholders = String::with_capacity(collected.len() * 2);
    for i in 0..collected.len() {
        if i > 0 {
            placeholders.push(',');
        }
        placeholders.push('?');
    }
    let sql = format!(
        "SELECT r.content_hash, r.classification, r.probability,
                t.title, t.title_short, t.description,
                p.title, p.title_short
         FROM inference_results r
         LEFT JOIN ccm_taxonomy t
           ON t.digit_level = ? AND t.code = r.classification
         LEFT JOIN ccm_taxonomy p
           ON p.digit_level = 2 AND p.code = substr(r.classification, 1, 2)
         WHERE r.model_id = ? AND r.content_hash IN ({placeholders})"
    );
    let parent_fallback = digit_level != 4;
    let digit_i64 = i64::from(digit_level);
    let mut params: Vec<&dyn duckdb::ToSql> = Vec::with_capacity(collected.len() + 2);
    params.push(&digit_i64);
    params.push(&model_id);
    for row in collected.iter() {
        params.push(&row.content_hash);
    }
    let mut lookup_stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("prepare inference lookup: {e}"))?;

    let mut by_hash: HashMap<String, ResultInfo> = HashMap::new();
    let lookup_rows = lookup_stmt
        .query_map(duckdb::params_from_iter(params), |r| {
            let hash: String = r.get(0)?;
            let exact_title: Option<String> = r.get(3)?;
            let exact_short: Option<String> = r.get(4)?;
            let exact_desc: Option<String> = r.get(5)?;
            let (parent_title, parent_short): (Option<String>, Option<String>) = if parent_fallback
            {
                (r.get(6)?, r.get(7)?)
            } else {
                (None, None)
            };
            let info = if exact_title.is_some() {
                ResultInfo {
                    classification: r.get(1)?,
                    probability: r.get(2)?,
                    title: exact_title,
                    title_short: exact_short,
                    description: exact_desc,
                    title_level: Some(digit_level),
                }
            } else {
                ResultInfo {
                    classification: r.get(1)?,
                    probability: r.get(2)?,
                    title: parent_title.clone(),
                    title_short: parent_short,
                    description: None,
                    title_level: parent_title.is_some().then_some(2),
                }
            };
            Ok((hash, info))
        })
        .map_err(|e| format!("query inference lookup: {e}"))?;
    for entry in lookup_rows {
        let (h, v) = entry.map_err(|e| format!("inference lookup row: {e}"))?;
        by_hash.insert(h, v);
    }
    for row in collected.iter_mut() {
        if let Some(info) = by_hash.get(&row.content_hash) {
            row.classification = Some(info.classification.clone());
            row.probability = info.probability;
            row.ccm_title = info.title.clone();
            row.ccm_title_short = info.title_short.clone();
            row.ccm_description = info.description.clone();
            row.ccm_title_level = info.title_level;
        }
    }
    Ok(())
}

/// Convenience IPC: return the manifest-active `models.id` for a digit level
/// so the frontend can request joined results without owning the surrogate id
/// space. Resolved through the catalog — never by SQL guessing, which would
/// happily pick a stale row from an earlier model family.
#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn model_id_for_digit_level(
    digit_level: u8,
    boot: State<'_, Boot>,
) -> Result<Option<i64>, String> {
    Ok(boot.ready()?.catalog.model_id(digit_level))
}

/// Per-model classification coverage for one dataset: how many of its courses
/// already have a cached result for each manifest-active model. Drives the
/// dataset page's per-level coverage and the classify confirm panel's
/// "already classified" count. Counts are course-level (duplicate content
/// hashes count once per course row), in the same units as a job's live
/// progress (`classify::LevelProgress`).
#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CoverageRow {
    pub model_id: i64,
    pub digit_level: u8,
    pub classified: i64,
    pub total: i64,
}

#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn get_classification_coverage(
    dataset_id: String,
    boot: State<'_, Boot>,
) -> Result<Vec<CoverageRow>, String> {
    let Services { db, catalog, .. } = boot.ready()?;
    let conn = db.ro()?;
    // Cached row count, same rationale as the pager: COUNT(*) over a
    // multi-million-row partition is a scan we don't need.
    let total: i64 = conn
        .query_row(
            "SELECT row_count FROM datasets WHERE id = ?",
            duckdb::params![dataset_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("dataset {dataset_id}: {e}"))?;

    let mut out: Vec<CoverageRow> = Vec::with_capacity(catalog.manifest.model.len());
    for entry in &catalog.manifest.model {
        let Some(model_id) = catalog.model_id(entry.digit_level) else {
            continue;
        };
        let classified: i64 = conn
            .query_row(
                "SELECT COUNT(*)
                 FROM courses c
                 JOIN inference_results ir
                   ON ir.content_hash = c.content_hash AND ir.model_id = ?
                 WHERE c.dataset_id = ?",
                duckdb::params![model_id, dataset_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("coverage for model {model_id}: {e}"))?;
        out.push(CoverageRow {
            model_id,
            digit_level: entry.digit_level,
            classified,
            total,
        });
    }
    out.sort_by_key(|r| r.digit_level);
    Ok(out)
}
