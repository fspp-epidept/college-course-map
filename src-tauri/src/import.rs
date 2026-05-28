//! CSV ingest IPC. Streams a user-picked file into the source/dataset/course
//! tables in a single transaction. The column mapping is auto-detected from
//! the header row via a small alias table; the full mapping UI is a follow-up.
//!
//! Same hostile-input posture as `csv_io::preview_csv`: bounded field length,
//! bounded column count, hard file-size cap, paths used only for reading.

use std::{fs::File, io::BufReader, path::Path};

use blake3::Hasher;
use chrono::Utc;
use duckdb::params;
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;
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

#[derive(Type, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportRequest {
    pub path: String,
    /// Falls back to the filename when null/blank.
    pub display_name: Option<String>,
    /// Optional row cap; `None` means import every row. The spike UI sends 200.
    pub limit: Option<u64>,
}

#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportResult {
    pub dataset_id: String,
    pub source_file_id: i64,
    pub rows_imported: u64,
    pub rows_skipped: u64,
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
pub(crate) fn import_csv(req: ImportRequest, db: State<'_, AppDb>) -> Result<ImportResult, String> {
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

    // Headers first — fail before opening a transaction if the mapping doesn't
    // resolve, so we never insert an empty source_files row on a doomed import.
    let headers = read_headers(p)?;
    if headers.len() > MAX_COLUMNS {
        return Err(format!(
            "{} columns exceeds {MAX_COLUMNS}-column cap",
            headers.len()
        ));
    }
    let mapping = detect_mapping(&headers)?;

    let conn = db.rw()?;
    let now = Utc::now().to_rfc3339();

    conn.execute_batch("BEGIN")
        .map_err(|e| format!("begin tx: {e}"))?;

    let outcome = (|| -> Result<ImportResult, String> {
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

        let dataset_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO datasets
                (id, title, source_kind, source_file_id, imported_at, row_count)
             VALUES (?, ?, 'file', ?, ?, 0)",
            params![dataset_id, &display_name, source_file_id, &now],
        )
        .map_err(|e| format!("insert datasets: {e}"))?;

        let (rows_imported, rows_skipped) =
            ingest_rows(&conn, p, &mapping, &dataset_id, req.limit)?;

        conn.execute(
            "UPDATE datasets SET row_count = ? WHERE id = ?",
            params![i64::try_from(rows_imported).unwrap_or(i64::MAX), dataset_id],
        )
        .map_err(|e| format!("update datasets.row_count: {e}"))?;

        Ok(ImportResult {
            dataset_id,
            source_file_id,
            rows_imported,
            rows_skipped,
        })
    })();

    match outcome {
        Ok(result) => {
            conn.execute_batch("COMMIT")
                .map_err(|e| format!("commit tx: {e}"))?;
            Ok(result)
        }
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(err)
        }
    }
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

fn ingest_rows(
    conn: &duckdb::Connection,
    path: &Path,
    mapping: &ColumnMap,
    dataset_id: &str,
    limit: Option<u64>,
) -> Result<(u64, u64), String> {
    let file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(BufReader::new(file));

    let mut imported: u64 = 0;
    let mut skipped: u64 = 0;
    let mut row_index: i64 = 0;

    let mut stmt = conn
        .prepare(
            "INSERT INTO courses
                (dataset_id, row_index, subject_code, catalog_number,
                 course_title, content_hash)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .map_err(|e| format!("prepare INSERT courses: {e}"))?;

    for record in reader.records() {
        if let Some(cap) = limit
            && imported >= cap
        {
            break;
        }
        let record = record.map_err(|e| format!("read row {row_index}: {e}"))?;
        let subject = record
            .get(mapping.subject)
            .map(str::trim)
            .unwrap_or_default();
        let catalog = record
            .get(mapping.catalog)
            .map(str::trim)
            .unwrap_or_default();
        let title = record.get(mapping.title).map(str::trim).unwrap_or_default();

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

        stmt.execute(params![
            dataset_id,
            row_index,
            &subject,
            &catalog,
            &title,
            &content_hash,
        ])
        .map_err(|e| format!("insert course row {row_index}: {e}"))?;

        imported += 1;
        row_index += 1;
    }

    Ok((imported, skipped))
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
