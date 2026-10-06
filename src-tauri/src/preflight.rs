//! CSV import pre-flight: everything that looks at a user-picked file before
//! committing to an import. Two passes, both read-only:
//!
//! 1. [`inspect_csv`] — byte-level, runs on file pick. Settles the text
//!    encoding (UTF-8, or a legacy single-byte encoding the user confirms, or
//!    a mixed file we refuse) and collects structurally broken rows.
//! 2. [`validate_import`] — decoded full-file dry run with the column mapping
//!    applied: the preview rows, what import will keep/skip, and per-column
//!    value distributions.
//!
//! It also owns the shared input guards (file size, field size, column count)
//! and [`open_csv`], the one CSV reader constructor every importer path uses,
//! so the encoding choice is applied identically everywhere. Paths are hostile
//! input per the CLAUDE.md security baseline and are only ever opened here.
//!
//! Row numbers in every report are spreadsheet rows (header = row 1), not the
//! csv crate's line numbers, which lag by one on CRLF files. Blank lines are
//! skipped by the parser, so a file with blank lines drifts from its physical
//! line count — the spreadsheet row is still what Excel shows.

use std::{
    fs::File,
    io::{BufReader, Read},
    path::Path,
};

use encoding_rs::{Encoding, MACINTOSH, WINDOWS_1252};
use encoding_rs_io::DecodeReaderBytesBuilder;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::profile::{InputProfile, InputProfiler, MappedColumns};

/// 1 GiB hard cap. Real working CSVs top out around 200 MB; rejecting anything
/// bigger keeps a malformed multi-GB file from stalling a pass indefinitely.
pub(crate) const MAX_FILE_BYTES: u64 = 1024 * 1024 * 1024;
/// 8 KiB per field — long enough for any course description, short enough to
/// prevent a single hostile cell from ballooning memory.
pub(crate) const MAX_FIELD_BYTES: usize = 8 * 1024;
/// 256 columns — the panel CSV has 13; "real" CSVs rarely exceed a few dozen.
pub(crate) const MAX_COLUMNS: usize = 256;
/// Rows shown in the preview table.
const PREVIEW_ROWS: usize = 5;
/// Examples kept per reported issue; the rest are only counted.
const SAMPLE_LIMIT: usize = 10;
/// Characters of a cell shown in an issue sample.
const DISPLAY_CHARS: usize = 200;

const UTF16_MESSAGE: &str =
    "this file is UTF-16 encoded. Re-save it as \"CSV UTF-8\" and choose it again.";

/// Text encoding of a source CSV. `Utf8` covers files with or without a BOM;
/// the legacy variants are the two single-byte codepages Excel writes for
/// plain "CSV": Windows-1252 on Windows, Mac Roman on macOS. Both are ASCII
/// supersets, so CSV structure (delimiters, quotes, newlines) parses
/// identically under every variant.
#[derive(Type, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum TextEncoding {
    Utf8,
    Windows1252,
    MacRoman,
}

impl TextEncoding {
    /// WHATWG label, persisted on `source_files.encoding`.
    pub(crate) fn label(self) -> &'static str {
        self.legacy().map_or("utf-8", Encoding::name)
    }

    fn legacy(self) -> Option<&'static Encoding> {
        match self {
            Self::Utf8 => None,
            Self::Windows1252 => Some(WINDOWS_1252),
            Self::MacRoman => Some(MACINTOSH),
        }
    }
}

/// Stat the file and enforce [`MAX_FILE_BYTES`]. Returns the size in bytes.
pub(crate) fn stat_source(path: &Path) -> Result<u64, String> {
    let shown = path.display();
    let metadata = std::fs::metadata(path).map_err(|e| format!("stat {shown}: {e}"))?;
    if !metadata.is_file() {
        return Err(format!("{shown}: not a regular file"));
    }
    let size_bytes = metadata.len();
    if size_bytes > MAX_FILE_BYTES {
        return Err(format!(
            "{shown}: {size_bytes} bytes exceeds {MAX_FILE_BYTES}-byte cap"
        ));
    }
    Ok(size_bytes)
}

