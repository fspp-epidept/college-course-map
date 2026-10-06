//! Course filters (#254). A [`FilterSpec`] is a flat list of rows combined
//! with AND, each row one field, one operator and the values it matches
//! (OR within the row: "subject is any of ACCT, ACC, ACTG"). The same spec
//! filters a dataset's courses table and, later, picks the rows of a derived
//! dataset.
//!
//! The spec is typed and compiled here to a `WHERE` fragment with bound
//! parameters. The only text that reaches the SQL unbound is a column
//! position formatted from a stored layout (a `usize`) and column names
//! chosen by a `match` on the field enum; values, dataset ids and model ids
//! are parameters. The frontend never sends SQL (security baseline).
//!
//! Text comparisons are case-insensitive and ignore surrounding whitespace
//! (decision 2026-10-05): subject codes arrive as `ACCT`, `acct` and
//! `ACCT ` across files, and matching codes across schools is the point.
//! `contains` / `starts with` are `DuckDB`'s functions of those names, not
//! `LIKE`, so `%` and `_` in a value need no escaping.

use std::collections::BTreeMap;

use duckdb::types::Value;
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;

use crate::{
    boot::{Boot, Services},
    layout::Layout,
};

/// Bounds on one spec, so the SQL it compiles to stays small.
const MAX_ROWS: usize = 64;
const MAX_VALUES: usize = 256;
/// Distinct values one [`column_values`] call returns.
const MAX_DISTINCT_VALUES: i64 = 100;

#[derive(Type, Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FilterSpec {
    /// AND of rows. An empty list matches every row.
    pub rows: Vec<FilterRow>,
}

#[derive(Type, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FilterRow {
    pub field: FilterField,
    pub op: FilterOp,
    /// OR within the row. Empty for `isEmpty` / `isNotEmpty`, at least one
    /// non-blank value otherwise.
    pub values: Vec<String>,
}

#[derive(Type, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum FilterField {
    Subject,
    Catalog,
    Title,
    /// A column of the scope by name (a header key for one dataset, an
    /// output column for a derivation), resolved to a position per source.
    Column {
        name: String,
    },
    /// Values are source dataset ids. Only meaningful over several sources.
    SourceDataset,
    /// The cached classification at a digit level. Matches only rows that
    /// already have a result for that level's active model.
    #[serde(rename_all = "camelCase")]
    Ccm {
        digit_level: u8,
    },
}

#[derive(Type, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum FilterOp {
    Is,
    IsNot,
    Contains,
    NotContains,
    StartsWith,
    IsEmpty,
    IsNotEmpty,
}

impl FilterOp {
    fn takes_values(self) -> bool {
        !matches!(self, Self::IsEmpty | Self::IsNotEmpty)
    }
}

/// Where a column of the scope lives in one source dataset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ColumnSource {
    pub dataset_id: String,
    pub position: usize,
}

/// What a filter runs over: the source datasets, in order, and the columns
/// a `Column` field may name, each with its position in every source that
/// has it. A source missing from a column's list reads that column as empty.
#[derive(Clone, Debug, Default)]
pub(crate) struct Scope {
    pub sources: Vec<String>,
    pub columns: BTreeMap<String, Vec<ColumnSource>>,
}

impl Scope {
    /// One dataset over its own layout: every non-mapped header is a
    /// column named by its key ([`column_keys`]). No layout, no columns.
    pub(crate) fn for_dataset(dataset_id: &str, layout: Option<&Layout>) -> Self {
        let columns = layout
            .map(column_keys)
            .unwrap_or_default()
            .into_iter()
            .map(|(position, key)| {
                (
                    key,
                    vec![ColumnSource {
                        dataset_id: dataset_id.to_owned(),
                        position,
                    }],
                )
            })
            .collect();
        Self {
            sources: vec![dataset_id.to_owned()],
            columns,
        }
    }
}

