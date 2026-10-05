//! Derived datasets (#254): a new dataset made of the rows of one or more
//! source datasets that match a filter, with chosen columns. Copies, not
//! views: the rows go into `courses` under the new `dataset_id` by one
//! `INSERT … SELECT`, `content_hash` carried over, so every per-dataset
//! path (paging, coverage, classification, export, delete) works unchanged
//! and cached results apply at once. `dataset_sources` records where the
//! rows came from; the new dataset never depends on its sources again.
//!
//! Columns (decision 2026-10-05): the three mapped columns always carry
//! over under names the user picks; every other column of the new dataset
//! is an [`OutputColumn`], fed by at most one column of each source.
//! Header-key matching ([`derived_columns`]) only fills the defaults; the
//! user renames, drops and merges (two sources' columns under one name).
//! A merge of several sources always adds a `source_dataset` column.
//! Duplicates are whatever the user says they are: rows equal in the
//! chosen `dedupe_columns` collapse to the earliest in source order.
//!
//! Built once: nothing re-runs when a source changes or is deleted.

use std::collections::{BTreeMap, BTreeSet};

use chrono::Utc;
use duckdb::{OptionalExt as _, params, types::Value};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, State};
use uuid::Uuid;

use crate::{
    activity::Activity,
    boot::{self, Boot, Services},
    db::AppDb,
    filter::{self, ColumnSource, FilterSpec, Scope, cell_sql, column_keys, column_sql},
    import,
    layout::Layout,
    preflight::ColumnMap,
    profile::{InputProfile, InputProfiler},
};

/// The column a merge adds, holding each row's source dataset title.
const SOURCE_COLUMN: &str = "source_dataset";
const PREVIEW_ROWS: i64 = 50;
const MAX_SOURCES: usize = 32;

/// One source column feeding an output column.
#[derive(Type, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ColumnOrigin {
    /// Source dataset id.
    pub source: String,
    /// Position in that source's layout.
    pub position: usize,
    /// That position's header, for display; the position is what counts.
    pub header: String,
}

/// One column of the new dataset.
#[derive(Type, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OutputColumn {
    pub name: String,
    /// At most one per source; a source not listed reads the column empty.
    pub from: Vec<ColumnOrigin>,
}

/// Names of the three mapped columns in the new dataset.
#[derive(Type, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MappedNames {
    pub subject: String,
    pub catalog: String,
    pub title: String,
}

impl Default for MappedNames {
    fn default() -> Self {
        Self {
            subject: "subject_code".to_owned(),
            catalog: "catalog_number".to_owned(),
            title: "course_title".to_owned(),
        }
    }
}

#[derive(Type, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeriveRequest {
    pub title: String,
    /// One or more ready datasets with a stored layout, in order; that order
    /// is the row order of the result.
    pub sources: Vec<String>,
    pub filter: FilterSpec,
    pub mapped_names: MappedNames,
    /// The kept extra columns, in order.
    pub columns: Vec<OutputColumn>,
    /// Output column names that define a duplicate; `None` keeps every row.
    pub dedupe_columns: Option<Vec<String>>,
}

/// Default columns for a set of sources, as the dialog first shows them.
#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DerivedColumns {
    pub mapped: MappedNames,
    pub columns: Vec<OutputColumn>,
}

#[derive(Type, Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceCount {
    pub source: String,
    pub matched: i64,
}

/// What a request would build: the match counts and the first rows under
/// the new dataset's headers, before duplicates are removed.
#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DerivePreview {
    pub matched: i64,
    pub by_source: Vec<SourceCount>,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
}

#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DerivedStarted {
    pub dataset_id: String,
}

#[derive(Type, Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceLink {
    pub id: String,
    /// The source's title when the dataset was built.
    pub title: String,
    /// Whether the source still exists.
    pub exists: bool,
}

/// How a derived dataset was built, for its page.
#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Derivation {
    pub sources: Vec<SourceLink>,
    pub filter: FilterSpec,
    pub dedupe_columns: Option<Vec<String>>,
}

/// A source dataset with what the build needs of it.
#[derive(Clone, Debug)]
struct Source {
    id: String,
    title: String,
    layout: Layout,
}

/// Load the sources of a request, in order. Each must exist, be `ready`
/// and have a stored layout.
fn load_sources(conn: &duckdb::Connection, ids: &[String]) -> Result<Vec<Source>, String> {
    if ids.is_empty() {
        return Err("Choose at least one dataset.".to_owned());
    }
    if ids.len() > MAX_SOURCES {
        return Err(format!("At most {MAX_SOURCES} datasets can be combined."));
    }
    let distinct: BTreeSet<&String> = ids.iter().collect();
    if distinct.len() != ids.len() {
        return Err("A dataset is listed twice.".to_owned());
    }
    ids.iter()
        .map(|id| {
            let row: Option<(String, Option<String>)> = conn
                .query_row(
                    "SELECT title, import_state FROM datasets WHERE id = ?",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|e| format!("dataset {id}: {e}"))?;
            let Some((title, state)) = row else {
                return Err("A chosen dataset no longer exists.".to_owned());
            };
            if state.as_deref().unwrap_or("ready") != "ready" {
                return Err(format!(
                    "\u{201c}{title}\u{201d} isn't ready. Wait for it to finish importing."
                ));
            }
            let Some(layout) = Layout::read(conn, id)? else {
                return Err(format!(
                    "\u{201c}{title}\u{201d} was imported before column layouts were stored. \
                     Import the file again to use it here."
                ));
            };
            Ok(Source {
                id: id.clone(),
                title,
                layout,
            })
        })
        .collect()
}

