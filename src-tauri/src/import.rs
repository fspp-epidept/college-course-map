//! Async CSV ingest IPC. `import_csv` validates the file, inserts the
//! `source_files` + `datasets` rows synchronously, returns a dataset id in
//! `importing` state, and spawns the row-loop on `tauri::async_runtime::
//! spawn_blocking`. The frontend polls `list_datasets` to watch `row_count`
//! tick up and `import_state` flip to `ready` / `failed`.
//!
//! Speed: rows are inserted via `DuckDB`'s [`Appender`] API
//! (`appender_with_columns`). The Appender bypasses SQL entirely and writes
//! column chunks directly; on this hardware it pushes ~100–300k rows/sec
//! against the courses table. Multi-row `VALUES` statements (the previous
//! approach) topped out around 6k rows/sec because every batch was a fresh
//! parse + auto-commit + WAL sync.
//!
//! [`Appender`]: duckdb::Appender

use std::{fs::File, io::BufReader, path::Path};

use blake3::Hasher;
use chrono::Utc;
use duckdb::params;
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;

use crate::{
    db::AppDb,
    format::{CourseInput, format_input},
};

const MAX_FILE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_FIELD_BYTES: usize = 8 * 1024;
const MAX_COLUMNS: usize = 256;
/// blake3 read chunk for streaming the file hash.
const HASH_CHUNK: usize = 1024 * 1024;
/// Rows per `appender.flush()`. The Appender batches internally; explicit
/// flushes here bound progress-tick latency. 5000 rows per flush at
/// ~100k rows/sec gives the UI a tick every ~50 ms, plenty for the
/// 1 Hz polling cadence.
const BATCH_SIZE: usize = 5000;

#[derive(Type, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportRequest {
    pub path: String,
    /// Falls back to the filename when null/blank.
    pub display_name: Option<String>,
    /// Optional row cap; `None` means import every row.
    pub limit: Option<u64>,
}

/// Response from `import_csv`: the dataset has been queued and is already
/// streaming rows in. The frontend polls `list_datasets` from here.
#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportStarted {
    pub dataset_id: String,
    pub source_file_id: i64,
}

/// Header aliases per logical field. Match is case-insensitive, exact equality
/// — no fuzzy / contains matching, so a column named `subject_xyz` is *not* a
/// `subject` match. The mapping UI (#62) replaces this with explicit picks.
const SUBJECT_ALIASES: &[&str] = &[
    "subject_code",
    "sub_pref",
    "subject",
    "subj",
    "dept",
    "department",
];
const CATALOG_ALIASES: &[&str] = &[
    "catalog_number",
    "course",
    "course_number",
    "number",
    "cat_no",
    "catalog",
];
const TITLE_ALIASES: &[&str] = &[
    "course_title",
    "title",
    "inventory_course_title",
    "name",
    "course_name",
];

/// Indexes of the mapped columns in the CSV's header order. Persisted to
/// `source_files.column_mapping` so export can reconstruct the original row
/// layout (mapped cells live in the structured `courses` columns, everything
/// else in `extra_columns`). Indexes, not header names: CSVs may repeat a
/// header name, and indexes stay unambiguous.
#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) struct ColumnMap {
    pub subject: usize,
    pub catalog: usize,
    pub title: usize,
}

fn detect_mapping(headers: &[String]) -> Result<ColumnMap, String> {
    let lc: Vec<String> = headers.iter().map(|h| h.to_ascii_lowercase()).collect();
    let find = |aliases: &[&str]| -> Option<usize> {
        aliases
            .iter()
            .find_map(|alias| lc.iter().position(|h| h == alias))
    };
    let subject = find(SUBJECT_ALIASES);
    let catalog = find(CATALOG_ALIASES);
    let title = find(TITLE_ALIASES);

    match (subject, catalog, title) {
        (Some(s), Some(c), Some(t)) => Ok(ColumnMap {
            subject: s,
            catalog: c,
            title: t,
        }),
        _ => Err(format!(
            "could not auto-detect required columns. \
             Found headers: {headers:?}. Need one each of: \
             subject={SUBJECT_ALIASES:?}, catalog={CATALOG_ALIASES:?}, title={TITLE_ALIASES:?}"
        )),
    }
}

