//! CSV export of classification results (EPI-15) with formula-injection
//! escaping (EPI-16), round-trip original-column output (EPI-79), CCM-named
//! model columns + taxonomy titles (EPI-77/EPI-81), and optional top-5
//! candidate columns (EPI-98).
//!
//! The export never ships rows across IPC: `DuckDB` streams straight to disk
//! via `COPY (SELECT ...) TO '<path>' (FORMAT CSV, HEADER)`. The save path
//! comes from a native save dialog opened *in Rust* — the frontend never
//! supplies a path string, so there's nothing to distrust at the boundary.
//!
//! Output shape (stakeholder meeting 2026-07-28): the original CSV's columns
//! in their original order — so the export can be merged back onto the source
//! file — with the model columns appended: `ccm{2|4|6}digit_code`,
//! `ccm…_prob`, `ccm…_title`, and (when requested) the numbered rank 1–5
//! candidate columns, where rank 1 duplicates the top-1 columns. Datasets
//! imported before migration 0004 (and derived/seeded datasets) have no
//! stored header layout and keep the legacy fixed-column shape.

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::Manager as _;
use tauri_plugin_dialog::DialogExt as _;

use crate::{db::AppDb, import::ColumnMap};

#[derive(Type, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportRequest {
    pub dataset_id: String,
    /// Model whose classifications populate the `ccm*` columns. Rows without
    /// a cached result export with those cells empty.
    pub model_id: i64,
    /// Emit the numbered top-5 candidate columns (`ccm…_code1` …
    /// `ccm…_title5`). Export-dialog toggle (EPI-98).
    pub include_top_candidates: bool,
}

#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportOutcome {
    pub path: String,
    pub rows: i64,
}

/// Text columns exported from `courses` in the legacy layout, in output
/// order. Hardcoded allowlist per the security baseline — identifiers never
/// come from user input.
const COURSE_TEXT_COLUMNS: &[&str] = &[
    "subject_code",
    "catalog_number",
    "course_title",
    "school_name",
    "school_year_enrolled",
];

/// How the source-data half of the exported row is shaped.
enum RowLayout {
    /// Round-trip shape (EPI-79): every original CSV column under its
    /// original header, in file order. Mapped cells come from the structured
    /// `courses` columns, everything else from `extra_columns`.
    Original {
        headers: Vec<String>,
        mapping: ColumnMap,
    },
    /// Pre-0004 imports and derived/seeded datasets: no stored header layout,
    /// keep the fixed allowlist shape.
    Legacy,
}

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

/// Quote an untrusted string as a `DuckDB` identifier (output column alias).
/// Original CSV headers are user input in identifier position — doubling `"`
/// inside a double-quoted identifier means the name can't terminate the
/// quoting, whatever it contains.
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// SELECT expressions for the source-data half of the row, per layout.
fn source_column_exprs(layout: &RowLayout) -> Vec<String> {
    match layout {
        RowLayout::Original { headers, mapping } => headers
            .iter()
            .enumerate()
            .map(|(i, header)| {
                let base = if i == mapping.subject {
                    "c.subject_code".to_owned()
                } else if i == mapping.catalog {
                    "c.catalog_number".to_owned()
                } else if i == mapping.title {
                    "c.course_title".to_owned()
                } else {
                    // Keys are indexes we generated at import; the quoted-key
                    // path form addresses object keys (not array positions).
                    format!("json_extract_string(c.extra_columns, '$.\"{i}\"')")
                };
                format!("{} AS {}", injection_escaped(&base), quote_ident(header))
            })
            .collect(),
        RowLayout::Legacy => {
            let mut cols = vec!["c.row_index".to_owned()];
            cols.extend(
                COURSE_TEXT_COLUMNS
                    .iter()
                    .map(|col| format!("{} AS {col}", injection_escaped(&format!("c.{col}")))),
            );
            cols
        }
    }
}