/// The default output columns: each source's non-mapped headers by key
/// (`filter::column_keys`), equal keys across sources merged into one
/// column named with the first source's spelling (plus ` #n` for a header
/// repeated within one file).
fn defaults(sources: &[Source]) -> DerivedColumns {
    let mut columns: Vec<OutputColumn> = Vec::new();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    for source in sources {
        for (position, key) in column_keys(&source.layout) {
            let header = source
                .layout
                .headers
                .get(position)
                .cloned()
                .unwrap_or_default();
            let origin = ColumnOrigin {
                source: source.id.clone(),
                position,
                header: header.clone(),
            };
            if let Some(&i) = index.get(&key) {
                if let Some(column) = columns.get_mut(i) {
                    column.from.push(origin);
                }
                continue;
            }
            // "notes #2" keeps its number in the name, so the defaults of a
            // file with repeated headers don't collide.
            let name = match key.rsplit_once(" #") {
                Some((_, n)) if n.chars().all(|c| c.is_ascii_digit()) => {
                    format!("{} #{n}", header.trim())
                }
                _ => header.trim().to_owned(),
            };
            index.insert(key, columns.len());
            columns.push(OutputColumn {
                name,
                from: vec![origin],
            });
        }
    }
    DerivedColumns {
        mapped: MappedNames::default(),
        columns,
    }
}

/// A kept extra column with its position in the new layout.
#[derive(Clone, Debug)]
struct Extra {
    name: String,
    position: usize,
    from: Vec<ColumnOrigin>,
}

/// A checked request: what the SQL is built from.
#[derive(Clone, Debug)]
struct Plan {
    sources: Vec<Source>,
    merge: bool,
    /// The new dataset's layout: mapped three, extras, `source_dataset`.
    layout: Layout,
    extras: Vec<Extra>,
    filter: FilterSpec,
    scope: Scope,
    dedupe: Option<Vec<String>>,
}

fn quoted(name: &str) -> String {
    format!("\u{201c}{name}\u{201d}")
}

/// The names of the new dataset's columns, in layout order: mapped three,
/// extras, `source_dataset` for a merge. Each trimmed and non-empty,
/// unique ignoring case, `source_dataset` reserved for the merge column.
fn check_names(req: &DeriveRequest, merge: bool) -> Result<Vec<String>, String> {
    let mut names: Vec<String> = [
        &req.mapped_names.subject,
        &req.mapped_names.catalog,
        &req.mapped_names.title,
    ]
    .into_iter()
    .chain(req.columns.iter().map(|c| &c.name))
    .map(|n| n.trim().to_owned())
    .collect();
    let user_named = names.len();
    if merge {
        names.push(SOURCE_COLUMN.to_owned());
    }
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (i, name) in names.iter().enumerate() {
        if name.is_empty() {
            return Err("Every column needs a name.".to_owned());
        }
        if i < user_named && name.eq_ignore_ascii_case(SOURCE_COLUMN) {
            return Err(format!(
                "{} is reserved for the column that says which dataset a row came from.",
                quoted(SOURCE_COLUMN)
            ));
        }
        if !seen.insert(name.to_lowercase()) {
            return Err(format!("Two columns are both named {}.", quoted(name)));
        }
    }
    Ok(names)
}

/// The kept extras with their new positions. Every origin is a real
/// non-mapped column of a listed source, used by one output column, and
/// no column takes two columns of one source.
fn check_origins(req: &DeriveRequest, sources: &[Source]) -> Result<Vec<Extra>, String> {
    let mut used: BTreeSet<(String, usize)> = BTreeSet::new();
    let mut extras = Vec::with_capacity(req.columns.len());
    for (i, column) in req.columns.iter().enumerate() {
        let name = column.name.trim().to_owned();
        if column.from.is_empty() {
            return Err(format!("{} has no source column.", quoted(&name)));
        }
        let mut per_source: BTreeSet<&str> = BTreeSet::new();
        for origin in &column.from {
            let Some(source) = sources.iter().find(|s| s.id == origin.source) else {
                return Err(format!(
                    "{} names a dataset that isn't a source.",
                    quoted(&name)
                ));
            };
            let mapping = source.layout.mapping;
            let is_mapped =
                [mapping.subject, mapping.catalog, mapping.title].contains(&origin.position);
            if origin.position >= source.layout.headers.len() || is_mapped {
                return Err(format!(
                    "{} names a column {} doesn't have.",
                    quoted(&name),
                    quoted(&source.title)
                ));
            }
            if !per_source.insert(source.id.as_str()) {
                return Err(format!(
                    "{} takes two columns from {}. A column can take one per dataset.",
                    quoted(&name),
                    quoted(&source.title)
                ));
            }
            if !used.insert((origin.source.clone(), origin.position)) {
                let header = source
                    .layout
                    .headers
                    .get(origin.position)
                    .map(String::as_str)
                    .unwrap_or_default();
                return Err(format!(
                    "{} of {} feeds two columns.",
                    quoted(header),
                    quoted(&source.title)
                ));
            }
        }
        extras.push(Extra {
            name,
            position: 3 + i,
            from: column.from.clone(),
        });
    }
    Ok(extras)
}