/// The one CSV reader constructor for decoded (`StringRecord`) reads. `Utf8`
/// reads the file as-is — strict, so an invalid byte is still an error, never
/// a silent U+FFFD — and the csv crate strips a UTF-8 BOM. Legacy encodings
/// are transcoded to UTF-8 before parsing; they map every byte, so decoding
/// cannot fail.
pub(crate) fn open_csv(
    path: &Path,
    encoding: TextEncoding,
) -> Result<csv::Reader<Box<dyn Read + Send>>, String> {
    let file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let source: Box<dyn Read + Send> = match encoding.legacy() {
        None => Box::new(file),
        Some(enc) => Box::new(
            DecodeReaderBytesBuilder::new()
                .encoding(Some(enc))
                .build(file),
        ),
    };
    Ok(csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(source))
}

/// Truncate a cell to [`MAX_FIELD_BYTES`] at a char boundary.
pub(crate) fn truncate(s: String) -> String {
    truncate_to(s, MAX_FIELD_BYTES)
}

pub(crate) fn truncate_to(mut s: String, max_bytes: usize) -> String {
    if s.len() > max_bytes {
        let mut cut = max_bytes;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
    s
}

pub(crate) fn display(s: &str) -> String {
    s.chars().take(DISPLAY_CHARS).collect()
}

/// Spreadsheet row of the `index`-th data record (0-based): the header is
/// row 1, so data starts at row 2.
pub(crate) fn spreadsheet_row(index: u64) -> u64 {
    index + 2
}

/// A count of occurrences plus the first few examples.
#[derive(Type, Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Samples<T> {
    pub count: u64,
    pub first: Vec<T>,
}

impl<T> Samples<T> {
    pub(crate) fn new() -> Self {
        Self {
            count: 0,
            first: Vec::new(),
        }
    }

    /// Count one occurrence; build the example only while under the limit.
    pub(crate) fn push(&mut self, example: impl FnOnce() -> T) {
        self.count += 1;
        if self.first.len() < SAMPLE_LIMIT {
            self.first.push(example());
        }
    }
}

// ---------------------------------------------------------------------------
// Column mapping
// ---------------------------------------------------------------------------

/// Header aliases per logical field. Match is case-insensitive, exact equality
/// — no fuzzy / contains matching, so a column named `subject_xyz` is *not* a
/// `subject` match. A column-mapping UI will replace this with explicit picks.
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

/// Indexes of the mapped columns in the CSV's header order. Persisted in
/// `datasets.layout` (`layout.rs`) so export can reconstruct the original
/// row layout (mapped cells live in the structured `courses` columns,
/// everything else in `extra_columns`). Indexes, not header names: CSVs may
/// repeat a header name, and indexes stay unambiguous.
#[derive(Type, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ColumnMap {
    pub subject: usize,
    pub catalog: usize,
    pub title: usize,
}

pub(crate) fn detect_mapping(headers: &[String]) -> Result<ColumnMap, String> {
    let lc: Vec<String> = headers.iter().map(|h| h.to_ascii_lowercase()).collect();
    let find = |aliases: &[&str]| -> Option<usize> {
        aliases
            .iter()
            .find_map(|alias| lc.iter().position(|h| h == alias))
    };
    match (
        find(SUBJECT_ALIASES),
        find(CATALOG_ALIASES),
        find(TITLE_ALIASES),
    ) {
        (Some(subject), Some(catalog), Some(title)) => Ok(ColumnMap {
            subject,
            catalog,
            title,
        }),
        _ => Err(format!(
            "could not auto-detect required columns. \
             Found headers: {headers:?}. Need one each of: \
             subject={SUBJECT_ALIASES:?}, catalog={CATALOG_ALIASES:?}, title={TITLE_ALIASES:?}"
        )),
    }
}