#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn import_csv(
    req: ImportRequest,
    app: AppHandle,
    db: State<'_, AppDb>,
) -> Result<ImportStarted, String> {
    let path_str = req.path;
    let p = Path::new(&path_str);
    let metadata = std::fs::metadata(p).map_err(|e| format!("stat {path_str}: {e}"))?;
    if !metadata.is_file() {
        return Err(format!("{path_str}: not a regular file"));
    }
    let size_bytes = metadata.len();
    if size_bytes > MAX_FILE_BYTES {
        return Err(format!(
            "{path_str}: {size_bytes} bytes exceeds {MAX_FILE_BYTES}-byte cap"
        ));
    }

    let imported_hash = hash_file(p)?;
    let display_name = req
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(
            || {
                p.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Untitled dataset")
                    .to_owned()
            },
            ToOwned::to_owned,
        );

    // Validate before we open a transaction so we never insert an empty
    // source_files row on a doomed import.
    let headers = read_headers(p)?;
    if headers.len() > MAX_COLUMNS {
        return Err(format!(
            "{} columns exceeds {MAX_COLUMNS}-column cap",
            headers.len()
        ));
    }
    let mapping = detect_mapping(&headers)?;

    let now = Utc::now().to_rfc3339();
    let dataset_id = Uuid::new_v4().to_string();

    // Persist the header order + mapping for round-trip export (EPI-79):
    // together they let export_results emit a column-identical copy of the
    // input with the ccm_* columns appended.
    let headers_json =
        serde_json::to_string(&headers).map_err(|e| format!("serialize headers: {e}"))?;
    let mapping_json =
        serde_json::to_string(&mapping).map_err(|e| format!("serialize mapping: {e}"))?;

    let source_file_id: i64 = {
        let conn = db.rw()?;
        let source_file_id: i64 = conn
            .query_row(
                "INSERT INTO source_files
                    (path, display_name, imported_at, imported_hash, size_bytes,
                     original_headers, column_mapping)
                 VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id",
                params![
                    path_str,
                    &display_name,
                    &now,
                    &imported_hash,
                    i64::try_from(size_bytes).unwrap_or(i64::MAX),
                    &headers_json,
                    &mapping_json,
                ],
                |row| row.get(0),
            )
            .map_err(|e| format!("insert source_files: {e}"))?;

        conn.execute(
            "INSERT INTO datasets
                (id, title, source_kind, source_file_id, imported_at, row_count, import_state)
             VALUES (?, ?, 'file', ?, ?, 0, 'importing')",
            params![dataset_id, &display_name, source_file_id, &now],
        )
        .map_err(|e| format!("insert datasets: {e}"))?;
        source_file_id
    };

    // Spawn the row loop. Owned values only so the closure has no borrowed
    // state to outlive.
    let task = ImportTask {
        app: app.clone(),
        path: path_str,
        dataset_id: dataset_id.clone(),
        mapping,
        limit: req.limit,
    };
    tauri::async_runtime::spawn_blocking(move || task.run());

    Ok(ImportStarted {
        dataset_id,
        source_file_id,
    })
}

struct ImportTask {
    app: AppHandle,
    path: String,
    dataset_id: String,
    mapping: ColumnMap,
    limit: Option<u64>,
}

impl ImportTask {
    fn run(self) {
        match self.run_inner() {
            Ok((imported, _skipped)) => self.mark_ready(imported),
            Err(err) => self.mark_failed(&err),
        }
    }

    /// Stream the CSV and bulk-insert in fixed-size batches.
    /// Returns `(imported, skipped)`.
    fn run_inner(&self) -> Result<(u64, u64), String> {
        let path = Path::new(&self.path);
        let file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(true)
            .from_reader(BufReader::new(file));

        let mut imported: u64 = 0;
        let mut skipped: u64 = 0;
        let mut row_index: i64 = 0;
        let mut batch: Vec<BatchRow> = Vec::with_capacity(BATCH_SIZE);

        for record in reader.records() {
            if let Some(cap) = self.limit
                && imported >= cap
            {
                break;
            }
            let record = record.map_err(|e| format!("read row {row_index}: {e}"))?;
            let subject = record
                .get(self.mapping.subject)
                .map(str::trim)
                .unwrap_or_default();
            let catalog = record
                .get(self.mapping.catalog)
                .map(str::trim)
                .unwrap_or_default();
            let title = record
                .get(self.mapping.title)
                .map(str::trim)
                .unwrap_or_default();

            if subject.is_empty() || catalog.is_empty() || title.is_empty() {
                skipped += 1;
                row_index += 1;
                continue;
            }

            let subject = truncate(subject.to_owned());
            let catalog = truncate(catalog.to_owned());
            let title = truncate(title.to_owned());

            let formatted = format_input(&CourseInput {
                subject_code: subject.clone(),
                catalog_number: catalog.clone(),
                course_title: title.clone(),
            });
            let mut hasher = Hasher::new();
            hasher.update(formatted.as_bytes());
            let content_hash = hasher.finalize().to_hex().to_string();

            batch.push(BatchRow {
                row_index,
                subject,
                catalog,
                title,
                content_hash,
                extra_columns: extra_columns_json(&record, self.mapping)?,
            });
            imported += 1;
            row_index += 1;

            if batch.len() >= BATCH_SIZE {
                self.flush(&batch)?;
                batch.clear();
                // Tick once per batch — at BATCH_SIZE=500 and ~50k rows/sec
                // that's every ~10 ms, which is plenty for the 500 ms poll.
                self.tick_row_count(imported)?;
            }
        }

        if !batch.is_empty() {
            self.flush(&batch)?;
        }
        // Final row_count is set in mark_ready.
        Ok((imported, skipped))
    }