/// The key of every non-mapped column of a layout, with its position: the
/// header trimmed and lowercased, and ` #2`, ` #3`, … appended to a header
/// that repeats within the layout (CSVs may repeat a header name). Keys are
/// what filters name and what matches columns across datasets.
pub(crate) fn column_keys(layout: &Layout) -> Vec<(usize, String)> {
    let mapped = [
        layout.mapping.subject,
        layout.mapping.catalog,
        layout.mapping.title,
    ];
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    layout
        .headers
        .iter()
        .enumerate()
        .filter(|(position, _)| !mapped.contains(position))
        .map(|(position, header)| {
            let base = header.trim().to_lowercase();
            let n = seen.entry(base.clone()).or_insert(0);
            *n += 1;
            let key = if *n == 1 {
                base
            } else {
                format!("{base} #{n}")
            };
            (position, key)
        })
        .collect()
}

/// A compiled spec: a fragment to append to a `WHERE` over the courses
/// table aliased `c` (empty, or starting with ` AND`), and the parameters
/// its `?` placeholders bind, in order.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Compiled {
    pub sql: String,
    pub params: Vec<Value>,
}

/// One SQL expression with the parameters it binds.
#[derive(Clone, Debug)]
struct Expr {
    sql: String,
    params: Vec<Value>,
}

impl Expr {
    fn fixed(sql: &str) -> Self {
        Self {
            sql: sql.to_owned(),
            params: Vec::new(),
        }
    }
}

/// A text expression normalized for comparison.
fn normalized(expr: &str) -> String {
    format!("lower(trim(COALESCE({expr}, '')))")
}

/// The cell of one extra column by position, over alias `c`. Keys are
/// positions we stored; the quoted-key path form addresses object keys,
/// not array positions.
pub(crate) fn cell_sql(position: usize) -> String {
    format!("json_extract_string(c.extra_columns, '$.\"{position}\"')")
}

/// The expression of a scope column over alias `c`, with its parameters:
/// the cell itself for one source, else a `CASE` on `c.dataset_id` with the
/// ids bound (a source without the column is left out and reads NULL).
pub(crate) fn column_sql(scope: &Scope, name: &str) -> Result<(String, Vec<Value>), String> {
    let sources = scope
        .columns
        .get(name)
        .filter(|sources| !sources.is_empty())
        .ok_or_else(|| format!("unknown column \u{201c}{name}\u{201d}"))?;
    if let ([only], [_]) = (sources.as_slice(), scope.sources.as_slice()) {
        return Ok((cell_sql(only.position), Vec::new()));
    }
    let mut sql = String::from("CASE c.dataset_id");
    let mut params = Vec::with_capacity(sources.len());
    for source in sources {
        sql.push_str(" WHEN ? THEN ");
        sql.push_str(&cell_sql(source.position));
        params.push(Value::Text(source.dataset_id.clone()));
    }
    sql.push_str(" END");
    Ok((sql, params))
}

/// The expression a text field compares, over alias `c`.
fn field_expr(field: &FilterField, scope: &Scope) -> Result<Expr, String> {
    match field {
        FilterField::Subject => Ok(Expr::fixed("c.subject_code")),
        FilterField::Catalog => Ok(Expr::fixed("c.catalog_number")),
        FilterField::Title => Ok(Expr::fixed("c.course_title")),
        FilterField::SourceDataset => Ok(Expr::fixed("c.dataset_id")),
        FilterField::Column { name } => {
            let (sql, params) = column_sql(scope, name)?;
            Ok(Expr { sql, params })
        }
        FilterField::Ccm { .. } => Err("a CCM field is not a text expression".to_owned()),
    }
}