/// A mapping is only usable against a file whose header has every mapped
/// index, with the three fields on distinct columns. Mappings arrive from the
/// frontend and from stored JSON, so both paths check here.
pub(crate) fn check_mapping(mapping: ColumnMap, header_len: usize) -> Result<(), String> {
    let ColumnMap {
        subject,
        catalog,
        title,
    } = mapping;
    if subject >= header_len || catalog >= header_len || title >= header_len {
        return Err(format!(
            "column mapping index out of bounds for a {header_len}-column header"
        ));
    }
    if subject == catalog || subject == title || catalog == title {
        return Err("column mapping must use three distinct columns".to_owned());
    }
    Ok(())
}

/// The mapped cells of one record, trimmed. The import loop and the dry run
/// share this so "importable" means the same thing in both.
pub(crate) fn mapped_cells(record: &csv::StringRecord, mapping: ColumnMap) -> [&str; 3] {
    let cell = |i: usize| record.get(i).map(str::trim).unwrap_or_default();
    [
        cell(mapping.subject),
        cell(mapping.catalog),
        cell(mapping.title),
    ]
}

// ---------------------------------------------------------------------------
// Pass 1: inspect (byte-level)
// ---------------------------------------------------------------------------

#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Inspection {
    pub size_bytes: u64,
    /// Data records, excluding the header.
    pub rows: u64,
    pub header_fields: u64,
    pub encoding: EncodingReport,
    /// Records whose field count differs from the header's. Import requires
    /// zero.
    pub ragged_rows: Samples<RaggedRow>,
}

/// Verdict of the encoding scan. `Legacy` means some fields aren't UTF-8 and
/// no field contains valid non-ASCII UTF-8 — consistent with a single-byte
/// codepage, which the user must pick (the bytes can't tell Windows-1252 from
/// Mac Roman). `Mixed` means both kinds appear, so no single encoding reads
/// the whole file correctly.
#[derive(Type, Serialize, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum EncodingReport {
    Utf8,
    Legacy {
        invalid: Samples<InvalidField>,
    },
    Mixed {
        invalid: Samples<InvalidField>,
        #[serde(rename = "utf8NonAsciiFields")]
        utf8_non_ascii_fields: u64,
    },
}

/// A field that isn't valid UTF-8, rendered under each legacy candidate so
/// the user can pick the one that reads correctly.
#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InvalidField {
    pub row: u64,
    pub column: String,
    pub windows1252: String,
    pub mac_roman: String,
}

#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RaggedRow {
    pub row: u64,
    pub fields: u64,
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn inspect_csv(path: String) -> Result<Inspection, String> {
    tauri::async_runtime::spawn_blocking(move || inspect(Path::new(&path)))
        .await
        .map_err(|e| format!("inspect task panicked: {e}"))?
}

struct EncodingScan<'h> {
    headers: &'h [String],
    invalid: Samples<InvalidField>,
    utf8_non_ascii: u64,
}

impl EncodingScan<'_> {
    fn field(&mut self, row: u64, index: usize, bytes: &[u8]) {
        if bytes.is_ascii() {
            return;
        }
        if std::str::from_utf8(bytes).is_ok() {
            self.utf8_non_ascii += 1;
            return;
        }
        let headers = self.headers;
        self.invalid.push(|| InvalidField {
            row,
            column: headers
                .get(index)
                .cloned()
                .unwrap_or_else(|| format!("column {}", index + 1)),
            windows1252: display(&WINDOWS_1252.decode_without_bom_handling(bytes).0),
            mac_roman: display(&MACINTOSH.decode_without_bom_handling(bytes).0),
        });
    }

    fn verdict(self) -> EncodingReport {
        match (self.invalid.count, self.utf8_non_ascii) {
            (0, _) => EncodingReport::Utf8,
            (_, 0) => EncodingReport::Legacy {
                invalid: self.invalid,
            },
            (_, utf8_non_ascii_fields) => EncodingReport::Mixed {
                invalid: self.invalid,
                utf8_non_ascii_fields,
            },
        }
    }
}

