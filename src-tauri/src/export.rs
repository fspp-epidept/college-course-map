//! CSV export of classification results (EPI-15) with formula-injection
//! escaping (EPI-16), round-trip original-column output (EPI-79), CCM-named
//! model columns + taxonomy titles (EPI-77/EPI-81), optional top-5 candidate
//! columns (EPI-98), combined multi-model output (EPI-80), and a
//! one-row-per-unique-input mode (EPI-78).
//!
//! The export never ships rows across IPC: `DuckDB` streams straight to disk
//! via `COPY (SELECT ...) TO '<path>' (FORMAT CSV, HEADER)`. The save path
//! comes from a native save dialog opened *in Rust* — the frontend never
//! supplies a path string, so there's nothing to distrust at the boundary.
//!
//! Output shape (stakeholder meeting 2026-07-28): the original CSV's columns
//! in their original order — so the export can be merged back onto the source
//! file — with one model-column set appended per exported digit level:
//! `ccm{2|4|6}digit_code`, `ccm…_prob`, `ccm…_title`, and (when requested)
//! the numbered rank 1–5 candidate columns, where rank 1 duplicates the top-1
//! columns. Datasets imported before migration 0004 (and derived/seeded
//! datasets) have no stored header layout and keep the legacy fixed-column
//! shape. The unique-rows mode collapses to one row per distinct classified
//! input; per-row fields (school, year, extras) are ambiguous for a merged
//! row, so it emits only the assembled-input columns.

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::Manager as _;
use tauri_plugin_dialog::DialogExt as _;

use crate::{db::AppDb, import::ColumnMap};

/// Row granularity of the export (EPI-78).
#[derive(Type, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum RowMode {
    /// One output row per input row (source fidelity, mergeable back onto
    /// the original file).
    All,
    /// One output row per distinct classified input (`content_hash`).
    /// Duplicate inputs share one cached result by construction; the
    /// representative row is the first occurrence (lowest `row_index`).
    Unique,
}

#[derive(Type, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportRequest {
    pub dataset_id: String,
    /// Models whose classifications populate the `ccm*` column sets — one
    /// set per model, ordered by digit level (EPI-80). Rows without a cached
    /// result for a model export with that model's cells empty. Must be
    /// non-empty with distinct digit levels.
    pub model_ids: Vec<i64>,
    /// Emit the numbered top-5 candidate columns (`ccm…_code1` …
    /// `ccm…_title5`) per exported model. Export-dialog toggle (EPI-98).
    pub include_top_candidates: bool,
    pub row_mode: RowMode,
}

#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportOutcome {
    pub path: String,
    pub rows: i64,
}

/// One model the export joins against, resolved from the `models` table.
#[derive(Clone, Copy)]
struct ExportModel {
    model_id: i64,
    digit_level: u8,
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
    /// Unique-rows mode (EPI-78): only the assembled-input columns — per-row
    /// fields are ambiguous once duplicates collapse.
    UniqueInputs,
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
        RowLayout::UniqueInputs => ["subject_code", "catalog_number", "course_title"]
            .iter()
            .map(|col| format!("{} AS {col}", injection_escaped(&format!("c.{col}"))))
            .collect(),
    }
}

/// The candidate-rank code expression for one model: rank 1 is the argmax
/// (`classification`), ranks 2–5 the persisted `top{k}` columns.
fn rank_code_expr(digit_level: u8, k: u8) -> String {
    if k == 1 {
        format!("r{digit_level}.classification")
    } else {
        format!("r{digit_level}.top{k}_code")
    }
}

/// Which candidate ranks the export touches: rank 1 always (it feeds the
/// top-1 trio's title join), 2–5 only when the numbered columns are on.
fn ranks(include_top: bool) -> &'static [u8] {
    if include_top { &[1, 2, 3, 4, 5] } else { &[1] }
}