/// `expr <op> values` as one parenthesized predicate. Values are OR'd; the
/// negative operators negate the whole disjunction, so "is not ACCT or ACC"
/// excludes both. The expression's own parameters are bound once per value
/// because the expression repeats.
fn predicate(expr: &Expr, op: FilterOp, values: &[String], exact: bool) -> Expr {
    let lhs = if exact {
        expr.sql.clone()
    } else {
        normalized(&expr.sql)
    };
    let rhs = if exact { "?" } else { "lower(trim(?))" };
    let (negate, term) = match op {
        FilterOp::Is => (false, format!("{lhs} = {rhs}")),
        FilterOp::IsNot => (true, format!("{lhs} = {rhs}")),
        FilterOp::Contains => (false, format!("contains({lhs}, {rhs})")),
        FilterOp::NotContains => (true, format!("contains({lhs}, {rhs})")),
        FilterOp::StartsWith => (false, format!("starts_with({lhs}, {rhs})")),
        FilterOp::IsEmpty | FilterOp::IsNotEmpty => {
            let test = if op == FilterOp::IsEmpty { "=" } else { "<>" };
            return Expr {
                sql: format!("(trim(COALESCE({}, '')) {test} '')", expr.sql),
                params: expr.params.clone(),
            };
        }
    };
    let mut params = Vec::with_capacity(values.len() * (expr.params.len() + 1));
    let terms: Vec<String> = values
        .iter()
        .map(|value| {
            params.extend(expr.params.iter().cloned());
            params.push(Value::Text(value.clone()));
            term.clone()
        })
        .collect();
    let joined = terms.join(" OR ");
    Expr {
        sql: if negate {
            format!("NOT ({joined})")
        } else {
            format!("({joined})")
        },
        params,
    }
}

/// Check a spec's shape before anything is compiled from it: bounds, values
/// where the operator takes them and nowhere else, fields the scope has.
pub(crate) fn check(spec: &FilterSpec, scope: &Scope) -> Result<(), String> {
    if spec.rows.len() > MAX_ROWS {
        return Err(format!("a filter can have at most {MAX_ROWS} rows"));
    }
    for row in &spec.rows {
        if row.values.len() > MAX_VALUES {
            return Err(format!("a filter row can have at most {MAX_VALUES} values"));
        }
        if row.op.takes_values() {
            if row.values.iter().all(|v| v.trim().is_empty()) {
                return Err("a filter row needs at least one value".to_owned());
            }
        } else if !row.values.is_empty() {
            return Err("an empty / not empty filter row takes no values".to_owned());
        }
        match &row.field {
            FilterField::Column { name } => {
                if !scope.columns.contains_key(name) {
                    return Err(format!("unknown column \u{201c}{name}\u{201d}"));
                }
            }
            FilterField::SourceDataset => {
                if scope.sources.len() < 2 {
                    return Err("source dataset applies only across several datasets".to_owned());
                }
                if !matches!(row.op, FilterOp::Is | FilterOp::IsNot) {
                    return Err("source dataset supports only is / is not".to_owned());
                }
                if let Some(unknown) = row.values.iter().find(|v| !scope.sources.contains(v)) {
                    return Err(format!("unknown source dataset {unknown}"));
                }
            }
            FilterField::Ccm { digit_level } => {
                if !matches!(digit_level, 2 | 4 | 6) {
                    return Err(format!("unsupported CCM digit level {digit_level}"));
                }
                if !row.op.takes_values() {
                    return Err("a CCM code filter needs values".to_owned());
                }
            }
            FilterField::Subject | FilterField::Catalog | FilterField::Title => {}
        }
    }
    Ok(())
}

