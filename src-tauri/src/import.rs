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

use std::{fs::File, path::Path};

use blake3::Hasher;
use chrono::Utc;
use duckdb::params;
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;

use crate::{
    db::AppDb,
    format::{CourseInput, content_hash},
    preflight::{
        ColumnMap, MAX_COLUMNS, TextEncoding, check_mapping, mapped_cells, open_csv,
        spreadsheet_row, stat_source, truncate,
    },
    profile::{InputProfile, InputProfiler},
};

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
    /// Confirmed by the user from `inspect_csv`'s encoding report.
    pub encoding: TextEncoding,
    /// The mapping `validate_import` returned; re-checked against the header.
    pub mapping: ColumnMap,
}

/// Response from `import_csv`: the dataset has been queued and is already
/// streaming rows in. The frontend polls `list_datasets` from here.
#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportStarted {
    pub dataset_id: String,
    pub source_file_id: i64,
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
    let size_bytes = stat_source(p)?;

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
    let headers = read_headers(p, req.encoding)?;
    if headers.len() > MAX_COLUMNS {
        return Err(format!(
            "{} columns exceeds {MAX_COLUMNS}-column cap",
            headers.len()
        ));
    }
    let mapping = req.mapping;
    check_mapping(mapping, headers.len())?;

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
                     original_headers, column_mapping, encoding)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
                params![
                    path_str,
                    &display_name,
                    &now,
                    &imported_hash,
                    i64::try_from(size_bytes).unwrap_or(i64::MAX),
                    &headers_json,
                    &mapping_json,
                    req.encoding.label(),
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
        encoding: req.encoding,
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
    encoding: TextEncoding,
    mapping: ColumnMap,
    limit: Option<u64>,
}

impl ImportTask {
    fn run(self) {
        match self.run_inner() {
            Ok((imported, profile)) => self.mark_ready(imported, &profile),
            Err(err) => self.mark_failed(&err),
        }
    }

    /// Stream the CSV and bulk-insert in fixed-size batches. Every record
    /// read also feeds the input profile, so a `limit`-capped import profiles
    /// only the rows it read. Returns `(imported, profile)`.
    fn run_inner(&self) -> Result<(u64, InputProfile), String> {
        let mut reader = open_csv(Path::new(&self.path), self.encoding)?;
        let headers = reader.headers().map_err(|e| format!("read headers: {e}"))?;
        let header = |i: usize| truncate(headers.get(i).unwrap_or_default().to_owned());
        let mut profiler = InputProfiler::new([
            header(self.mapping.subject),
            header(self.mapping.catalog),
            header(self.mapping.title),
        ]);

        let mut imported: u64 = 0;
        let mut row_index: i64 = 0;
        let mut batch: Vec<BatchRow> = Vec::with_capacity(BATCH_SIZE);

        for record in reader.records() {
            if let Some(cap) = self.limit
                && imported >= cap
            {
                break;
            }
            let record = record.map_err(|e| format!("read row {row_index}: {e}"))?;
            let cells = mapped_cells(&record, self.mapping);
            profiler.observe(
                spreadsheet_row(u64::try_from(row_index).unwrap_or(u64::MAX)),
                cells,
            );
            let [subject, catalog, title] = cells;

            if subject.is_empty() || catalog.is_empty() || title.is_empty() {
                row_index += 1;
                continue;
            }

            let subject = truncate(subject.to_owned());
            let catalog = truncate(catalog.to_owned());
            let title = truncate(title.to_owned());

            let content_hash = content_hash(&CourseInput {
                subject_code: subject.clone(),
                catalog_number: catalog.clone(),
                course_title: title.clone(),
            });

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
        Ok((imported, profiler.finish()))
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

    fn mark_ready(&self, imported: u64, profile: &InputProfile) {
        // Serialize before taking the lock: `mark_failed` takes it too.
        let profile_json = match serde_json::to_string(profile) {
            Ok(json) => json,
            Err(e) => {
                self.mark_failed(&format!("serialize input profile: {e}"));
                return;
            }
        };
        let db = self.app.state::<AppDb>();
        let Ok(conn) = db.rw() else {
            log::error!(
                "import {}: rw mutex poisoned at mark_ready",
                self.dataset_id
            );
            return;
        };
        // The JSON column takes the string directly (VARCHAR -> JSON cast).
        if let Err(e) = conn.execute(
            "UPDATE datasets
                SET row_count = ?, import_state = 'ready', import_error = NULL,
                    input_profile = ?
              WHERE id = ?",
            params![
                i64::try_from(imported).unwrap_or(i64::MAX),
                &profile_json,
                &self.dataset_id,
            ],
        ) {
            log::error!("import {}: mark_ready: {e}", self.dataset_id);
        }
        // CHECKPOINT compacts the WAL into the main file. Without this, the
        // first read against the freshly-imported dataset pays the merge cost
        // for every row — on a 2M-row import that shows up as a UI hang
        // when opening the dataset tab.
        if let Err(e) = conn.execute_batch("CHECKPOINT") {
            log::warn!("import {}: post-import checkpoint: {e}", self.dataset_id);
        }
    }

    fn mark_failed(&self, err: &str) {
        let db = self.app.state::<AppDb>();
        let Ok(conn) = db.rw() else {
            log::error!(
                "import {}: rw mutex poisoned at mark_failed",
                self.dataset_id
            );
            return;
        };
        if let Err(e) = conn.execute(
            "UPDATE datasets SET import_state = 'failed', import_error = ? WHERE id = ?",
            params![err, &self.dataset_id],
        ) {
            log::error!("import {}: mark_failed: {e}", self.dataset_id);
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

fn read_headers(path: &Path, encoding: TextEncoding) -> Result<Vec<String>, String> {
    let mut reader = open_csv(path, encoding)?;
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

#[cfg(test)]
mod tests {
    use super::extra_columns_json;
    use crate::preflight::ColumnMap;

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

    /// `mark_ready` writes the serialized profile into the JSON column as a
    /// plain string parameter, and `get_input_profile` reads it back through
    /// `::VARCHAR`. Round-trips through the real schema.
    #[test]
    fn input_profile_round_trips_through_json_column() -> Result<(), String> {
        use crate::profile::InputProfiler;

        let conn = duckdb::Connection::open_in_memory().map_err(|e| e.to_string())?;
        crate::db::migrate(&conn)?;
        conn.execute_batch(
            "INSERT INTO datasets (id, title, source_kind, imported_at, row_count, import_state)
             VALUES ('d1', 't', 'file', now(), 0, 'importing')",
        )
        .map_err(|e| e.to_string())?;

        let mut profiler = InputProfiler::new(["s".into(), "c".into(), "t".into()]);
        profiler.observe(2, ["PSYC", "PSYC 4325", "ABNORMAL"]);
        profiler.observe(3, ["ECON", "", "MICRO"]);
        let profile = profiler.finish();
        let json = serde_json::to_string(&profile).map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE datasets SET input_profile = ? WHERE id = ?",
            duckdb::params![&json, "d1"],
        )
        .map_err(|e| e.to_string())?;

        let stored: Option<String> = conn
            .query_row(
                "SELECT input_profile::VARCHAR FROM datasets WHERE id = ?",
                ["d1"],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let back: crate::profile::InputProfile =
            serde_json::from_str(stored.as_deref().ok_or("NULL profile")?)
                .map_err(|e| e.to_string())?;
        assert_eq!(
            (back.rows, back.importable, back.skipped.catalog),
            (2, 1, 1)
        );
        assert_eq!(back.samples.first().map(|s| s.row), Some(2));
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
