//! CSV preview IPC. Reads a small sample from a user-picked file so the
//! frontend can render headers + first rows without ever touching the file
//! itself. Treats the path as hostile input per the CLAUDE.md security baseline:
//! file size is bounded, field length is bounded, column count is bounded, and
//! the path is used here and nowhere else.

use std::{fs::File, io::BufReader, path::Path};

use serde::Serialize;
use specta::Type;

/// 1 GiB hard cap. Real working CSVs top out around 200 MB; rejecting anything
/// bigger keeps a malformed multi-GB file from stalling preview indefinitely.
const MAX_FILE_BYTES: u64 = 1024 * 1024 * 1024;
/// 8 KiB per field — long enough for any course description, short enough to
/// prevent a single hostile cell from ballooning preview memory.
const MAX_FIELD_BYTES: usize = 8 * 1024;
/// 256 columns — the panel CSV has 13; "real" CSVs rarely exceed a few dozen.
const MAX_COLUMNS: usize = 256;
/// Rows shown to the user in the preview table.
const SAMPLE_ROW_LIMIT: usize = 5;

#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CsvPreview {
    pub headers: Vec<String>,
    pub sample_rows: Vec<Vec<String>>,
    pub total_columns: u32,
    pub size_bytes: u64,
}

/// Read up to [`SAMPLE_ROW_LIMIT`] rows from the CSV at `path`, returning the
/// headers + sample rows + file metadata. Never persists anything — the only
/// side effect is opening the file for reading.
#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn preview_csv(path: String) -> Result<CsvPreview, String> {
    let p = Path::new(&path);
    let metadata = std::fs::metadata(p).map_err(|e| format!("stat {path}: {e}"))?;
    if !metadata.is_file() {
        return Err(format!("{path}: not a regular file"));
    }
    let size_bytes = metadata.len();
    if size_bytes > MAX_FILE_BYTES {
        return Err(format!(
            "{path}: {size_bytes} bytes exceeds {MAX_FILE_BYTES}-byte cap"
        ));
    }

    let file = File::open(p).map_err(|e| format!("open {path}: {e}"))?;
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(BufReader::new(file));

    let headers = reader
        .headers()
        .map_err(|e| format!("read headers: {e}"))?
        .iter()
        .map(|h| truncate(h.to_owned()))
        .collect::<Vec<String>>();

    if headers.len() > MAX_COLUMNS {
        return Err(format!(
            "{} columns exceeds {MAX_COLUMNS}-column cap",
            headers.len()
        ));
    }

    let total_columns = u32::try_from(headers.len()).unwrap_or(u32::MAX);

    let mut sample_rows: Vec<Vec<String>> = Vec::with_capacity(SAMPLE_ROW_LIMIT);
    for record in reader.records().take(SAMPLE_ROW_LIMIT) {
        let record = record.map_err(|e| format!("read row: {e}"))?;
        sample_rows.push(record.iter().map(|f| truncate(f.to_owned())).collect());
    }

    Ok(CsvPreview {
        headers,
        sample_rows,
        total_columns,
        size_bytes,
    })
}

/// Truncate a single CSV cell to [`MAX_FIELD_BYTES`]. We do this on the way out
/// rather than at the parser level so we surface the raw header text the user
/// will see in the mapping step — just bounded.
fn truncate(mut s: String) -> String {
    if s.len() > MAX_FIELD_BYTES {
        // Find a UTF-8 char boundary at or before the cap.
        let mut cut = MAX_FIELD_BYTES;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
    s
}