/// Check a request against its sources and lay out the new dataset
/// ([`check_names`], [`check_origins`], the duplicate key, the filter).
fn plan(sources: Vec<Source>, req: &DeriveRequest) -> Result<Plan, String> {
    let merge = sources.len() > 1;
    let names = check_names(req, merge)?;
    let extras = check_origins(req, &sources)?;
    let layout = Layout::new(
        names.clone(),
        ColumnMap {
            subject: 0,
            catalog: 1,
            title: 2,
        },
    )?;

    let dedupe = match &req.dedupe_columns {
        None => None,
        Some(columns) => {
            let chosen: Vec<String> = columns.iter().map(|c| c.trim().to_owned()).collect();
            if chosen.is_empty() {
                return Err("Choose at least one column that defines a duplicate.".to_owned());
            }
            if let Some(unknown) = chosen.iter().find(|c| !names.contains(c)) {
                return Err(format!(
                    "{} isn't a column of the new dataset.",
                    quoted(unknown)
                ));
            }
            Some(chosen)
        }
    };

    let scope = Scope {
        sources: sources.iter().map(|s| s.id.clone()).collect(),
        columns: extras
            .iter()
            .map(|e| {
                (
                    e.name.clone(),
                    e.from
                        .iter()
                        .map(|o| ColumnSource {
                            dataset_id: o.source.clone(),
                            position: o.position,
                        })
                        .collect(),
                )
            })
            .collect(),
    };
    filter::check(&req.filter, &scope)?;

    Ok(Plan {
        sources,
        merge,
        layout,
        extras,
        filter: req.filter.clone(),
        scope,
        dedupe,
    })
}

/// SQL text with the parameters its placeholders bind, in order.
#[derive(Debug, Default)]
struct Sql {
    text: String,
    params: Vec<Value>,
}

impl Sql {
    fn push(&mut self, text: &str) {
        self.text.push_str(text);
    }
}

impl Plan {
    /// `CASE c.dataset_id WHEN ? THEN 0 … END`: the row order across sources.
    fn source_position(&self, sql: &mut Sql) {
        if let [only] = self.sources.as_slice() {
            let _ = only;
            sql.push("0");
            return;
        }
        sql.push("CASE c.dataset_id");
        for (i, source) in self.sources.iter().enumerate() {
            sql.push(&format!(" WHEN ? THEN {i}"));
            sql.params.push(Value::Text(source.id.clone()));
        }
        sql.push(" END");
    }

    /// The new row's `extra_columns`: per source, a `json_object` of the
    /// kept extras it has (keyed by their new positions) and, for a merge,
    /// the source's title under `source_dataset`'s position. NULL for a
    /// source that contributes nothing.
    fn extras_object(&self, sql: &mut Sql) {
        let source_position = self.merge.then(|| self.layout.headers.len() - 1);
        let per_source: Vec<Option<Sql>> = self
            .sources
            .iter()
            .map(|source| {
                let mut entries: Vec<String> = Vec::new();
                let mut params: Vec<Value> = Vec::new();
                for extra in &self.extras {
                    if let Some(origin) = extra.from.iter().find(|o| o.source == source.id) {
                        entries.push(format!(
                            "'{}', {}",
                            extra.position,
                            cell_sql(origin.position)
                        ));
                    }
                }
                if let Some(position) = source_position {
                    entries.push(format!("'{position}', ?"));
                    params.push(Value::Text(source.title.clone()));
                }
                (!entries.is_empty()).then(|| Sql {
                    text: format!("json_object({})", entries.join(", ")),
                    params,
                })
            })
            .collect();
        if per_source.iter().all(Option::is_none) {
            sql.push("NULL");
            return;
        }
        if let [Some(only)] = per_source.as_slice() {
            sql.push(&only.text);
            sql.params.extend(only.params.iter().cloned());
            return;
        }
        sql.push("CASE c.dataset_id");
        for (source, object) in self.sources.iter().zip(&per_source) {
            if let Some(object) = object {
                sql.push(" WHEN ? THEN ");
                sql.params.push(Value::Text(source.id.clone()));
                sql.push(&object.text);
                sql.params.extend(object.params.iter().cloned());
            }
        }
        sql.push(" END");
    }

    /// The expression of one output column over alias `c`, for the
    /// duplicate key.
    fn column_expr(&self, name: &str, sql: &mut Sql) -> Result<(), String> {
        let mapped = &self.layout.headers;
        if mapped.first().is_some_and(|n| n == name) {
            sql.push("c.subject_code");
        } else if mapped.get(1).is_some_and(|n| n == name) {
            sql.push("c.catalog_number");
        } else if mapped.get(2).is_some_and(|n| n == name) {
            sql.push("c.course_title");
        } else if self.merge && name == SOURCE_COLUMN {
            sql.push("c.dataset_id");
        } else {
            let (text, params) = column_sql(&self.scope, name)?;
            sql.push(&text);
            sql.params.extend(params);
        }
        Ok(())
    }

    /// The matching rows with the new extras assembled, ordered by source
    /// then original row; with `dedupe`, one row per distinct duplicate key.
    fn select(
        &self,
        model_for_level: impl Fn(u8) -> Option<i64>,
        dedupe: bool,
    ) -> Result<Sql, String> {
        let mut sql = Sql::default();
        sql.push(
            "SELECT c.subject_code, c.catalog_number, c.course_title, c.content_hash,
                    c.row_index, ",
        );
        self.source_position(&mut sql);
        sql.push(" AS src_pos, ");
        self.extras_object(&mut sql);
        sql.push(" AS extras FROM courses c WHERE c.dataset_id IN (");
        sql.push(
            &self
                .sources
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(", "),
        );
        sql.push(")");
        sql.params
            .extend(self.sources.iter().map(|s| Value::Text(s.id.clone())));
        let compiled = filter::compile(&self.filter, &self.scope, model_for_level)?;
        sql.push(&compiled.sql);
        sql.params.extend(compiled.params);
        if let (true, Some(columns)) = (dedupe, &self.dedupe) {
            sql.push(" QUALIFY row_number() OVER (PARTITION BY ");
            for (i, name) in columns.iter().enumerate() {
                if i > 0 {
                    sql.push(", ");
                }
                self.column_expr(name, &mut sql)?;
            }
            sql.push(" ORDER BY src_pos, c.row_index) = 1");
        }
        Ok(sql)
    }