    /// Bulk-insert one batch via the `DuckDB` Appender. `appender_with_columns`
    /// lets us omit the `id` (sequence default) + `is_classifiable` (TRUE
    /// default) + the nullable description / school / parse fields, so we only
    /// push the columns we actually care about.
    fn flush(&self, batch: &[BatchRow]) -> Result<(), String> {
        if batch.is_empty() {
            return Ok(());
        }
        let db = self.app.state::<AppDb>();
        let conn = db.rw()?;
        let mut appender = conn
            .appender_with_columns(
                "courses",
                &[
                    "dataset_id",
                    "row_index",
                    "subject_code",
                    "catalog_number",
                    "course_title",
                    "content_hash",
                    "extra_columns",
                ],
            )
            .map_err(|e| format!("open appender: {e}"))?;
        for row in batch {
            appender
                .append_row(params![
                    self.dataset_id.as_str(),
                    row.row_index,
                    row.subject.as_str(),
                    row.catalog.as_str(),
                    row.title.as_str(),
                    row.content_hash.as_str(),
                    row.extra_columns.as_deref(),
                ])
                .map_err(|e| format!("appender append_row: {e}"))?;
        }
        // Drop flushes implicitly, but doing it explicitly surfaces any error
        // at the call site rather than silently in the destructor.
        appender
            .flush()
            .map_err(|e| format!("appender flush: {e}"))?;
        Ok(())
    }

    fn tick_row_count(&self, imported: u64) -> Result<(), String> {
        let db = self.app.state::<AppDb>();
        let conn = db.rw()?;
        conn.execute(
            "UPDATE datasets SET row_count = ? WHERE id = ?",
            params![
                i64::try_from(imported).unwrap_or(i64::MAX),
                &self.dataset_id,
            ],
        )
        .map_err(|e| format!("update row_count: {e}"))?;
        Ok(())
    }

    fn mark_ready(&self, imported: u64) {
        let db = self.app.state::<AppDb>();
        let Ok(conn) = db.rw() else {
            eprintln!(
                "import {}: rw mutex poisoned at mark_ready",
                self.dataset_id
            );
            return;
        };
        if let Err(e) = conn.execute(
            "UPDATE datasets
                SET row_count = ?, import_state = 'ready', import_error = NULL
              WHERE id = ?",
            params![
                i64::try_from(imported).unwrap_or(i64::MAX),
                &self.dataset_id,
            ],
        ) {
            eprintln!("import {}: mark_ready: {e}", self.dataset_id);
        }
        // CHECKPOINT compacts the WAL into the main file. Without this, the
        // first read against the freshly-imported dataset pays the merge cost
        // for every row — on a 2M-row import that shows up as a UI hang
        // when opening the dataset tab.
        if let Err(e) = conn.execute_batch("CHECKPOINT") {
            eprintln!("import {}: post-import checkpoint: {e}", self.dataset_id);
        }
    }

    fn mark_failed(&self, err: &str) {
        let db = self.app.state::<AppDb>();
        let Ok(conn) = db.rw() else {
            eprintln!(
                "import {}: rw mutex poisoned at mark_failed",
                self.dataset_id
            );
            return;
        };
        if let Err(e) = conn.execute(
            "UPDATE datasets SET import_state = 'failed', import_error = ? WHERE id = ?",
            params![err, &self.dataset_id],
        ) {
            eprintln!("import {}: mark_failed: {e}", self.dataset_id);
        }
    }
}

struct BatchRow {
    row_index: i64,
    subject: String,
    catalog: String,
    title: String,
    content_hash: String,
    /// JSON object of the row's unmapped cells keyed by column index
    /// (`{"3": "…"}`), `None` when the file has no unmapped columns. See
    /// [`extra_columns_json`].
    extra_columns: Option<String>,
}