/// Compile a checked spec. `model_for_level` resolves a CCM field's digit
/// level to the active model's id (the catalog); `None` is an error, since
/// a filter on a model this build doesn't ship can't mean anything.
pub(crate) fn compile(
    spec: &FilterSpec,
    scope: &Scope,
    model_for_level: impl Fn(u8) -> Option<i64>,
) -> Result<Compiled, String> {
    check(spec, scope)?;
    let mut out = Compiled::default();
    for row in &spec.rows {
        let values: Vec<String> = row
            .values
            .iter()
            .filter(|v| !v.trim().is_empty())
            .cloned()
            .collect();
        let clause = match &row.field {
            FilterField::Ccm { digit_level } => {
                let model_id = model_for_level(*digit_level)
                    .ok_or_else(|| format!("no active {digit_level}-digit model"))?;
                let inner = predicate(&Expr::fixed("ir.classification"), row.op, &values, false);
                let mut params = vec![Value::BigInt(model_id)];
                params.extend(inner.params);
                Expr {
                    sql: format!(
                        "EXISTS (SELECT 1 FROM inference_results ir
                                 WHERE ir.model_id = ? AND ir.content_hash = c.content_hash
                                   AND {})",
                        inner.sql
                    ),
                    params,
                }
            }
            FilterField::SourceDataset => {
                predicate(&field_expr(&row.field, scope)?, row.op, &values, true)
            }
            _ => predicate(&field_expr(&row.field, scope)?, row.op, &values, false),
        };
        out.sql.push_str(" AND ");
        out.sql.push_str(&clause.sql);
        out.params.extend(clause.params);
    }
    Ok(out)
}

/// The columns of one dataset a filter may name: key and header spelling,
/// in layout order. Empty for a dataset with no stored layout.
#[derive(Type, Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DatasetColumn {
    pub name: String,
    pub header: String,
}

#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn dataset_columns(
    dataset_id: String,
    boot: State<'_, Boot>,
) -> Result<Vec<DatasetColumn>, String> {
    let conn = boot.ready()?.db.ro()?;
    let Some(layout) = Layout::read(&conn, &dataset_id)? else {
        return Ok(Vec::new());
    };
    Ok(column_keys(&layout)
        .into_iter()
        .map(|(position, name)| DatasetColumn {
            header: layout.headers.get(position).cloned().unwrap_or_default(),
            name,
        })
        .collect())
}

/// A value picker's request: the distinct values of one field in one
/// dataset, narrowed by a search string.
#[derive(Type, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ColumnValuesRequest {
    pub dataset_id: String,
    pub field: FilterField,
    pub search: String,
}

#[derive(Type, Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ColumnValue {
    pub value: String,
    pub count: i64,
    /// For a CCM code, its taxonomy title (the 2-digit parent's for a
    /// 6-digit code the table lacks; none at the 4-digit level).
    pub label: Option<String>,
}

/// The most frequent distinct values of a field in a dataset (Excel
/// `AutoFilter` style), trimmed, blank values left out, at most
/// [`MAX_DISTINCT_VALUES`], those containing `search` (case-insensitive)
/// first by count then by value. Read-only; one `GROUP BY`.
#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn column_values(
    req: ColumnValuesRequest,
    boot: State<'_, Boot>,
) -> Result<Vec<ColumnValue>, String> {
    let Services { db, catalog, .. } = boot.ready()?;
    let conn = db.ro()?;
    let layout = Layout::read(&conn, &req.dataset_id)?;
    let scope = Scope::for_dataset(&req.dataset_id, layout.as_ref());
    values(&conn, &scope, &req, |level| catalog.model_id(level))
}

