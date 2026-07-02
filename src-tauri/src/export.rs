//! CSV export of classification results (EPI-15) with formula-injection
//! escaping (EPI-16).
//!
//! The export never ships rows across IPC: `DuckDB` streams straight to disk
//! via `COPY (SELECT ...) TO '<path>' (FORMAT CSV, HEADER)`. The save path
//! comes from a native save dialog opened *in Rust* — the frontend never
//! supplies a path string, so there's nothing to distrust at the boundary.

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::Manager as _;
use tauri_plugin_dialog::DialogExt as _;

use crate::db::AppDb;

#[derive(Type, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportRequest {
    pub dataset_id: String,
    /// Model whose classifications populate the classification/probability
    /// columns. Rows without a cached result export with those cells empty.
    pub model_id: i64,
}

#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportOutcome {
    pub path: String,
    pub rows: i64,
}

/// Text columns exported from `courses`, in output order. Hardcoded allowlist
/// per the security baseline — identifiers never come from user input.
const COURSE_TEXT_COLUMNS: &[&str] = &[
    "subject_code",
    "catalog_number",
    "course_title",
    "school_name",
    "school_year_enrolled",
];

/// Wrap a column expression in OWASP CSV-injection prefix-escaping: any cell
/// starting with `=`, `+`, `-`, `@`, tab, or CR gets a leading apostrophe so
/// spreadsheet apps render it as text instead of evaluating a formula.
/// `\t`/`\r` are interpreted by the regex engine, not the SQL literal, so the
/// pattern survives `DuckDB`'s non-escaping string syntax; `-` sits last in
/// the class to stay literal.
fn injection_escaped(expr: &str) -> String {
    format!("CASE WHEN regexp_matches({expr}, '^[=+@\\t\\r-]') THEN '''' || {expr} ELSE {expr} END")
}

/// Escape a string for embedding in a single-quoted `DuckDB` literal. `COPY`
/// statements can't take bound parameters, so the dataset id and target path
/// are embedded — with `'` doubled, the literal can't be broken out of.
fn sql_quote(s: &str) -> String {
    s.replace('\'', "''")
}

fn export_sql(dataset_id: &str, model_id: i64, target_path: &str) -> String {
    let course_cols = COURSE_TEXT_COLUMNS
        .iter()
        .map(|col| format!("{} AS {col}", injection_escaped(&format!("c.{col}"))))
        .collect::<Vec<_>>()
        .join(",\n           ");
    format!(
        "COPY (
        SELECT c.row_index,
           {course_cols},
           {classification} AS classification,
           r.probability
        FROM courses c
        LEFT JOIN inference_results r
          ON r.model_id = {model_id} AND r.content_hash = c.content_hash
        WHERE c.dataset_id = '{dataset}'
        ORDER BY c.row_index
    ) TO '{path}' (FORMAT CSV, HEADER)",
        classification = injection_escaped("r.classification"),
        dataset = sql_quote(dataset_id),
        path = sql_quote(target_path),
    )
}