/// SELECT expressions for the appended model columns: the top-1 trio, then —
/// when requested — the numbered rank 1–5 candidate columns (rank 1
/// duplicates the top-1 values by stakeholder decision). Taxonomy titles use
/// the same exact-level-else-2-digit-parent fallback as the results view
/// (`courses.rs::attach_results`); `t{k}`/`p{k}` are the per-rank taxonomy
/// join aliases emitted by [`taxonomy_joins`].
fn model_column_exprs(digit_level: u8, include_top: bool) -> Vec<String> {
    let prefix = format!("ccm{digit_level}digit");
    let title_expr = |k: u8| format!("COALESCE(t{k}.title, p{k}.title)");
    let code_expr = |k: u8| {
        if k == 1 {
            "r.classification".to_owned()
        } else {
            format!("r.top{k}_code")
        }
    };
    let prob_expr = |k: u8| {
        if k == 1 {
            "r.probability".to_owned()
        } else {
            format!("r.top{k}_prob")
        }
    };

    let mut cols = vec![
        format!("{} AS {prefix}_code", injection_escaped("r.classification")),
        format!("r.probability AS {prefix}_prob"),
        format!("{} AS {prefix}_title", injection_escaped(&title_expr(1))),
    ];
    if include_top {
        for k in 1..=5_u8 {
            cols.push(format!(
                "{} AS {prefix}_code{k}",
                injection_escaped(&code_expr(k))
            ));
            cols.push(format!("{} AS {prefix}_prob{k}", prob_expr(k)));
            cols.push(format!(
                "{} AS {prefix}_title{k}",
                injection_escaped(&title_expr(k))
            ));
        }
    }
    cols
}

/// LEFT JOINs resolving each rank's taxonomy title: exact match at the
/// model's digit level (`t{k}`), 2-digit parent by code prefix (`p{k}`).
/// Rank 1 joins on `classification`; ranks 2–5 (only when the numbered
/// columns are requested) join on their `top{k}_code`.
fn taxonomy_joins(digit_level: u8, include_top: bool) -> String {
    let ranks: &[u8] = if include_top { &[1, 2, 3, 4, 5] } else { &[1] };
    ranks
        .iter()
        .map(|&k| {
            let code = if k == 1 {
                "r.classification".to_owned()
            } else {
                format!("r.top{k}_code")
            };
            format!(
                "
        LEFT JOIN ccm_taxonomy t{k}
          ON t{k}.digit_level = {digit_level} AND t{k}.code = {code}
        LEFT JOIN ccm_taxonomy p{k}
          ON p{k}.digit_level = 2 AND p{k}.code = substr({code}, 1, 2)"
            )
        })
        .collect::<Vec<_>>()
        .concat()
}

fn export_sql(
    dataset_id: &str,
    model_id: i64,
    digit_level: u8,
    layout: &RowLayout,
    include_top: bool,
    target_path: &str,
) -> String {
    let mut cols = source_column_exprs(layout);
    cols.extend(model_column_exprs(digit_level, include_top));
    let col_list = cols.join(",\n           ");
    let joins = taxonomy_joins(digit_level, include_top);
    format!(
        "COPY (
        SELECT {col_list}
        FROM courses c
        LEFT JOIN inference_results r
          ON r.model_id = {model_id} AND r.content_hash = c.content_hash{joins}
        WHERE c.dataset_id = '{dataset}'
        ORDER BY c.row_index
    ) TO '{path}' (FORMAT CSV, HEADER)",
        dataset = sql_quote(dataset_id),
        path = sql_quote(target_path),
    )
}