pub(crate) fn inspect(path: &Path) -> Result<Inspection, String> {
    let size_bytes = stat_source(path)?;
    let file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let mut reader = BufReader::new(file);

    // UTF-16 BOM sniff. A UTF-8 BOM needs no handling: the csv crate strips it.
    let mut bom = [0_u8; 2];
    let sniffed = reader
        .read(&mut bom)
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    if sniffed == 2 && matches!(bom, [0xFF, 0xFE] | [0xFE, 0xFF]) {
        return Err(UTF16_MESSAGE.to_owned());
    }
    let mut csv_reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(bom.get(..sniffed).unwrap_or(&[]).chain(reader));

    let header_record = csv_reader
        .byte_headers()
        .map_err(|e| format!("read headers: {e}"))?
        .clone();
    // BOM-less UTF-16 is valid UTF-8 byte-wise (NULs and ASCII); the NULs
    // interleaved through the header give it away.
    if header_record.as_slice().contains(&0) {
        return Err(UTF16_MESSAGE.to_owned());
    }
    if header_record.len() > MAX_COLUMNS {
        return Err(format!(
            "{} columns exceeds {MAX_COLUMNS}-column cap",
            header_record.len()
        ));
    }
    let headers: Vec<String> = header_record
        .iter()
        .map(|h| display(&String::from_utf8_lossy(h)))
        .collect();

    let mut scan = EncodingScan {
        headers: &headers,
        invalid: Samples::new(),
        utf8_non_ascii: 0,
    };
    for (i, field) in header_record.iter().enumerate() {
        scan.field(1, i, field);
    }

    let mut ragged_rows = Samples::new();
    let mut rows: u64 = 0;
    let mut record = csv::ByteRecord::new();
    while csv_reader
        .read_byte_record(&mut record)
        .map_err(|e| format!("read row {}: {e}", spreadsheet_row(rows)))?
    {
        let row = spreadsheet_row(rows);
        if record.len() != header_record.len() {
            let fields = record.len() as u64;
            ragged_rows.push(|| RaggedRow { row, fields });
        }
        for (i, field) in record.iter().enumerate() {
            scan.field(row, i, field);
        }
        rows += 1;
    }

    Ok(Inspection {
        size_bytes,
        rows,
        header_fields: header_record.len() as u64,
        encoding: scan.verdict(),
        ragged_rows,
    })
}

// ---------------------------------------------------------------------------
// Pass 2: validate (decoded dry run)
// ---------------------------------------------------------------------------

#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Validation {
    pub headers: Vec<String>,
    pub sample_rows: Vec<Vec<String>>,
    /// Auto-detected from header aliases. `import_csv` takes it back verbatim.
    pub mapping: ColumnMap,
    /// Data records, excluding the header.
    pub rows: u64,
    /// Records with subject, catalog, and title all non-empty — what import
    /// will ingest.
    pub importable: u64,
    pub skipped: Samples<SkippedRow>,
    /// Cells longer than the per-field cap; import truncates them.
    pub truncated_fields: u64,
    /// `profile.columns`, kept here for the dialog's column grid.
    pub columns: MappedColumns,
    /// Field shapes, checks, findings, and sample model inputs.
    pub profile: InputProfile,
}

#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SkippedRow {
    pub row: u64,
    /// Headers of the required columns that were empty.
    pub missing: Vec<String>,
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn validate_import(
    path: String,
    encoding: TextEncoding,
) -> Result<Validation, String> {
    tauri::async_runtime::spawn_blocking(move || validate(Path::new(&path), encoding))
        .await
        .map_err(|e| format!("validate task panicked: {e}"))?
}