/// Export a dataset's courses (joined with one model's cached classifications)
/// to a CSV the user picks via the native save dialog. Returns `None` when the
/// user cancels the dialog.
#[tauri::command]
#[specta::specta]
pub(crate) async fn export_results(
    req: ExportRequest,
    app: tauri::AppHandle,
) -> Result<Option<ExportOutcome>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let db = app.state::<AppDb>();

        // Default filename from the dataset title + the model's digit level.
        let (title, digit): (String, String) = {
            let conn = db.ro()?;
            let title = conn
                .query_row(
                    "SELECT title FROM datasets WHERE id = ?",
                    [&req.dataset_id],
                    |row| row.get(0),
                )
                .map_err(|e| format!("dataset lookup: {e}"))?;
            let digit = conn
                .query_row(
                    "SELECT model_type FROM models WHERE id = ?",
                    [req.model_id],
                    |row| row.get(0),
                )
                .map_err(|e| format!("model lookup: {e}"))?;
            (title, digit)
        }; // release the read lock before blocking on the dialog

        let slug: String = title
            .chars()
            .map(|ch| if ch.is_alphanumeric() { ch } else { '-' })
            .collect::<String>()
            .to_lowercase();
        let Some(file_path) = app
            .dialog()
            .file()
            .add_filter("CSV", &["csv"])
            .set_file_name(format!("{slug}-{digit}-digit.csv"))
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let target = file_path
            .into_path()
            .map_err(|e| format!("resolve save path: {e}"))?;
        let target_str = target
            .to_str()
            .ok_or_else(|| "save path is not valid UTF-8".to_owned())?
            .to_owned();

        let sql = export_sql(&req.dataset_id, req.model_id, &target_str);
        let rows: i64 = {
            let conn = db.ro()?;
            // COPY ... TO returns a single-row result with the exported count.
            conn.query_row(&sql, [], |row| row.get(0))
                .map_err(|e| format!("export copy: {e}"))?
        };

        Ok(Some(ExportOutcome {
            path: target_str,
            rows,
        }))
    })
    .await
    .map_err(|e| format!("export task panicked: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::{export_sql, injection_escaped};

    /// EPI-16: known-malicious payloads get the apostrophe prefix; benign
    /// values pass through byte-identical. Runs the real `COPY` path against
    /// an in-memory `DuckDB` so the SQL fragment is what's under test.
    #[test]
    fn formula_injection_is_prefix_escaped() -> Result<(), String> {
        let conn = duckdb::Connection::open_in_memory().map_err(|e| e.to_string())?;
        conn.execute_batch("CREATE TABLE t (v VARCHAR)")
            .map_err(|e| e.to_string())?;

        let cases: &[(&str, &str)] = &[
            ("=SUM(A1:A9)", "'=SUM(A1:A9)"),
            ("+1234", "'+1234"),
            ("-cmd|' /C calc'!A0", "'-cmd|' /C calc'!A0"),
            ("@evil()", "'@evil()"),
            ("\tstart-with-tab", "'\tstart-with-tab"),
            ("\rstart-with-cr", "'\rstart-with-cr"),
            ("ECON 101", "ECON 101"),
            ("Introduction to = signs", "Introduction to = signs"),
        ];
        for (payload, _) in cases {
            conn.execute("INSERT INTO t VALUES (?)", [payload])
                .map_err(|e| e.to_string())?;
        }

        let escaped = injection_escaped("v");
        for (payload, expected) in cases {
            let got: String = conn
                .query_row(
                    &format!("SELECT {escaped} FROM t WHERE v = ?"),
                    [payload],
                    |row| row.get(0),
                )
                .map_err(|e| e.to_string())?;
            assert_eq!(&got, expected, "payload {payload:?}");
        }
        Ok(())
    }

    /// The full statement embeds ids/paths as quoted literals — a quote in
    /// either can't terminate the literal.
    #[test]
    fn sql_literals_cannot_be_broken_out_of() {
        let sql = export_sql("id'--", 3, "/tmp/o'brien.csv");
        assert!(sql.contains("WHERE c.dataset_id = 'id''--'"));
        assert!(sql.contains("TO '/tmp/o''brien.csv'"));
    }

    /// Drive the complete generated statement — `COPY` to a real file from
    /// minimal `courses`/`inference_results` shapes — and confirm the count
    /// row `query_row` relies on, the left-join semantics, and the header.
    #[test]
    fn copy_statement_exports_joined_rows() -> Result<(), String> {
        let conn = duckdb::Connection::open_in_memory().map_err(|e| e.to_string())?;
        conn.execute_batch(
            "CREATE TABLE courses (
                 dataset_id VARCHAR, row_index BIGINT, subject_code VARCHAR,
                 catalog_number VARCHAR, course_title VARCHAR, school_name VARCHAR,
                 school_year_enrolled VARCHAR, content_hash VARCHAR);
             CREATE TABLE inference_results (
                 model_id BIGINT, content_hash VARCHAR,
                 classification VARCHAR, probability REAL);
             INSERT INTO courses VALUES
                 ('ds', 0, 'ECON', '101', '=HYPERLINK(\"x\")', 'A', '2024', 'h1'),
                 ('ds', 1, 'MATH', '201', 'Calculus', 'A', '2024', 'h2'),
                 ('other', 0, 'BIO', '1', 'Not exported', 'B', '2024', 'h3');
             INSERT INTO inference_results VALUES (7, 'h1', '45.06', 0.99);",
        )
        .map_err(|e| e.to_string())?;

        let out = std::env::temp_dir().join(format!("export-test-{}.csv", std::process::id()));
        let out_str = out
            .to_str()
            .ok_or_else(|| "temp path not UTF-8".to_owned())?;
        let rows: i64 = conn
            .query_row(&export_sql("ds", 7, out_str), [], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        let contents = std::fs::read_to_string(&out).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(&out);

        assert_eq!(rows, 2);
        let mut lines = contents.lines();
        assert_eq!(
            lines.next(),
            Some(
                "row_index,subject_code,catalog_number,course_title,school_name,\
                 school_year_enrolled,classification,probability"
            )
        );
        // Formula title escaped; classified row carries its cached result.
        assert_eq!(
            lines.next(),
            Some("0,ECON,101,\"'=HYPERLINK(\"\"x\"\")\",A,2024,45.06,0.99")
        );
        // Unclassified row left-joins to empty classification cells.
        assert_eq!(lines.next(), Some("1,MATH,201,Calculus,A,2024,,"));
        assert_eq!(lines.next(), None);
        Ok(())
    }
}