    /// The copy: every selected row under the new dataset id, numbered
    /// from 0 in source-then-row order.
    fn insert(
        &self,
        dataset_id: &str,
        model_for_level: impl Fn(u8) -> Option<i64>,
    ) -> Result<Sql, String> {
        let inner = self.select(model_for_level, true)?;
        let mut sql = Sql {
            text: String::new(),
            params: vec![Value::Text(dataset_id.to_owned())],
        };
        sql.push(
            "INSERT INTO courses
                (dataset_id, row_index, subject_code, catalog_number, course_title,
                 content_hash, extra_columns)
             SELECT ?, row_number() OVER (ORDER BY src_pos, row_index) - 1,
                    subject_code, catalog_number, course_title, content_hash, extras
             FROM (",
        );
        sql.push(&inner.text);
        sql.params.extend(inner.params);
        sql.push(") t");
        Ok(sql)
    }

    fn preview(
        &self,
        conn: &duckdb::Connection,
        model_for_level: impl Fn(u8) -> Option<i64>,
    ) -> Result<DerivePreview, String> {
        // Per-source match counts, in source order (0 where nothing matched).
        let mut counts = Sql::default();
        counts.push("SELECT c.dataset_id, COUNT(*) FROM courses c WHERE c.dataset_id IN (");
        counts.push(
            &self
                .sources
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(", "),
        );
        counts.push(")");
        counts
            .params
            .extend(self.sources.iter().map(|s| Value::Text(s.id.clone())));
        let compiled = filter::compile(&self.filter, &self.scope, &model_for_level)?;
        counts.push(&compiled.sql);
        counts.params.extend(compiled.params);
        counts.push(" GROUP BY c.dataset_id");
        let mut stmt = conn
            .prepare(&counts.text)
            .map_err(|e| format!("prepare preview counts: {e}"))?;
        let found: BTreeMap<String, i64> = stmt
            .query_map(duckdb::params_from_iter(counts.params.iter()), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| format!("preview counts: {e}"))?
            .collect::<Result<_, _>>()
            .map_err(|e| format!("preview counts: {e}"))?;
        let by_source: Vec<SourceCount> = self
            .sources
            .iter()
            .map(|s| SourceCount {
                source: s.id.clone(),
                matched: found.get(&s.id).copied().unwrap_or(0),
            })
            .collect();
        let matched = by_source.iter().map(|s| s.matched).sum();

        let inner = self.select(model_for_level, false)?;
        let mut rows_sql = Sql::default();
        rows_sql.push("SELECT subject_code, catalog_number, course_title, extras::VARCHAR FROM (");
        rows_sql.push(&inner.text);
        rows_sql.params.extend(inner.params);
        rows_sql.push(&format!(
            ") t ORDER BY src_pos, row_index LIMIT {PREVIEW_ROWS}"
        ));
        let mut stmt = conn
            .prepare(&rows_sql.text)
            .map_err(|e| format!("prepare preview rows: {e}"))?;
        let width = self.layout.headers.len();
        let rows = stmt
            .query_map(duckdb::params_from_iter(rows_sql.params.iter()), |row| {
                let mut cells: Vec<Option<String>> = Vec::with_capacity(width);
                cells.push(row.get(0)?);
                cells.push(row.get(1)?);
                cells.push(row.get(2)?);
                let extras: Option<String> = row.get(3)?;
                let object: serde_json::Map<String, serde_json::Value> = extras
                    .and_then(|json| serde_json::from_str(&json).ok())
                    .unwrap_or_default();
                for position in 3..width {
                    cells.push(
                        object
                            .get(&position.to_string())
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned),
                    );
                }
                Ok(cells)
            })
            .map_err(|e| format!("preview rows: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("preview rows: {e}"))?;

        Ok(DerivePreview {
            matched,
            by_source,
            headers: self.layout.headers.clone(),
            rows,
        })
    }
}

/// The default columns for a set of sources (section "Columns" of the
/// dialog). Read-only.
#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn derived_columns(
    sources: Vec<String>,
    boot: State<'_, Boot>,
) -> Result<DerivedColumns, String> {
    let conn = boot.ready()?.db.ro()?;
    Ok(defaults(&load_sources(&conn, &sources)?))
}

/// What a request would build. `title` and `dedupe_columns` are checked
/// but the counts and rows are before duplicates are removed. Read-only.
#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn preview_derivation(
    req: DeriveRequest,
    boot: State<'_, Boot>,
) -> Result<DerivePreview, String> {
    let Services { db, catalog, .. } = boot.ready()?;
    let conn = db.ro()?;
    let plan = plan(load_sources(&conn, &req.sources)?, &req)?;
    plan.preview(&conn, |level| catalog.model_id(level))
}