pub(crate) fn validate(path: &Path, encoding: TextEncoding) -> Result<Validation, String> {
    stat_source(path)?;
    let mut reader = open_csv(path, encoding)?;
    let headers: Vec<String> = reader
        .headers()
        .map_err(|e| format!("read headers: {e}"))?
        .iter()
        .map(|h| truncate(h.to_owned()))
        .collect();
    if headers.len() > MAX_COLUMNS {
        return Err(format!(
            "{} columns exceeds {MAX_COLUMNS}-column cap",
            headers.len()
        ));
    }
    let mapping = detect_mapping(&headers)?;
    let header = |i: usize| headers.get(i).cloned().unwrap_or_default();
    let mapped_headers = [
        header(mapping.subject),
        header(mapping.catalog),
        header(mapping.title),
    ];
    let mut profiler = InputProfiler::new(mapped_headers.clone());

    let mut sample_rows = Vec::with_capacity(PREVIEW_ROWS);
    let mut rows: u64 = 0;
    let mut skipped = Samples::new();
    let mut truncated_fields: u64 = 0;
    let mut record = csv::StringRecord::new();
    while reader
        .read_record(&mut record)
        .map_err(|e| format!("read row {}: {e}", spreadsheet_row(rows)))?
    {
        let row = spreadsheet_row(rows);
        rows += 1;
        if sample_rows.len() < PREVIEW_ROWS {
            sample_rows.push(record.iter().map(|f| truncate(f.to_owned())).collect());
        }
        truncated_fields += record.iter().filter(|f| f.len() > MAX_FIELD_BYTES).count() as u64;

        let cells = mapped_cells(&record, mapping);
        profiler.observe(row, cells);
        if cells.iter().any(|c| c.is_empty()) {
            skipped.push(|| SkippedRow {
                row,
                missing: mapped_headers
                    .iter()
                    .zip(cells)
                    .filter(|(_, c)| c.is_empty())
                    .map(|(h, _)| h.clone())
                    .collect(),
            });
        }
    }

    let profile = profiler.finish();
    Ok(Validation {
        headers,
        sample_rows,
        mapping,
        rows,
        importable: profile.importable,
        skipped,
        truncated_fields,
        columns: profile.columns.clone(),
        profile,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        ColumnMap, EncodingReport, Inspection, TextEncoding, check_mapping, inspect, mapped_cells,
        open_csv, validate,
    };
    use crate::{
        format::{CourseInput, content_hash},
        profile::FindingCode,
    };

    const HEADER: &[u8] = b"sub_pref,course,course_title\r\n";

    fn fixture(name: &str, body: &[u8]) -> Result<PathBuf, String> {
        let dir = std::env::temp_dir().join(format!("ccm-preflight-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(name);
        std::fs::write(&path, body).map_err(|e| e.to_string())?;
        Ok(path)
    }

    fn csv(rows: &[&[u8]]) -> Vec<u8> {
        let mut out = HEADER.to_vec();
        for row in rows {
            out.extend_from_slice(row);
            out.extend_from_slice(b"\r\n");
        }
        out
    }

    fn inspect_fixture(name: &str, body: &[u8]) -> Result<Inspection, String> {
        inspect(&fixture(name, body)?)
    }

    /// Hash each record's mapped cells the way import does.
    fn hashes(name: &str, body: &[u8], encoding: TextEncoding) -> Result<Vec<String>, String> {
        let mut reader = open_csv(&fixture(name, body)?, encoding)?;
        let mapping = ColumnMap {
            subject: 0,
            catalog: 1,
            title: 2,
        };
        reader
            .records()
            .map(|r| {
                let record = r.map_err(|e| e.to_string())?;
                let [subject, catalog, title] = mapped_cells(&record, mapping);
                Ok(content_hash(&CourseInput {
                    subject_code: subject.to_owned(),
                    catalog_number: catalog.to_owned(),
                    course_title: title.to_owned(),
                }))
            })
            .collect()
    }

    #[test]
    fn utf8_with_and_without_bom() -> Result<(), String> {
        let plain = csv(&[b"ECON,101,Caf\xC3\xA9 Economics"]);
        let got = inspect_fixture("utf8.csv", &plain)?;
        assert!(matches!(got.encoding, EncodingReport::Utf8), "{got:?}");
        assert_eq!(got.rows, 1);

        let mut bom = b"\xEF\xBB\xBF".to_vec();
        bom.extend_from_slice(&plain);
        let path = fixture("utf8_bom.csv", &bom)?;
        let got = inspect(&path)?;
        assert!(matches!(got.encoding, EncodingReport::Utf8), "{got:?}");
        // The BOM never reaches the first header, so alias detection works.
        let validation = validate(&path, TextEncoding::Utf8)?;
        assert_eq!(
            validation.headers.first().map(String::as_str),
            Some("sub_pref")
        );
        Ok(())
    }

    #[test]
    fn legacy_bytes_render_under_both_candidates_with_spreadsheet_rows() -> Result<(), String> {
        // Mirrors the #178 sample: Mac Roman 0xA8 is the registered sign, on
        // the third data row of a CRLF file (spreadsheet row 4).
        let body = csv(&[
            b"ECON,101,Micro",
            b"ECON,102,Macro",
            b"HMGT,6330,\"HLTHCARE LAW, POLICY \xA8ULATN\"",
        ]);
        let got = inspect_fixture("macroman.csv", &body)?;
        let EncodingReport::Legacy { invalid } = got.encoding else {
            return Err(format!("expected legacy, got {:?}", got.encoding));
        };
        assert_eq!(invalid.count, 1);
        let field = invalid.first.first().ok_or("no sample")?;
        assert_eq!(field.row, 4);
        assert_eq!(field.column, "course_title");
        assert_eq!(field.mac_roman, "HLTHCARE LAW, POLICY \u{AE}ULATN");
        assert_eq!(field.windows1252, "HLTHCARE LAW, POLICY \u{A8}ULATN");
        Ok(())
    }

    #[test]
    fn valid_and_invalid_non_ascii_is_mixed() -> Result<(), String> {
        let body = csv(&[b"LAW,1,Policy \xC2\xAE", b"LAW,2,Policy \xAE"]);
        let got = inspect_fixture("mixed.csv", &body)?;
        assert!(
            matches!(
                got.encoding,
                EncodingReport::Mixed {
                    utf8_non_ascii_fields: 1,
                    ..
                }
            ),
            "{got:?}"
        );
        Ok(())
    }

    #[test]
    fn utf16_is_rejected_with_or_without_bom() -> Result<(), String> {
        let utf16le: Vec<u8> = "sub_pref,course\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let mut with_bom = vec![0xFF, 0xFE];
        with_bom.extend_from_slice(&utf16le);
        for (name, body) in [("utf16_bom.csv", &with_bom), ("utf16.csv", &utf16le)] {
            let err = inspect_fixture(name, body)
                .err()
                .ok_or_else(|| format!("{name}: expected rejection"))?;
            assert!(err.contains("UTF-16"), "{name}: {err}");
        }
        Ok(())
    }

    #[test]
    fn ragged_rows_are_collected_not_fatal() -> Result<(), String> {
        let body = csv(&[b"ECON,101,Micro", b"ECON,102", b"ECON,103,Macro,extra"]);
        let got = inspect_fixture("ragged.csv", &body)?;
        assert_eq!(got.rows, 3);
        assert_eq!(got.ragged_rows.count, 2);
        let rows: Vec<(u64, u64)> = got
            .ragged_rows
            .first
            .iter()
            .map(|r| (r.row, r.fields))
            .collect();
        assert_eq!(rows, vec![(3, 2), (4, 4)]);
        Ok(())
    }

    /// A legacy file decoded with the right encoding hashes identically to
    /// the same text re-saved as UTF-8, so cached classifications carry over.
    #[test]
    fn legacy_decode_matches_utf8_resave_hashes() -> Result<(), String> {
        let cp1252 = csv(&[
            b"LAW,1,Policy \xAE Regulation",
            b"SPAN,2,Don Quijote \x97 Part I",
        ]);
        let utf8 = csv(&[
            "LAW,1,Policy \u{AE} Regulation".as_bytes(),
            "SPAN,2,Don Quijote \u{2014} Part I".as_bytes(),
        ]);
        let legacy = hashes("cp1252.csv", &cp1252, TextEncoding::Windows1252)?;
        let resaved = hashes("cp1252_resaved.csv", &utf8, TextEncoding::Utf8)?;
        assert_eq!(legacy.len(), 2);
        assert_eq!(legacy, resaved);

        // Strict UTF-8 still refuses the legacy bytes.
        assert!(hashes("cp1252_as_utf8.csv", &cp1252, TextEncoding::Utf8).is_err());
        Ok(())
    }

    #[test]
    fn validate_counts_skipped_rows_and_distributions() -> Result<(), String> {
        let body = csv(&[
            b"ECON,101,Micro",
            b"ECON,102,Macro",
            b"MATH,101,  ",
            b",200,Orphan",
            b"ECON,101,Micro",
        ]);
        let got = validate(&fixture("validate.csv", &body)?, TextEncoding::Utf8)?;
        assert_eq!(got.rows, 5);
        assert_eq!(got.importable, 3);
        assert_eq!(got.skipped.count, 2);
        let skipped: Vec<(u64, Vec<String>)> = got
            .skipped
            .first
            .iter()
            .map(|s| (s.row, s.missing.clone()))
            .collect();
        assert_eq!(
            skipped,
            vec![
                (4, vec!["course_title".to_owned()]),
                (5, vec!["sub_pref".to_owned()]),
            ]
        );
        assert_eq!(got.sample_rows.len(), 5);

        let subject = &got.columns.subject;
        assert_eq!((subject.empty, subject.distinct), (1, 2));
        let top: Vec<(&str, u64)> = subject
            .top
            .iter()
            .map(|v| (v.value.as_str(), v.count))
            .collect();
        assert_eq!(top, vec![("ECON", 3), ("MATH", 1)]);
        assert!(!subject.distinct_capped);
        assert_eq!(got.columns.title.empty, 1);
        Ok(())
    }

    /// A catalog column that already carries the subject is the mapping
    /// mistake the profiler exists for: the dry run reports it with the
    /// doubled model input before Import is clicked.
    #[test]
    fn validate_profiles_combined_catalog_column() -> Result<(), String> {
        let body = csv(&[
            b"PSYC,PSYC 4325,ABNORMAL PSYCHOLOGY",
            b"PSYC,PSYC 2301,GENERAL PSYCHOLOGY",
            b"ECON,2301,PRINCIPLES OF MICRO",
            b"ECON,,ORPHAN",
        ]);
        let got = validate(&fixture("combined.csv", &body)?, TextEncoding::Utf8)?;
        let profile = &got.profile;
        assert_eq!((profile.rows, profile.importable), (4, 3));
        assert_eq!(profile.skipped.catalog, 1);
        assert_eq!(profile.columns.catalog.header, got.columns.catalog.header);
        let finding = profile
            .findings
            .iter()
            .find(|f| f.code == FindingCode::CatalogHasSubjectPrefix)
            .ok_or("no prefix finding")?;
        assert_eq!(finding.count, 2);
        assert_eq!(
            finding
                .examples
                .first
                .first()
                .map(|s| (s.row, s.input.as_str())),
            Some((2, "PSYC PSYC 4325 --- ABNORMAL PSYCHOLOGY"))
        );
        assert_eq!(profile.samples.len(), 3);
        Ok(())
    }

    #[test]
    fn mapping_must_be_in_bounds_and_distinct() {
        let map = |subject, catalog, title| ColumnMap {
            subject,
            catalog,
            title,
        };
        assert!(check_mapping(map(0, 1, 2), 3).is_ok());
        assert!(check_mapping(map(0, 1, 3), 3).is_err());
        assert!(check_mapping(map(0, 0, 2), 3).is_err());
    }
}