/// SELECT expressions for one model's appended columns: the top-1 trio, then
/// — when requested — the numbered rank 1–5 candidate columns (rank 1
/// duplicates the top-1 values by stakeholder decision). Taxonomy titles use
/// the same exact-level-else-2-digit-parent fallback as the results view
/// (`courses.rs::attach_results`); `t{d}_{k}`/`p{d}_{k}` are the per-model,
/// per-rank taxonomy join aliases emitted by [`model_joins`].
fn model_column_exprs(digit_level: u8, include_top: bool) -> Vec<String> {
    let prefix = format!("ccm{digit_level}digit");
    let title_expr =
        |k: u8| format!("COALESCE(t{digit_level}_{k}.title, p{digit_level}_{k}.title)");
    let prob_expr = |k: u8| {
        if k == 1 {
            format!("r{digit_level}.probability")
        } else {
            format!("r{digit_level}.top{k}_prob")
        }
    };

    let mut cols = vec![
        format!(
            "{} AS {prefix}_code",
            injection_escaped(&rank_code_expr(digit_level, 1))
        ),
        format!("r{digit_level}.probability AS {prefix}_prob"),
        format!("{} AS {prefix}_title", injection_escaped(&title_expr(1))),
    ];
    if include_top {
        for k in 1..=5_u8 {
            cols.push(format!(
                "{} AS {prefix}_code{k}",
                injection_escaped(&rank_code_expr(digit_level, k))
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

/// LEFT JOINs for one model: its `inference_results` alias (`r{d}`) plus the
/// per-rank taxonomy resolution — exact match at the model's digit level
/// (`t{d}_{k}`), 2-digit parent by code prefix (`p{d}_{k}`).
fn model_joins(model: ExportModel, include_top: bool) -> String {
    let d = model.digit_level;
    let base = format!(
        "
        LEFT JOIN inference_results r{d}
          ON r{d}.model_id = {id} AND r{d}.content_hash = c.content_hash",
        id = model.model_id,
    );
    let taxonomy = ranks(include_top)
        .iter()
        .map(|&k| {
            let code = rank_code_expr(d, k);
            format!(
                "
        LEFT JOIN ccm_taxonomy t{d}_{k}
          ON t{d}_{k}.digit_level = {d} AND t{d}_{k}.code = {code}
        LEFT JOIN ccm_taxonomy p{d}_{k}
          ON p{d}_{k}.digit_level = 2 AND p{d}_{k}.code = substr({code}, 1, 2)"
            )
        })
        .collect::<Vec<_>>()
        .concat();
    format!("{base}{taxonomy}")
}

fn export_sql(
    dataset_id: &str,
    models: &[ExportModel],
    layout: &RowLayout,
    include_top: bool,
    row_mode: RowMode,
    target_path: &str,
) -> String {
    let mut cols = source_column_exprs(layout);
    for model in models {
        cols.extend(model_column_exprs(model.digit_level, include_top));
    }
    let col_list = cols.join(",\n           ");
    let joins = models
        .iter()
        .map(|&m| model_joins(m, include_top))
        .collect::<Vec<_>>()
        .concat();

    // The `c` alias is either the courses table itself or, in unique mode,
    // a grouped subquery collapsing duplicates to their first occurrence
    // (same arg_min representative-row pattern as the run pipeline's
    // materialize_misses). Either way the join surface is identical:
    // `c.content_hash` plus the assembled-input columns.
    let (source, filter, order) = match row_mode {
        RowMode::All => (
            "courses c".to_owned(),
            format!(
                "
        WHERE c.dataset_id = '{dataset}'",
                dataset = sql_quote(dataset_id),
            ),
            "c.row_index",
        ),
        RowMode::Unique => (
            // The dataset filter lives inside the subquery, so no outer
            // WHERE is needed (or valid — it would follow the joins).
            format!(
                "(
            SELECT content_hash,
                   arg_min(subject_code, row_index) AS subject_code,
                   arg_min(catalog_number, row_index) AS catalog_number,
                   arg_min(course_title, row_index) AS course_title,
                   min(row_index) AS first_row
            FROM courses
            WHERE dataset_id = '{dataset}'
            GROUP BY content_hash
        ) c",
                dataset = sql_quote(dataset_id),
            ),
            String::new(),
            "c.first_row",
        ),
    };
    format!(
        "COPY (
        SELECT {col_list}
        FROM {source}{joins}{filter}
        ORDER BY {order}
    ) TO '{path}' (FORMAT CSV, HEADER)",
        path = sql_quote(target_path),
    )
}

/// Resolve the export's models (id + digit level), row layout, and
/// default-filename inputs. The layout comes from the dataset's source file
/// when it stored the original header order (post-0004 imports); unique mode
/// forces the assembled-input layout regardless.
fn resolve_export_inputs(
    conn: &duckdb::Connection,
    req: &ExportRequest,
) -> Result<(String, Vec<ExportModel>, RowLayout), String> {
    if req.model_ids.is_empty() {
        return Err("export requires at least one model".to_owned());
    }

    let mut models = Vec::with_capacity(req.model_ids.len());
    for &model_id in &req.model_ids {
        let model_type: String = conn
            .query_row(
                "SELECT model_type FROM models WHERE id = ?",
                [model_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("model lookup ({model_id}): {e}"))?;
        let digit_level: u8 = model_type
            .parse()
            .map_err(|e| format!("non-numeric model_type {model_type:?}: {e}"))?;
        if !matches!(digit_level, 2 | 4 | 6) {
            return Err(format!("unsupported model digit level {digit_level}"));
        }
        models.push(ExportModel {
            model_id,
            digit_level,
        });
    }
    // Distinct digit levels keep the ccm{N}digit column names unambiguous;
    // ascending order keeps the combined file's column order stable.
    models.sort_by_key(|m| m.digit_level);
    if models
        .windows(2)
        .any(|w| matches!(w, [a, b] if a.digit_level == b.digit_level))
    {
        return Err("export models must have distinct digit levels".to_owned());
    }

    let (title, headers_json, mapping_json): (String, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT d.title, sf.original_headers, sf.column_mapping
             FROM datasets d
             LEFT JOIN source_files sf ON sf.id = d.source_file_id
             WHERE d.id = ?",
            [&req.dataset_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|e| format!("dataset lookup: {e}"))?;

    let layout = match (req.row_mode, headers_json, mapping_json) {
        (RowMode::Unique, ..) => RowLayout::UniqueInputs,
        (RowMode::All, Some(headers_json), Some(mapping_json)) => {
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
        (RowMode::All, ..) => RowLayout::Legacy,
    };
    Ok((title, models, layout))
}

/// Suggested filename: the single-model form keeps `{slug}-{d}-digit`, the
/// combined form says so (EPI-80); unique mode appends `-unique`.
fn suggested_filename(slug: &str, models: &[ExportModel], row_mode: RowMode) -> String {
    let base = match models {
        [only] => format!("{slug}-{}-digit", only.digit_level),
        _ => format!("{slug}-ccm-all-levels"),
    };
    match row_mode {
        RowMode::All => format!("{base}.csv"),
        RowMode::Unique => format!("{base}-unique.csv"),
    }
}

/// Export a dataset's courses (joined with the requested models' cached
/// classifications + taxonomy titles) to a CSV the user picks via the native
/// save dialog. Returns `None` when the user cancels the dialog.
#[tauri::command]
#[specta::specta]
pub(crate) async fn export_results(
    req: ExportRequest,
    app: tauri::AppHandle,
) -> Result<Option<ExportOutcome>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let db = app.state::<AppDb>();

        let (title, models, layout) = {
            let conn = db.ro()?;
            resolve_export_inputs(&conn, &req)?
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
            .set_file_name(suggested_filename(&slug, &models, req.row_mode))
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
            &models,
            &layout,
            req.include_top_candidates,
            req.row_mode,
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
    use super::{
        ColumnMap, ExportModel, RowLayout, RowMode, export_sql, injection_escaped, quote_ident,
    };

    fn six(model_id: i64) -> Vec<ExportModel> {
        vec![ExportModel {
            model_id,
            digit_level: 6,
        }]
    }

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
        let sql = export_sql(
            "id'--",
            &six(3),
            &RowLayout::Legacy,
            false,
            RowMode::All,
            "/tmp/o'brien.csv",
        );
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
            export_sql("ds", &six(7), &RowLayout::Legacy, false, RowMode::All, out)
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
        let (rows, contents) = run_copy(&conn, |out| {
            export_sql("ds", &six(7), &layout, false, RowMode::All, out)
        })?;
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
    /// parent fallback.
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
            export_sql("ds", &six(7), &RowLayout::Legacy, true, RowMode::All, out)
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

    /// Combined multi-model export (EPI-80): one column set per digit level,
    /// ascending; a level with no cached result exports as empty cells
    /// (partial coverage stays visible rather than erroring).
    #[test]
    fn multi_model_appends_one_column_set_per_level() -> Result<(), String> {
        let conn = scratch_conn()?;
        conn.execute_batch(
            "INSERT INTO courses VALUES
                 ('ds', 0, 'MATH', '201', 'Calculus', 'A', '2024', NULL, 'h1');
             INSERT INTO inference_results VALUES
                 (2, 'h1', '27', 0.97, '45', 0.01, '11', 0.005, '26', 0.004, '23', 0.003),
                 (7, 'h1', '27.0101', 0.9, '45.0601', 0.05, '27.0199', 0.02,
                  '45.9999', 0.01, '27.0101', 0.005);",
        )
        .map_err(|e| e.to_string())?;

        // Models arrive unsorted; the SQL orders column sets by digit level.
        let models = [
            ExportModel {
                model_id: 7,
                digit_level: 6,
            },
            ExportModel {
                model_id: 2,
                digit_level: 2,
            },
            ExportModel {
                model_id: 5,
                digit_level: 4,
            },
        ];
        let mut sorted = models;
        sorted.sort_by_key(|m| m.digit_level);
        let (rows, contents) = run_copy(&conn, |out| {
            export_sql("ds", &sorted, &RowLayout::Legacy, false, RowMode::All, out)
        })?;
        assert_eq!(rows, 1);
        let mut lines = contents.lines();
        assert_eq!(
            lines.next(),
            Some(
                "row_index,subject_code,catalog_number,course_title,school_name,\
                 school_year_enrolled,\
                 ccm2digit_code,ccm2digit_prob,ccm2digit_title,\
                 ccm4digit_code,ccm4digit_prob,ccm4digit_title,\
                 ccm6digit_code,ccm6digit_prob,ccm6digit_title"
            )
        );
        // 2- and 6-digit results present; the never-classified 4-digit level
        // exports empty cells between them.
        assert_eq!(
            lines.next(),
            Some(
                "0,MATH,201,Calculus,A,2024,\
                 27,0.97,Mathematics and Statistics,,,,\
                 27.0101,0.9,\"Mathematics, General\""
            )
        );
        assert_eq!(lines.next(), None);
        Ok(())
    }

    /// Unique-rows mode (EPI-78): duplicates collapse to one row per
    /// `content_hash` (representative = lowest `row_index`, output ordered by
    /// first occurrence), and the source columns are only the assembled-input
    /// trio — per-row fields would be ambiguous for a merged row.
    #[test]
    fn unique_mode_collapses_duplicate_inputs() -> Result<(), String> {
        let conn = scratch_conn()?;
        conn.execute_batch(
            "INSERT INTO courses VALUES
                 ('ds', 0, 'MATH', '201', 'Calculus', 'A', '2023', NULL, 'h1'),
                 ('ds', 1, 'ECON', '101', 'Micro', 'A', '2023', NULL, 'h2'),
                 ('ds', 2, 'MATH', '201', 'Calculus', 'B', '2024', NULL, 'h1'),
                 ('ds', 3, 'MATH', '201', 'Calculus', 'C', '2025', NULL, 'h1');
             INSERT INTO inference_results VALUES
                 (7, 'h1', '27.0101', 0.9, '45.0601', 0.05, '27.0199', 0.02,
                  '45.9999', 0.01, '27.0101', 0.005),
                 (7, 'h2', '45.0601', 0.8, '27.0101', 0.1, '45.9999', 0.02,
                  '11.0701', 0.01, '26.0101', 0.005);",
        )
        .map_err(|e| e.to_string())?;

        let (rows, contents) = run_copy(&conn, |out| {
            export_sql(
                "ds",
                &six(7),
                &RowLayout::UniqueInputs,
                false,
                RowMode::Unique,
                out,
            )
        })?;
        assert_eq!(rows, 2);
        let mut lines = contents.lines();
        assert_eq!(
            lines.next(),
            Some(
                "subject_code,catalog_number,course_title,\
                 ccm6digit_code,ccm6digit_prob,ccm6digit_title"
            )
        );
        // First-occurrence order: h1 (row 0) before h2 (row 1); the three
        // Calculus duplicates are one row.
        assert_eq!(
            lines.next(),
            Some("MATH,201,Calculus,27.0101,0.9,\"Mathematics, General\"")
        );
        assert_eq!(
            lines.next(),
            Some("ECON,101,Micro,45.0601,0.8,\"Economics, General\"")
        );
        assert_eq!(lines.next(), None);
        Ok(())
    }
}
