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

#[derive(Clone, Copy)]
struct ColumnMap {
    subject: usize,
    catalog: usize,
    title: usize,
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

    let source_file_id: i64 = {
        let conn = db.rw()?;
        let source_file_id: i64 = conn
            .query_row(
                "INSERT INTO source_files
                    (path, display_name, imported_at, imported_hash, size_bytes)
                 VALUES (?, ?, ?, ?, ?) RETURNING id",
                params![
                    path_str,
                    &display_name,
                    &now,
                    &imported_hash,
                    i64::try_from(size_bytes).unwrap_or(i64::MAX),
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
    /// default) + the nullable description / school / `extra_columns` / parse
    /// fields, so we only push the six columns we actually care about.
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