/// Serialize the unmapped cells of one record as a JSON object keyed by
/// column index (as a string — JSON object keys are strings). Mapped cells
/// (subject/catalog/title) are excluded: they live in the structured
/// `courses` columns and export reconstructs them from there. Cells pass the
/// same [`truncate`] bound as mapped fields; untrusted content is neutralized
/// at export time (CSV-injection escaping), not here.
fn extra_columns_json(
    record: &csv::StringRecord,
    mapping: ColumnMap,
) -> Result<Option<String>, String> {
    let mut map = serde_json::Map::new();
    for (i, field) in record.iter().enumerate() {
        if i == mapping.subject || i == mapping.catalog || i == mapping.title {
            continue;
        }
        map.insert(
            i.to_string(),
            serde_json::Value::String(truncate(field.to_owned())),
        );
    }
    if map.is_empty() {
        return Ok(None);
    }
    serde_json::to_string(&serde_json::Value::Object(map))
        .map(Some)
        .map_err(|e| format!("serialize extra columns: {e}"))
}

fn read_headers(path: &Path) -> Result<Vec<String>, String> {
    let file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(BufReader::new(file));
    Ok(reader
        .headers()
        .map_err(|e| format!("read headers: {e}"))?
        .iter()
        .map(|h| truncate(h.to_owned()))
        .collect())
}

fn hash_file(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let mut file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let mut hasher = Hasher::new();
    let mut buf = vec![0_u8; HASH_CHUNK];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("read {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(buf.get(..n).unwrap_or(&[]));
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn truncate(mut s: String) -> String {
    if s.len() > MAX_FIELD_BYTES {
        let mut cut = MAX_FIELD_BYTES;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::{ColumnMap, extra_columns_json};

    /// Unmapped cells are keyed by column index; mapped cells are excluded;
    /// a file with only mapped columns produces `None` (NULL in the DB).
    #[test]
    fn extra_columns_keyed_by_index_excluding_mapped() -> Result<(), String> {
        let mapping = ColumnMap {
            subject: 0,
            catalog: 2,
            title: 3,
        };
        let record = csv::StringRecord::from(vec!["ECON", "Fall 2024", "101", "Micro", ""]);
        let json = extra_columns_json(&record, mapping)?
            .ok_or_else(|| "expected extra columns".to_owned())?;
        let value: serde_json::Value = serde_json::from_str(&json).map_err(|e| e.to_string())?;
        assert_eq!(
            value,
            serde_json::json!({ "1": "Fall 2024", "4": "" }),
            "got {value}"
        );

        let all_mapped = csv::StringRecord::from(vec!["ECON", "x", "101", "Micro"]);
        let mapping_all = ColumnMap {
            subject: 0,
            catalog: 2,
            title: 3,
        };
        // Column 1 is unmapped, so this still yields a map…
        assert!(extra_columns_json(&all_mapped, mapping_all)?.is_some());
        // …but a three-column file that maps everything yields None.
        let three = csv::StringRecord::from(vec!["ECON", "101", "Micro"]);
        let mapping_three = ColumnMap {
            subject: 0,
            catalog: 1,
            title: 2,
        };
        assert!(extra_columns_json(&three, mapping_three)?.is_none());
        Ok(())
    }

    /// The Appender path used by `flush` accepts JSON strings (and NULL) into
    /// a JSON column, and the values read back via `json_extract_string` —
    /// the same access pattern export uses.
    #[test]
    fn appender_writes_json_column() -> Result<(), String> {
        let conn = duckdb::Connection::open_in_memory().map_err(|e| e.to_string())?;
        conn.execute_batch("CREATE TABLE t (row_index BIGINT, extra_columns JSON)")
            .map_err(|e| e.to_string())?;
        {
            let mut appender = conn
                .appender_with_columns("t", &["row_index", "extra_columns"])
                .map_err(|e| e.to_string())?;
            appender
                .append_row(duckdb::params![0_i64, Some(r#"{"1": "Fall 2024"}"#)])
                .map_err(|e| e.to_string())?;
            appender
                .append_row(duckdb::params![1_i64, None::<&str>])
                .map_err(|e| e.to_string())?;
            appender.flush().map_err(|e| e.to_string())?;
        }
        let val: Option<String> = conn
            .query_row(
                "SELECT json_extract_string(extra_columns, '$.\"1\"') FROM t WHERE row_index = 0",
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        assert_eq!(val.as_deref(), Some("Fall 2024"));
        let null_row: Option<String> = conn
            .query_row(
                "SELECT extra_columns FROM t WHERE row_index = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        assert_eq!(null_row, None);
        Ok(())
    }
}