/// Start building a derived dataset. Like `import_csv`, returns as soon as
/// the dataset row exists, in `importing` state, and a worker copies the
/// rows; the datasets list shows it fill. A failure marks it `failed` with
/// the reason; a process death leaves `importing`, which the startup sweep
/// turns into `failed` like any import.
#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn create_derived_dataset(
    req: DeriveRequest,
    app: AppHandle,
    boot: State<'_, Boot>,
    activity: State<'_, Activity>,
) -> Result<DerivedStarted, String> {
    let Services { db, catalog, .. } = boot.ready()?;
    // Not during a delete, prune or compaction (activity.rs): checked here
    // so the call fails at once, and again under the write lock below.
    activity.ensure_idle()?;
    let title = req.title.trim().to_owned();
    if title.is_empty() {
        return Err("Give the new dataset a title.".to_owned());
    }
    let dataset_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();

    let plan = {
        let conn = db.rw()?;
        activity.ensure_idle()?;
        let plan = plan(load_sources(&conn, &req.sources)?, &req)?;
        // Compile once now, so a filter this build can't run (a CCM level
        // with no active model) is refused before any row exists.
        plan.select(|level| catalog.model_id(level), true)?;
        let filter_json =
            serde_json::to_string(&plan.filter).map_err(|e| format!("serialize filter: {e}"))?;
        let dedupe_json = plan
            .dedupe
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| format!("serialize dedupe columns: {e}"))?;
        conn.execute(
            "INSERT INTO datasets
                (id, title, source_kind, imported_at, row_count, import_state, classify_state,
                 filter_spec, layout, dedupe_columns)
             VALUES (?, ?, 'derived', ?, 0, 'importing', 'idle', ?, ?, ?)",
            params![
                &dataset_id,
                &title,
                &now,
                &filter_json,
                plan.layout.to_json()?,
                dedupe_json,
            ],
        )
        .map_err(|e| format!("insert datasets: {e}"))?;
        for (i, source) in plan.sources.iter().enumerate() {
            conn.execute(
                "INSERT INTO dataset_sources
                    (dataset_id, position, source_dataset_id, source_title)
                 VALUES (?, ?, ?, ?)",
                params![
                    &dataset_id,
                    i32::try_from(i).unwrap_or(i32::MAX),
                    &source.id,
                    &source.title
                ],
            )
            .map_err(|e| format!("insert dataset_sources: {e}"))?;
        }
        plan
    };

    let id = dataset_id.clone();
    tauri::async_runtime::spawn_blocking(move || match boot::services(&app) {
        Ok(services) => build(&services.db, &id, &plan, |level| {
            services.catalog.model_id(level)
        }),
        Err(e) => log::error!("derive {id}: {e}"),
    });
    Ok(DerivedStarted { dataset_id })
}

/// The worker: copy the rows, profile them, mark the dataset ready.
fn build(db: &AppDb, dataset_id: &str, plan: &Plan, model_for_level: impl Fn(u8) -> Option<i64>) {
    let outcome = copy_rows(db, dataset_id, plan, model_for_level)
        .and_then(|rows| profile(db, dataset_id, plan).map(|profile| (rows, profile)));
    match outcome {
        Ok((rows, profile)) => {
            log::info!("derive {dataset_id}: copied {rows} row(s)");
            import::mark_ready(db, dataset_id, rows, &profile);
        }
        Err(e) => {
            log::error!("derive {dataset_id}: {e}");
            import::mark_failed(db, dataset_id, &e);
        }
    }
}

fn copy_rows(
    db: &AppDb,
    dataset_id: &str,
    plan: &Plan,
    model_for_level: impl Fn(u8) -> Option<i64>,
) -> Result<u64, String> {
    let sql = plan.insert(dataset_id, model_for_level)?;
    let conn = db.rw()?;
    let rows = conn
        .execute(&sql.text, duckdb::params_from_iter(sql.params.iter()))
        .map_err(|e| format!("copy rows: {e}"))?;
    Ok(u64::try_from(rows).unwrap_or(u64::MAX))
}