/// Resolve the export's row layout + default-filename inputs in one read:
/// dataset title, the model's digit level, and — when the dataset's source
/// file stored them (post-0004 imports) — the original header order and
/// column mapping.
fn resolve_export_inputs(
    conn: &duckdb::Connection,
    dataset_id: &str,
    model_id: i64,
) -> Result<(String, u8, RowLayout), String> {
    let (title, headers_json, mapping_json): (String, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT d.title, sf.original_headers, sf.column_mapping
             FROM datasets d
             LEFT JOIN source_files sf ON sf.id = d.source_file_id
             WHERE d.id = ?",
            [dataset_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|e| format!("dataset lookup: {e}"))?;

    let model_type: String = conn
        .query_row(
            "SELECT model_type FROM models WHERE id = ?",
            [model_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("model lookup: {e}"))?;
    let digit_level: u8 = model_type
        .parse()
        .map_err(|e| format!("non-numeric model_type {model_type:?}: {e}"))?;
    if !matches!(digit_level, 2 | 4 | 6) {
        return Err(format!("unsupported model digit level {digit_level}"));
    }

    let layout = match (headers_json, mapping_json) {
        (Some(headers_json), Some(mapping_json)) => {
            let headers: Vec<String> = serde_json::from_str(&headers_json)
                .map_err(|e| format!("parse stored original_headers: {e}"))?;
            let mapping: ColumnMap = serde_json::from_str(&mapping_json)
                .map_err(|e| format!("parse stored column_mapping: {e}"))?;
            let in_bounds = |i: usize| i < headers.len();
            if !(in_bounds(mapping.subject)
                && in_bounds(mapping.catalog)
                && in_bounds(mapping.title))
            {
                return Err("stored column_mapping index out of header bounds".to_owned());
            }
            RowLayout::Original { headers, mapping }
        }
        _ => RowLayout::Legacy,
    };
    Ok((title, digit_level, layout))
}

/// Export a dataset's courses (joined with one model's cached classifications
/// + taxonomy titles) to a CSV the user picks via the native save dialog.
/// Returns `None` when the user cancels the dialog.
#[tauri::command]
#[specta::specta]
pub(crate) async fn export_results(
    req: ExportRequest,
    app: tauri::AppHandle,
) -> Result<Option<ExportOutcome>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let db = app.state::<AppDb>();

        let (title, digit_level, layout) = {
            let conn = db.ro()?;
            resolve_export_inputs(&conn, &req.dataset_id, req.model_id)?
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
            .set_file_name(format!("{slug}-{digit_level}-digit.csv"))
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

        let sql = export_sql(
            &req.dataset_id,
            req.model_id,
            digit_level,
            &layout,
            req.include_top_candidates,
            &target_str,
        );
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
    use super::{ColumnMap, RowLayout, export_sql, injection_escaped, quote_ident};

    /// Minimal schema matching the columns the export SQL touches, plus a
    /// taxonomy slice exercising both the exact-level match and the 2-digit
    /// parent fallback.
    fn scratch_conn() -> Result<duckdb::Connection, String> {
        let conn = duckdb::Connection::open_in_memory().map_err(|e| e.to_string())?;
        conn.execute_batch(
            "CREATE TABLE courses (
                 dataset_id VARCHAR, row_index BIGINT, subject_code VARCHAR,
                 catalog_number VARCHAR, course_title VARCHAR, school_name VARCHAR,
                 school_year_enrolled VARCHAR, extra_columns JSON, content_hash VARCHAR);
             CREATE TABLE inference_results (
                 model_id BIGINT, content_hash VARCHAR,
                 classification VARCHAR, probability REAL,
                 top2_code VARCHAR, top2_prob REAL,
                 top3_code VARCHAR, top3_prob REAL,
                 top4_code VARCHAR, top4_prob REAL,
                 top5_code VARCHAR, top5_prob REAL);
             CREATE TABLE ccm_taxonomy (
                 digit_level TINYINT, code TEXT, title TEXT);
             INSERT INTO ccm_taxonomy VALUES
                 (6, '45.0601', 'Economics, General'),
                 (6, '27.0101', 'Mathematics, General'),
                 (2, '45', 'Social Sciences'),
                 (2, '27', 'Mathematics and Statistics');",
        )
        .map_err(|e| e.to_string())?;
        Ok(conn)
    }

    fn run_copy(
        conn: &duckdb::Connection,
        sql_builder: impl Fn(&str) -> String,
    ) -> Result<(i64, String), String> {
        let out = std::env::temp_dir().join(format!(
            "export-test-{}-{:p}.csv",
            std::process::id(),
            &conn
        ));
        let out_str = out
            .to_str()
            .ok_or_else(|| "temp path not UTF-8".to_owned())?;
        let rows: i64 = conn
            .query_row(&sql_builder(out_str), [], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        let contents = std::fs::read_to_string(&out).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(&out);
        Ok((rows, contents))
    }

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
    /// either can't terminate the literal. Untrusted original headers are
    /// double-quote-escaped in identifier position.
    #[test]
    fn sql_literals_and_identifiers_cannot_be_broken_out_of() {
        let sql = export_sql("id'--", 3, 6, &RowLayout::Legacy, false, "/tmp/o'brien.csv");
        assert!(sql.contains("WHERE c.dataset_id = 'id''--'"));
        assert!(sql.contains("TO '/tmp/o''brien.csv'"));

        assert_eq!(quote_ident("plain"), "\"plain\"");
        assert_eq!(
            quote_ident("evil\" FROM courses; --"),
            "\"evil\"\" FROM courses; --\""
        );
    }

    /// Legacy layout (no stored headers): fixed allowlist columns, model
    /// columns renamed to the ccm scheme (EPI-77) with the taxonomy title
    /// appended (EPI-81). Unclassified rows left-join to empty cells.
    #[test]
    fn legacy_copy_exports_ccm_named_columns() -> Result<(), String> {
        let conn = scratch_conn()?;
        conn.execute_batch(
            "INSERT INTO courses VALUES
                 ('ds', 0, 'ECON', '101', '=HYPERLINK(\"x\")', 'A', '2024', NULL, 'h1'),
                 ('ds', 1, 'MATH', '201', 'Calculus', 'A', '2024', NULL, 'h2'),
                 ('other', 0, 'BIO', '1', 'Not exported', 'B', '2024', NULL, 'h3');
             INSERT INTO inference_results VALUES
                 (7, 'h1', '45.0601', 0.99, '45.0602', 0.005, '27.0101', 0.002,
                  '11.0701', 0.001, '26.0101', 0.0005);",
        )
        .map_err(|e| e.to_string())?;

        let (rows, contents) = run_copy(&conn, |out| {
            export_sql("ds", 7, 6, &RowLayout::Legacy, false, out)
        })?;
        assert_eq!(rows, 2);
        let mut lines = contents.lines();
        assert_eq!(
            lines.next(),
            Some(
                "row_index,subject_code,catalog_number,course_title,school_name,\
                 school_year_enrolled,ccm6digit_code,ccm6digit_prob,ccm6digit_title"
            )
        );
        // Formula title escaped; classified row carries its cached result and
        // its taxonomy title.
        assert_eq!(
            lines.next(),
            Some(
                "0,ECON,101,\"'=HYPERLINK(\"\"x\"\")\",A,2024,45.0601,0.99,\"Economics, General\""
            )
        );
        // Unclassified row left-joins to empty model cells.
        assert_eq!(lines.next(), Some("1,MATH,201,Calculus,A,2024,,,"));
        assert_eq!(lines.next(), None);
        Ok(())
    }

    /// Round-trip layout (EPI-79): original headers in original order —
    /// mapped cells from the structured columns, unmapped cells from
    /// `extra_columns` — with the ccm columns appended and injection escaping
    /// applied to extra-column values too.
    #[test]
    fn original_layout_reconstructs_source_columns() -> Result<(), String> {
        let conn = scratch_conn()?;
        conn.execute_batch(
            r#"INSERT INTO courses VALUES
                 ('ds', 0, 'ECON', '101', 'Micro', NULL, NULL,
                  '{"1": "Fall 2024", "4": "=EVIL()"}', 'h1');
             INSERT INTO inference_results VALUES
                 (7, 'h1', '45.0601', 0.99, '45.0602', 0.005, '27.0101', 0.002,
                  '11.0701', 0.001, '26.0101', 0.0005);"#,
        )
        .map_err(|e| e.to_string())?;

        let layout = RowLayout::Original {
            headers: vec![
                "sub_pref".to_owned(),
                "term".to_owned(),
                "course".to_owned(),
                "inventory_course_title".to_owned(),
                "notes".to_owned(),
            ],
            mapping: ColumnMap {
                subject: 0,
                catalog: 2,
                title: 3,
            },
        };
        let (rows, contents) = run_copy(&conn, |out| export_sql("ds", 7, 6, &layout, false, out))?;
        assert_eq!(rows, 1);
        let mut lines = contents.lines();
        assert_eq!(
            lines.next(),
            Some(
                "sub_pref,term,course,inventory_course_title,notes,\
                 ccm6digit_code,ccm6digit_prob,ccm6digit_title"
            )
        );
        assert_eq!(
            lines.next(),
            Some("ECON,Fall 2024,101,Micro,'=EVIL(),45.0601,0.99,\"Economics, General\"")
        );
        assert_eq!(lines.next(), None);
        Ok(())
    }

    /// Top-5 candidate columns (EPI-98): rank 1 duplicates the top-1 trio
    /// (stakeholder decision 2026-07-28); ranks 2–5 come from the persisted
    /// `top{k}` columns; titles use the exact-level match with the 2-digit
    /// parent fallback (here: rank 4's code has no 6-digit taxonomy row).
    #[test]
    fn top_candidate_columns_rank_and_title() -> Result<(), String> {
        let conn = scratch_conn()?;
        conn.execute_batch(
            "INSERT INTO courses VALUES
                 ('ds', 0, 'MATH', '201', 'Calculus', NULL, NULL, NULL, 'h1');
             INSERT INTO inference_results VALUES
                 (7, 'h1', '27.0101', 0.9, '45.0601', 0.05, '27.0199', 0.02,
                  '45.9999', 0.01, '27.0101', 0.005);",
        )
        .map_err(|e| e.to_string())?;

        let (rows, contents) = run_copy(&conn, |out| {
            export_sql("ds", 7, 6, &RowLayout::Legacy, true, out)
        })?;
        assert_eq!(rows, 1);
        let mut lines = contents.lines();
        let header = lines.next().unwrap_or_default();
        assert!(
            header.ends_with(
                "ccm6digit_code,ccm6digit_prob,ccm6digit_title,\
                 ccm6digit_code1,ccm6digit_prob1,ccm6digit_title1,\
                 ccm6digit_code2,ccm6digit_prob2,ccm6digit_title2,\
                 ccm6digit_code3,ccm6digit_prob3,ccm6digit_title3,\
                 ccm6digit_code4,ccm6digit_prob4,ccm6digit_title4,\
                 ccm6digit_code5,ccm6digit_prob5,ccm6digit_title5"
            ),
            "header = {header}"
        );
        let row = lines.next().unwrap_or_default();
        // Rank 1 duplicates the top-1 columns verbatim.
        assert!(
            row.contains(
                "27.0101,0.9,\"Mathematics, General\",27.0101,0.9,\"Mathematics, General\","
            ),
            "row = {row}"
        );
        // Ranks whose codes have no 6-digit taxonomy row fall back to the
        // 2-digit parent title: 27.0199 → "Mathematics and Statistics",
        // 45.9999 → "Social Sciences".
        assert!(
            row.contains("27.0199,0.02,Mathematics and Statistics"),
            "row = {row}"
        );
        assert!(row.contains("45.9999,0.01,Social Sciences"), "row = {row}");
        assert_eq!(lines.next(), None);
        Ok(())
    }
}