fn values(
    conn: &duckdb::Connection,
    scope: &Scope,
    req: &ColumnValuesRequest,
    model_for_level: impl Fn(u8) -> Option<i64>,
) -> Result<Vec<ColumnValue>, String> {
    let search = Value::Text(req.search.trim().to_lowercase());
    let (sql, params) = match &req.field {
        FilterField::SourceDataset => {
            return Err("source datasets are listed by the UI".to_owned());
        }
        FilterField::Ccm { digit_level } => {
            let model_id = model_for_level(*digit_level)
                .ok_or_else(|| format!("no active {digit_level}-digit model"))?;
            // Same title resolution as the results view: exact level, else
            // the 2-digit parent, and never a title at the 4-digit level.
            let title = if *digit_level == 4 {
                "NULL"
            } else {
                "COALESCE(t.title, p.title)"
            };
            (
                format!(
                    "SELECT ir.classification, COUNT(*) AS n, {title} AS label
                     FROM courses c
                     JOIN inference_results ir
                       ON ir.content_hash = c.content_hash AND ir.model_id = ?
                     LEFT JOIN ccm_taxonomy t
                       ON t.digit_level = ? AND t.code = ir.classification
                     LEFT JOIN ccm_taxonomy p
                       ON p.digit_level = 2 AND p.code = substr(ir.classification, 1, 2)
                     WHERE c.dataset_id = ?
                       AND (contains(lower(ir.classification), ?)
                            OR contains(lower(COALESCE({title}, '')), ?))
                     GROUP BY ir.classification, label
                     ORDER BY n DESC, ir.classification
                     LIMIT {MAX_DISTINCT_VALUES}"
                ),
                vec![
                    Value::BigInt(model_id),
                    Value::BigInt(i64::from(*digit_level)),
                    Value::Text(req.dataset_id.clone()),
                    search.clone(),
                    search,
                ],
            )
        }
        field => {
            let expr = field_expr(field, scope)?;
            let mut params = expr.params;
            params.push(Value::Text(req.dataset_id.clone()));
            params.push(search);
            (
                format!(
                    "SELECT v, COUNT(*) AS n, NULL AS label
                     FROM (SELECT trim({}) AS v FROM courses c WHERE c.dataset_id = ?) t
                     WHERE v IS NOT NULL AND v <> '' AND contains(lower(v), ?)
                     GROUP BY v
                     ORDER BY n DESC, v
                     LIMIT {MAX_DISTINCT_VALUES}",
                    expr.sql
                ),
                params,
            )
        }
    };
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("prepare column values: {e}"))?;
    let rows = stmt
        .query_map(duckdb::params_from_iter(params.iter()), |row| {
            Ok(ColumnValue {
                value: row.get(0)?,
                count: row.get(1)?,
                label: row.get(2)?,
            })
        })
        .map_err(|e| format!("query column values: {e}"))?;
    rows.collect::<Result<_, _>>()
        .map_err(|e| format!("column values: {e}"))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use duckdb::types::Value;

    use super::{
        ColumnSource, ColumnValuesRequest, FilterField, FilterOp, FilterRow, FilterSpec, Scope,
        column_keys, compile, values,
    };
    use crate::{layout::Layout, preflight::ColumnMap};

    /// Two datasets with the real `courses` columns: `a` has a SCHOOL
    /// column at position 3, `b` has it at 0 and a YEAR at 4; one cached
    /// 6-digit result.
    fn scratch() -> Result<duckdb::Connection, String> {
        let conn = duckdb::Connection::open_in_memory().map_err(|e| e.to_string())?;
        conn.execute_batch(
            "CREATE TABLE courses (
                 dataset_id VARCHAR, row_index BIGINT, subject_code VARCHAR,
                 catalog_number VARCHAR, course_title VARCHAR, extra_columns JSON,
                 content_hash VARCHAR);
             CREATE TABLE inference_results (
                 model_id BIGINT, content_hash VARCHAR, classification VARCHAR);
             CREATE TABLE ccm_taxonomy (digit_level TINYINT, code TEXT, title TEXT);
             INSERT INTO ccm_taxonomy VALUES (2, '52', 'Business'), (6, '52.0301', 'Accounting');
             INSERT INTO courses VALUES
                 ('a', 0, 'ACCT', '101', 'Intro Accounting', '{\"3\":\"Ross\"}', 'h0'),
                 ('a', 1, ' acct ', '201', '50% off', '{\"3\":\"Ross\"}', 'h1'),
                 ('a', 2, 'MATH', '101', 'Calculus', NULL, 'h2'),
                 ('b', 0, 'ACC', '301', 'Tax', '{\"0\":\"Wharton\",\"4\":\"2024\"}', 'h3'),
                 ('b', 1, 'ACC', '301', 'Tax', '{\"0\":\"Wharton\",\"4\":\"2024\"}', 'h3');
             INSERT INTO inference_results VALUES (6, 'h0', '52.0301'), (6, 'h3', '52.1601');",
        )
        .map_err(|e| e.to_string())?;
        Ok(conn)
    }

    fn layout(headers: &[&str], mapping: ColumnMap) -> Result<Layout, String> {
        Layout::new(headers.iter().map(|h| (*h).to_owned()).collect(), mapping)
    }

    fn row(field: FilterField, op: FilterOp, values: &[&str]) -> FilterRow {
        FilterRow {
            field,
            op,
            values: values.iter().map(|v| (*v).to_owned()).collect(),
        }
    }

    fn column(name: &str) -> FilterField {
        FilterField::Column {
            name: name.to_owned(),
        }
    }

    /// Row indexes of dataset `a` (or of the scope's sources) that match.
    fn matching(
        conn: &duckdb::Connection,
        scope: &Scope,
        spec: &FilterSpec,
    ) -> Result<Vec<(String, i64)>, String> {
        let compiled = compile(spec, scope, |level| (level == 6).then_some(6))?;
        let placeholders = scope
            .sources
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(", ");
        let mut params: Vec<Value> = scope
            .sources
            .iter()
            .map(|s| Value::Text(s.clone()))
            .collect();
        params.extend(compiled.params);
        let mut stmt = conn
            .prepare(&format!(
                "SELECT dataset_id, row_index FROM courses c
                 WHERE c.dataset_id IN ({placeholders}){}
                 ORDER BY dataset_id, row_index",
                compiled.sql
            ))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(duckdb::params_from_iter(params.iter()), |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }

    fn a(rows: &[i64]) -> Vec<(String, i64)> {
        rows.iter().map(|&r| ("a".to_owned(), r)).collect()
    }

    /// Keys are trimmed and lowercased, skip the mapped three, and number
    /// repeats within the layout.
    #[test]
    fn column_keys_follow_the_layout() -> Result<(), String> {
        let layout = layout(
            &[
                "School", "SUBJ", "NUM", "TITLE", " Notes", "notes ", "NOTES",
            ],
            ColumnMap {
                subject: 1,
                catalog: 2,
                title: 3,
            },
        )?;
        assert_eq!(
            column_keys(&layout),
            vec![
                (0, "school".to_owned()),
                (4, "notes".to_owned()),
                (5, "notes #2".to_owned()),
                (6, "notes #3".to_owned()),
            ]
        );
        Ok(())
    }

    /// Every operator over one dataset: text matching ignores case and
    /// surrounding whitespace, values within a row are OR'd, rows are
    /// AND'd, `%` and `'` in values match literally, and a NULL cell counts
    /// as empty / as "is not".
    #[test]
    fn operators_match_as_documented() -> Result<(), String> {
        let conn = scratch()?;
        let layout = layout(
            &["SUBJ", "NUM", "TITLE", "SCHOOL"],
            ColumnMap {
                subject: 0,
                catalog: 1,
                title: 2,
            },
        )?;
        let scope = Scope::for_dataset("a", Some(&layout));
        let run = |rows: Vec<FilterRow>| matching(&conn, &scope, &FilterSpec { rows });

        assert_eq!(run(vec![])?, a(&[0, 1, 2]));
        assert_eq!(
            run(vec![row(FilterField::Subject, FilterOp::Is, &["acct"])])?,
            a(&[0, 1])
        );
        assert_eq!(
            run(vec![row(
                FilterField::Subject,
                FilterOp::Is,
                &["ACC", " math"]
            )])?,
            a(&[2])
        );
        assert_eq!(
            run(vec![row(FilterField::Subject, FilterOp::IsNot, &["ACCT"])])?,
            a(&[2])
        );
        assert_eq!(
            run(vec![row(FilterField::Title, FilterOp::Contains, &["calc"])])?,
            a(&[2])
        );
        assert_eq!(
            run(vec![row(FilterField::Title, FilterOp::Contains, &["50%"])])?,
            a(&[1])
        );
        assert_eq!(
            run(vec![row(FilterField::Title, FilterOp::Contains, &["%"])])?,
            a(&[1])
        );
        assert_eq!(
            run(vec![row(
                FilterField::Title,
                FilterOp::NotContains,
                &["acc", "'"]
            )])?,
            a(&[1, 2])
        );
        assert_eq!(
            run(vec![row(
                FilterField::Catalog,
                FilterOp::StartsWith,
                &["1"]
            )])?,
            a(&[0, 2])
        );
        assert_eq!(
            run(vec![row(column("school"), FilterOp::IsEmpty, &[])])?,
            a(&[2])
        );
        assert_eq!(
            run(vec![row(column("school"), FilterOp::IsNotEmpty, &[])])?,
            a(&[0, 1])
        );
        assert_eq!(
            run(vec![row(column("school"), FilterOp::IsNot, &["ross"])])?,
            a(&[2])
        );
        assert_eq!(
            run(vec![
                row(FilterField::Subject, FilterOp::Is, &["ACCT"]),
                row(FilterField::Catalog, FilterOp::Is, &["201"]),
            ])?,
            a(&[1])
        );
        Ok(())
    }

    /// A CCM filter matches only rows with a cached result at that level,
    /// by code or prefix, and "is not" still requires a result.
    #[test]
    fn ccm_filters_match_classified_rows_only() -> Result<(), String> {
        let conn = scratch()?;
        let scope = Scope::for_dataset("a", None);
        let ccm = FilterField::Ccm { digit_level: 6 };
        let run = |rows: Vec<FilterRow>| matching(&conn, &scope, &FilterSpec { rows });
        assert_eq!(
            run(vec![row(ccm.clone(), FilterOp::Is, &["52.0301"])])?,
            a(&[0])
        );
        assert_eq!(
            run(vec![row(ccm.clone(), FilterOp::StartsWith, &["52"])])?,
            a(&[0])
        );
        assert_eq!(
            run(vec![row(ccm.clone(), FilterOp::IsNot, &["52.0301"])])?,
            a(&[])
        );
        assert!(
            compile(
                &FilterSpec {
                    rows: vec![row(
                        FilterField::Ccm { digit_level: 2 },
                        FilterOp::Is,
                        &["52"]
                    )]
                },
                &scope,
                |level| (level == 6).then_some(6),
            )
            .is_err(),
            "a level with no active model compiled"
        );
        Ok(())
    }

    /// Over two sources a column resolves per source (and reads empty in a
    /// source without it), and `source dataset` picks by id.
    #[test]
    fn columns_resolve_per_source() -> Result<(), String> {
        let conn = scratch()?;
        let mut scope = Scope {
            sources: vec!["a".to_owned(), "b".to_owned()],
            columns: BTreeMap::default(),
        };
        let src = |dataset_id: &str, position: usize| ColumnSource {
            dataset_id: dataset_id.to_owned(),
            position,
        };
        scope
            .columns
            .insert("school".to_owned(), vec![src("a", 3), src("b", 0)]);
        scope.columns.insert("year".to_owned(), vec![src("b", 4)]);
        let run = |rows: Vec<FilterRow>| matching(&conn, &scope, &FilterSpec { rows });
        let b = |rows: &[i64]| -> Vec<(String, i64)> {
            rows.iter().map(|&r| ("b".to_owned(), r)).collect()
        };

        assert_eq!(
            run(vec![row(
                column("school"),
                FilterOp::Is,
                &["wharton", "ROSS"]
            )])?,
            [a(&[0, 1]), b(&[0, 1])].concat()
        );
        assert_eq!(
            run(vec![row(column("year"), FilterOp::IsEmpty, &[])])?,
            a(&[0, 1, 2])
        );
        assert_eq!(
            run(vec![row(FilterField::SourceDataset, FilterOp::Is, &["b"])])?,
            b(&[0, 1])
        );
        assert_eq!(
            run(vec![
                row(FilterField::SourceDataset, FilterOp::IsNot, &["b"]),
                row(FilterField::Subject, FilterOp::StartsWith, &["ac"]),
            ])?,
            a(&[0, 1])
        );
        Ok(())
    }

    /// Specs that can't mean anything are refused before any SQL exists.
    #[test]
    fn malformed_specs_are_refused() -> Result<(), String> {
        let one = Scope::for_dataset("a", None);
        let refuse = |scope: &Scope, r: FilterRow, why: &str| -> Result<(), String> {
            let err = compile(&FilterSpec { rows: vec![r] }, scope, |_| Some(1))
                .err()
                .ok_or_else(|| format!("compiled: {why}"))?;
            assert!(err.contains(why), "{err}");
            Ok(())
        };
        refuse(
            &one,
            row(FilterField::Subject, FilterOp::Is, &[" "]),
            "at least one value",
        )?;
        refuse(
            &one,
            row(FilterField::Subject, FilterOp::IsEmpty, &["x"]),
            "takes no values",
        )?;
        refuse(
            &one,
            row(column("school"), FilterOp::Is, &["x"]),
            "unknown column",
        )?;
        refuse(
            &one,
            row(FilterField::SourceDataset, FilterOp::Is, &["a"]),
            "several datasets",
        )?;
        refuse(
            &one,
            row(FilterField::Ccm { digit_level: 3 }, FilterOp::Is, &["5"]),
            "digit level",
        )?;
        refuse(
            &one,
            row(FilterField::Ccm { digit_level: 6 }, FilterOp::IsEmpty, &[]),
            "needs values",
        )?;
        let two = Scope {
            sources: vec!["a".to_owned(), "b".to_owned()],
            columns: BTreeMap::default(),
        };
        refuse(
            &two,
            row(FilterField::SourceDataset, FilterOp::Contains, &["a"]),
            "only is / is not",
        )?;
        refuse(
            &two,
            row(FilterField::SourceDataset, FilterOp::Is, &["zzz"]),
            "unknown source",
        )?;
        Ok(())
    }

    /// The value picker lists trimmed distinct values by frequency, narrowed
    /// by a search, and CCM codes with their titles.
    #[test]
    fn value_picker_lists_distinct_values() -> Result<(), String> {
        let conn = scratch()?;
        let layout = layout(
            &["SUBJ", "NUM", "TITLE", "SCHOOL"],
            ColumnMap {
                subject: 0,
                catalog: 1,
                title: 2,
            },
        )?;
        let scope = Scope::for_dataset("a", Some(&layout));
        let ask = |field: FilterField, search: &str| {
            values(
                &conn,
                &scope,
                &ColumnValuesRequest {
                    dataset_id: "a".to_owned(),
                    field,
                    search: search.to_owned(),
                },
                |level| (level == 6).then_some(6),
            )
        };
        let flat = |found: Vec<super::ColumnValue>| -> Vec<(String, i64, Option<String>)> {
            found
                .into_iter()
                .map(|v| (v.value, v.count, v.label))
                .collect()
        };

        assert_eq!(
            flat(ask(FilterField::Subject, "")?),
            vec![
                ("ACCT".to_owned(), 1, None),
                ("MATH".to_owned(), 1, None),
                ("acct".to_owned(), 1, None),
            ]
        );
        assert_eq!(
            flat(ask(column("school"), "ros")?),
            vec![("Ross".to_owned(), 2, None)]
        );
        assert_eq!(
            flat(ask(FilterField::Ccm { digit_level: 6 }, "account")?),
            vec![("52.0301".to_owned(), 1, Some("Accounting".to_owned()))]
        );
        Ok(())
    }
}