/// The input profile of the copied rows (profile.rs), so the new dataset's
/// page has its own input check. Rows are read in order through the read
/// connection; the stored values were trimmed at import.
fn profile(db: &AppDb, dataset_id: &str, plan: &Plan) -> Result<InputProfile, String> {
    let headers = plan.layout.headers.clone();
    let [subject, catalog, title] = [
        headers.first().cloned().unwrap_or_default(),
        headers.get(1).cloned().unwrap_or_default(),
        headers.get(2).cloned().unwrap_or_default(),
    ];
    let mut profiler = InputProfiler::new([subject, catalog, title]);
    let conn = db.ro()?;
    let mut stmt = conn
        .prepare(
            "SELECT row_index, subject_code, catalog_number, course_title
             FROM courses WHERE dataset_id = ? ORDER BY row_index",
        )
        .map_err(|e| format!("prepare profile scan: {e}"))?;
    let rows = stmt
        .query_map([dataset_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(|e| format!("profile scan: {e}"))?;
    for row in rows {
        let (row_index, subject, catalog, title) = row.map_err(|e| format!("profile scan: {e}"))?;
        // Spreadsheet-style row numbers, 1-based, as the import reports them.
        let row_number = u64::try_from(row_index).unwrap_or(0) + 1;
        profiler.observe(
            row_number,
            [
                subject.as_deref().unwrap_or_default(),
                catalog.as_deref().unwrap_or_default(),
                title.as_deref().unwrap_or_default(),
            ],
        );
    }
    Ok(profiler.finish())
}

/// How a derived dataset was built; `None` for a dataset that isn't one.
#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn get_derivation(
    dataset_id: String,
    boot: State<'_, Boot>,
) -> Result<Option<Derivation>, String> {
    derivation(&*boot.ready()?.db.ro()?, &dataset_id)
}

fn derivation(conn: &duckdb::Connection, dataset_id: &str) -> Result<Option<Derivation>, String> {
    let row: Option<(String, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT source_kind, filter_spec::VARCHAR, dedupe_columns::VARCHAR
             FROM datasets WHERE id = ?",
            [dataset_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|e| format!("dataset {dataset_id}: {e}"))?;
    let Some((kind, filter_json, dedupe_json)) = row else {
        return Ok(None);
    };
    if kind != "derived" {
        return Ok(None);
    }
    let filter: FilterSpec = filter_json
        .map(|j| serde_json::from_str(&j).map_err(|e| format!("parse stored filter: {e}")))
        .transpose()?
        .unwrap_or_default();
    let dedupe_columns: Option<Vec<String>> = dedupe_json
        .map(|j| serde_json::from_str(&j).map_err(|e| format!("parse stored dedupe columns: {e}")))
        .transpose()?;
    let mut stmt = conn
        .prepare(
            "SELECT s.source_dataset_id, s.source_title, d.id IS NOT NULL
             FROM dataset_sources s
             LEFT JOIN datasets d ON d.id = s.source_dataset_id
             WHERE s.dataset_id = ?
             ORDER BY s.position",
        )
        .map_err(|e| format!("prepare sources of {dataset_id}: {e}"))?;
    let sources = stmt
        .query_map([dataset_id], |row| {
            Ok(SourceLink {
                id: row.get(0)?,
                title: row.get(1)?,
                exists: row.get(2)?,
            })
        })
        .map_err(|e| format!("sources of {dataset_id}: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("sources of {dataset_id}: {e}"))?;
    Ok(Some(Derivation {
        sources,
        filter,
        dedupe_columns,
    }))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        ColumnOrigin, DeriveRequest, MappedNames, OutputColumn, SOURCE_COLUMN, SourceCount, build,
        defaults, derivation, load_sources, plan,
    };
    use crate::{
        boot::Progress,
        db::AppDb,
        filter::{FilterField, FilterOp, FilterRow, FilterSpec},
        layout::Layout,
        preflight::ColumnMap,
    };

    /// Two file datasets with layouts: `a` (SUBJ, NUM, TITLE, SCHOOL) and
    /// `b` (School, `sub_pref`, course, ttl, YEAR), plus `old` with no layout.
    fn seeded(name: &str) -> Result<AppDb, String> {
        let root: PathBuf =
            std::env::temp_dir().join(format!("ccm-derive-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let db = AppDb::open_at(root.join("app.duckdb"), "test", &Progress::none())?;
        let layout_a = Layout::new(
            vec!["SUBJ".into(), "NUM".into(), "TITLE".into(), "SCHOOL".into()],
            ColumnMap {
                subject: 0,
                catalog: 1,
                title: 2,
            },
        )?
        .to_json()?;
        let layout_b = Layout::new(
            vec![
                "School".into(),
                "sub_pref".into(),
                "course".into(),
                "ttl".into(),
                "YEAR".into(),
            ],
            ColumnMap {
                subject: 1,
                catalog: 2,
                title: 3,
            },
        )?
        .to_json()?;
        let conn = db.rw()?;
        conn.execute(
            "INSERT INTO datasets
                (id, title, source_kind, imported_at, row_count, import_state, layout)
             VALUES ('a', 'A', 'file', now(), 3, 'ready', ?),
                    ('b', 'B', 'file', now(), 2, 'ready', ?),
                    ('old', 'Old', 'file', now(), 0, 'ready', NULL)",
            [&layout_a, &layout_b],
        )
        .map_err(|e| e.to_string())?;
        conn.execute_batch(
            "INSERT INTO courses
                (dataset_id, row_index, subject_code, catalog_number, course_title,
                 content_hash, extra_columns)
             VALUES ('a', 0, 'ACCT', '101', 'Intro', 'h0', '{\"3\":\"Ross\"}'),
                    ('a', 1, 'ACCT', '101', 'Intro', 'h0', '{\"3\":\"Ross\"}'),
                    ('a', 2, 'MATH', '101', 'Calc', 'h1', '{\"3\":\"Ross\"}'),
                    ('b', 0, 'ACC', '201', 'Tax', 'h2', '{\"0\":\"Wharton\",\"4\":\"2024\"}'),
                    ('b', 1, 'ACCT', '101', 'Intro', 'h0', '{\"0\":\"Wharton\",\"4\":\"2024\"}')",
        )
        .map_err(|e| e.to_string())?;
        drop(conn);
        Ok(db)
    }

    fn origin(source: &str, position: usize, header: &str) -> ColumnOrigin {
        ColumnOrigin {
            source: source.to_owned(),
            position,
            header: header.to_owned(),
        }
    }

    fn request(sources: &[&str], columns: Vec<OutputColumn>) -> DeriveRequest {
        DeriveRequest {
            title: "New".to_owned(),
            sources: sources.iter().map(|&s| s.to_owned()).collect(),
            filter: FilterSpec::default(),
            mapped_names: MappedNames::default(),
            columns,
            dedupe_columns: None,
        }
    }

    fn school_merged() -> OutputColumn {
        OutputColumn {
            name: "school".to_owned(),
            from: vec![origin("a", 3, "SCHOOL"), origin("b", 0, "School")],
        }
    }

    fn year() -> OutputColumn {
        OutputColumn {
            name: "year".to_owned(),
            from: vec![origin("b", 4, "YEAR")],
        }
    }

    fn rows(db: &AppDb, dataset_id: &str) -> Result<Vec<(i64, String, Option<String>)>, String> {
        let conn = db.rw()?;
        let mut stmt = conn
            .prepare(
                "SELECT row_index, subject_code, extra_columns::VARCHAR
                 FROM courses WHERE dataset_id = ? ORDER BY row_index",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([dataset_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }

    /// Default columns match headers by key across sources, in first-seen
    /// order, naming each with the first source's spelling.
    #[test]
    fn defaults_match_headers_across_sources() -> Result<(), String> {
        let db = seeded("defaults")?;
        let sources = load_sources(&*db.rw()?, &["a".to_owned(), "b".to_owned()])?;
        let found = defaults(&sources);
        assert_eq!(found.mapped, MappedNames::default());
        assert_eq!(
            found.columns,
            vec![
                OutputColumn {
                    name: "SCHOOL".to_owned(),
                    from: vec![origin("a", 3, "SCHOOL"), origin("b", 0, "School")],
                },
                OutputColumn {
                    name: "YEAR".to_owned(),
                    from: vec![origin("b", 4, "YEAR")],
                },
            ]
        );
        Ok(())
    }

    /// A source that isn't ready, is listed twice, or has no layout is
    /// refused with a reason that names it.
    #[test]
    fn sources_are_checked() -> Result<(), String> {
        let db = seeded("sources")?;
        let conn = db.rw()?;
        let refuse = |ids: &[&str], why: &str| -> Result<(), String> {
            let ids: Vec<String> = ids.iter().map(|&s| s.to_owned()).collect();
            let err = load_sources(&conn, &ids).err().ok_or("loaded")?;
            assert!(err.contains(why), "{err}");
            Ok(())
        };
        refuse(&[], "at least one")?;
        refuse(&["a", "a"], "listed twice")?;
        refuse(&["a", "missing"], "no longer exists")?;
        refuse(&["old"], "before column layouts were stored")?;
        conn.execute_batch("UPDATE datasets SET import_state = 'importing' WHERE id = 'b'")
            .map_err(|e| e.to_string())?;
        refuse(&["b"], "isn't ready")?;
        Ok(())
    }

    /// Names must be unique and non-empty, `source_dataset` is reserved,
    /// and origins must be real, unmapped, used once and one per source.
    #[test]
    fn column_rules_are_enforced() -> Result<(), String> {
        let db = seeded("rules")?;
        let refuse = |req: DeriveRequest, why: &str| -> Result<(), String> {
            let sources = load_sources(&*db.rw()?, &req.sources)?;
            let err = plan(sources, &req)
                .err()
                .ok_or_else(|| format!("planned: {why}"))?;
            assert!(err.contains(why), "{err}");
            Ok(())
        };
        let named = |name: &str, from: Vec<ColumnOrigin>| OutputColumn {
            name: name.to_owned(),
            from,
        };

        refuse(
            request(
                &["a", "b"],
                vec![named("Subject_Code", vec![origin("a", 3, "SCHOOL")])],
            ),
            "both named",
        )?;
        refuse(
            request(
                &["a", "b"],
                vec![named(" ", vec![origin("a", 3, "SCHOOL")])],
            ),
            "needs a name",
        )?;
        refuse(
            request(
                &["a", "b"],
                vec![named("Source_Dataset", vec![origin("a", 3, "SCHOOL")])],
            ),
            "reserved",
        )?;
        refuse(
            request(&["a"], vec![named("x", vec![])]),
            "no source column",
        )?;
        refuse(
            request(&["a"], vec![named("x", vec![origin("b", 0, "School")])]),
            "isn't a source",
        )?;
        refuse(
            request(&["a"], vec![named("x", vec![origin("a", 0, "SUBJ")])]),
            "doesn't have",
        )?;
        refuse(
            request(&["a"], vec![named("x", vec![origin("a", 9, "?")])]),
            "doesn't have",
        )?;
        refuse(
            request(
                &["b"],
                vec![named(
                    "x",
                    vec![origin("b", 0, "School"), origin("b", 4, "YEAR")],
                )],
            ),
            "takes two columns",
        )?;
        refuse(
            request(
                &["a"],
                vec![
                    named("x", vec![origin("a", 3, "SCHOOL")]),
                    named("y", vec![origin("a", 3, "SCHOOL")]),
                ],
            ),
            "feeds two columns",
        )?;
        let mut req = request(&["a"], vec![]);
        req.dedupe_columns = Some(vec!["nope".to_owned()]);
        refuse(req, "isn't a column")?;
        let mut req = request(&["a"], vec![]);
        req.dedupe_columns = Some(vec![]);
        refuse(req, "at least one column")?;
        Ok(())
    }

    /// A merge of two sources: rows in source order, extras re-keyed to the
    /// new layout (empty where a source lacks the column), `source_dataset`
    /// holding each row's source title, the filter applied, and the preview
    /// agreeing with the copy.
    #[test]
    fn merge_copies_rows_under_the_new_layout() -> Result<(), String> {
        let db = seeded("merge")?;
        let mut req = request(&["a", "b"], vec![school_merged(), year()]);
        req.filter = FilterSpec {
            rows: vec![FilterRow {
                field: FilterField::Subject,
                op: FilterOp::StartsWith,
                values: vec!["ac".to_owned()],
            }],
        };
        let sources = load_sources(&*db.rw()?, &req.sources)?;
        let plan = plan(sources, &req)?;
        assert_eq!(
            plan.layout.headers,
            [
                "subject_code",
                "catalog_number",
                "course_title",
                "school",
                "year",
                SOURCE_COLUMN
            ]
        );

        let preview = plan.preview(&*db.rw()?, |_| None)?;
        assert_eq!(preview.matched, 4);
        assert_eq!(
            preview.by_source,
            vec![
                SourceCount {
                    source: "a".to_owned(),
                    matched: 2
                },
                SourceCount {
                    source: "b".to_owned(),
                    matched: 2
                },
            ]
        );
        assert_eq!(preview.headers, plan.layout.headers);
        assert_eq!(preview.rows.len(), 4);
        assert_eq!(
            preview.rows.first().map(Vec::as_slice),
            Some(
                [
                    Some("ACCT".to_owned()),
                    Some("101".to_owned()),
                    Some("Intro".to_owned()),
                    Some("Ross".to_owned()),
                    None,
                    Some("A".to_owned()),
                ]
                .as_slice()
            )
        );

        db.rw()?
            .execute_batch(
                "INSERT INTO datasets (id, title, source_kind, imported_at, row_count, import_state)
                 VALUES ('m', 'M', 'derived', now(), 0, 'importing')",
            )
            .map_err(|e| e.to_string())?;
        build(&db, "m", &plan, |_| None);
        let copied = rows(&db, "m")?;
        assert_eq!(
            copied,
            vec![
                (
                    0,
                    "ACCT".to_owned(),
                    Some("{\"3\":\"Ross\",\"5\":\"A\"}".to_owned())
                ),
                (
                    1,
                    "ACCT".to_owned(),
                    Some("{\"3\":\"Ross\",\"5\":\"A\"}".to_owned())
                ),
                (
                    2,
                    "ACC".to_owned(),
                    Some("{\"3\":\"Wharton\",\"4\":\"2024\",\"5\":\"B\"}".to_owned())
                ),
                (
                    3,
                    "ACCT".to_owned(),
                    Some("{\"3\":\"Wharton\",\"4\":\"2024\",\"5\":\"B\"}".to_owned())
                ),
            ]
        );
        let (state, count, profiled): (String, i64, bool) = db
            .rw()?
            .query_row(
                "SELECT import_state, row_count, input_profile IS NOT NULL
                 FROM datasets WHERE id = 'm'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(|e| e.to_string())?;
        assert_eq!((state.as_str(), count, profiled), ("ready", 4, true));
        Ok(())
    }

    /// Duplicates are the user's columns: by the mapped three alone, the
    /// repeated ACCT 101 rows collapse across sources to the first; with
    /// `source_dataset` in the key, only the within-source repeat goes.
    #[test]
    fn duplicates_follow_the_chosen_columns() -> Result<(), String> {
        let db = seeded("dedupe")?;
        let sources = load_sources(&*db.rw()?, &["a".to_owned(), "b".to_owned()])?;
        let subjects = |dedupe: Vec<&str>, id: &str| -> Result<Vec<String>, String> {
            let mut req = request(&["a", "b"], vec![school_merged()]);
            req.dedupe_columns = Some(dedupe.iter().map(|&s| s.to_owned()).collect());
            let plan = plan(sources.clone(), &req)?;
            db.rw()?
                .execute(
                    "INSERT INTO datasets (id, title, source_kind, imported_at, row_count, import_state)
                     VALUES (?, 'D', 'derived', now(), 0, 'importing')",
                    [id],
                )
                .map_err(|e| e.to_string())?;
            build(&db, id, &plan, |_| None);
            Ok(rows(&db, id)?.into_iter().map(|(_, s, _)| s).collect())
        };
        assert_eq!(
            subjects(vec!["subject_code", "catalog_number", "course_title"], "d1")?,
            ["ACCT", "MATH", "ACC"]
        );
        assert_eq!(
            subjects(
                vec![
                    "subject_code",
                    "catalog_number",
                    "course_title",
                    SOURCE_COLUMN
                ],
                "d2"
            )?,
            ["ACCT", "MATH", "ACC", "ACCT"]
        );
        assert_eq!(subjects(vec!["school"], "d3")?, ["ACCT", "ACC"]);
        Ok(())
    }

    /// A subset of one source keeps its chosen extras under new positions
    /// and no `source_dataset`; a source with no kept extras writes NULL.
    #[test]
    fn subset_of_one_source() -> Result<(), String> {
        let db = seeded("subset")?;
        let req = request(
            &["b"],
            vec![OutputColumn {
                name: "Year".to_owned(),
                from: vec![origin("b", 4, "YEAR")],
            }],
        );
        let plan = plan(load_sources(&*db.rw()?, &req.sources)?, &req)?;
        assert_eq!(
            plan.layout.headers,
            ["subject_code", "catalog_number", "course_title", "Year"]
        );
        db.rw()?
            .execute_batch(
                "INSERT INTO datasets (id, title, source_kind, imported_at, row_count, import_state)
                 VALUES ('s', 'S', 'derived', now(), 0, 'importing'),
                        ('n', 'N', 'derived', now(), 0, 'importing')",
            )
            .map_err(|e| e.to_string())?;
        build(&db, "s", &plan, |_| None);
        assert_eq!(
            rows(&db, "s")?,
            vec![
                (0, "ACC".to_owned(), Some("{\"3\":\"2024\"}".to_owned())),
                (1, "ACCT".to_owned(), Some("{\"3\":\"2024\"}".to_owned())),
            ]
        );
        let bare = plan_for(&db, &request(&["b"], vec![]))?;
        build(&db, "n", &bare, |_| None);
        assert_eq!(
            rows(&db, "n")?,
            vec![(0, "ACC".to_owned(), None), (1, "ACCT".to_owned(), None)]
        );
        Ok(())
    }

    fn plan_for(db: &AppDb, req: &DeriveRequest) -> Result<super::Plan, String> {
        plan(load_sources(&*db.rw()?, &req.sources)?, req)
    }

    /// The stored derivation reads back with its sources, including one
    /// that has since been deleted; a file dataset has none.
    #[test]
    fn derivation_reads_back_after_a_source_is_deleted() -> Result<(), String> {
        let db = seeded("derivation")?;
        db.rw()?
            .execute_batch(
                "INSERT INTO datasets
                    (id, title, source_kind, imported_at, row_count, import_state, filter_spec,
                     dedupe_columns)
                 VALUES ('m', 'M', 'derived', now(), 0, 'ready',
                         '{\"rows\":[{\"field\":{\"kind\":\"subject\"},\"op\":\"is\",\"values\":[\"ACCT\"]}]}',
                         '[\"subject_code\"]');
                 INSERT INTO dataset_sources VALUES ('m', 0, 'a', 'A'), ('m', 1, 'b', 'B');
                 DELETE FROM courses WHERE dataset_id = 'b';
                 DELETE FROM datasets WHERE id = 'b';",
            )
            .map_err(|e| e.to_string())?;
        let conn = db.rw()?;
        let found = derivation(&conn, "m")?.ok_or("no derivation")?;
        assert_eq!(
            found
                .sources
                .iter()
                .map(|s| (s.title.as_str(), s.exists))
                .collect::<Vec<_>>(),
            [("A", true), ("B", false)]
        );
        assert_eq!(found.filter.rows.len(), 1);
        assert_eq!(found.dedupe_columns, Some(vec!["subject_code".to_owned()]));
        assert!(derivation(&conn, "a")?.is_none());
        assert!(derivation(&conn, "missing")?.is_none());
        Ok(())
    }
}
