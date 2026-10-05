//! Classifying a dataset (#249). `classify_dataset` marks the dataset
//! `running`, registers a [`Job`], and runs the batched inference loop on a
//! blocking task. There is no job record: results are cached by
//! `(model_id, content_hash)`, so the cache is the saved progress, and what
//! the dataset keeps is its state (`datasets.classify_state`, `idle |
//! running | stopped | failed`), the last error, and the execution provider.
//! Classifying a stopped dataset again is the resume: the cache check skips
//! everything already done.
//!
//! Live progress is in memory, on the [`Job`]; `list_datasets` attaches it to
//! the running dataset's row, and the frontend polls that list.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
};

use chrono::Utc;
use duckdb::{OptionalExt as _, params};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager, State};

use crate::{
    activity::Activity,
    boot::{self, Boot, Services},
    db::AppDb,
    format::{CourseInput, format_input},
    inference::{LoadedModel, ModelStore, classify_batch},
};

/// Rows per DB round-trip in the classify worker (EPI-89): one cache
/// anti-join window read and one Appender insert per super-chunk of this
/// many distinct inputs — the "flush ~1000 results" write-batching ground
/// rule. Deliberately independent of `inference::batch_size` (the ONNX/GPU
/// granularity inside a super-chunk): coupling them left the GPU idle on DB
/// round-trips two-thirds of the time. Public so `check_resume` can derive
/// its kill threshold from the real flush cadence instead of drifting.
pub const FLUSH_SIZE: usize = 1024;

/// Compact the WAL this often during a job (EPI-92), at a flushed super-chunk
/// boundary. 60 s of GPU-rate results is a few tens of MB of WAL — compaction
/// costs tens of ms (<0.1% of throughput); without it the WAL grows until app
/// exit and crash-recovery time scales with it.
const CHECKPOINT_INTERVAL: std::time::Duration = std::time::Duration::from_mins(1);

/// A dataset's classification state, stored as text in
/// `datasets.classify_state`. `running` never survives a launch: the startup
/// sweep turns it into `stopped`.
#[derive(Type, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ClassifyState {
    /// Not classifying. Coverage says how complete it is.
    Idle,
    Running,
    /// Stopped by the user, or by the app closing, before every model was
    /// done. Classifying again picks up where it left off.
    Stopped,
    Failed,
}

impl ClassifyState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        }
    }

    /// The stored text back to a state.
    pub fn parse(stored: &str) -> Result<Self, String> {
        match stored {
            "idle" => Ok(Self::Idle),
            "running" => Ok(Self::Running),
            "stopped" => Ok(Self::Stopped),
            "failed" => Ok(Self::Failed),
            other => Err(format!("unknown classify_state {other:?}")),
        }
    }
}

/// A dataset's classification, as `list_datasets` reports it.
#[derive(Type, Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Classification {
    pub state: ClassifyState,
    /// The last failure's message, while `state` is `failed`.
    pub error: Option<String>,
    /// Execution provider the latest job ran on.
    pub execution_provider: Option<String>,
    /// When the state last changed.
    pub updated_at: Option<String>,
    /// Only while a job is executing in this process.
    pub progress: Option<ClassifyProgress>,
}

/// Live progress of one job.
#[derive(Type, Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ClassifyProgress {
    /// One row per model, in the order they run.
    pub levels: Vec<LevelProgress>,
    /// A stop was requested; the worker is finishing its current batch.
    pub stopping: bool,
}

/// How many of the dataset's courses have a result for one model. Same units
/// as `get_classification_coverage`, so the card can switch between them.
#[derive(Type, Serialize, Clone, Copy, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LevelProgress {
    pub digit_level: u8,
    pub done: i64,
    pub total: i64,
}

/// One classification in flight. The worker writes progress at each flush;
/// readers take a snapshot. No progress rows until the worker has measured
/// what is already cached, so a resumed job never reads zero.
#[derive(Debug, Default)]
pub struct Job {
    cancel: AtomicBool,
    levels: Mutex<Vec<LevelProgress>>,
}

impl Job {
    /// Poisoning is benign: the progress rows are always consistent.
    fn lock(&self) -> MutexGuard<'_, Vec<LevelProgress>> {
        self.levels.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    fn stopping(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn set_levels(&self, levels: Vec<LevelProgress>) {
        *self.lock() = levels;
    }

    fn set_done(&self, index: usize, done: u64) {
        if let Some(level) = self.lock().get_mut(index) {
            level.done = i64::try_from(done).unwrap_or(i64::MAX);
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> ClassifyProgress {
        ClassifyProgress {
            levels: self.lock().clone(),
            stopping: self.stopping(),
        }
    }
}

/// The jobs executing in this process, keyed by dataset id. Membership is
/// the in-memory truth of "a worker is executing": a `running` row with no
/// job here is not active. Managed as Tauri state.
#[derive(Default)]
pub(crate) struct ClassifyRegistry {
    jobs: Mutex<HashMap<String, Arc<Job>>>,
}

impl ClassifyRegistry {
    /// Poisoning is benign: the only state is the job map, and losing the
    /// ability to stop a job over an unrelated panic is worse.
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Arc<Job>>> {
        self.jobs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Refused when the dataset already has a job.
    pub(crate) fn register(&self, dataset_id: &str, job: &Arc<Job>) -> Result<(), String> {
        let mut jobs = self.lock();
        if jobs.contains_key(dataset_id) {
            return Err("This dataset is already classifying.".to_owned());
        }
        jobs.insert(dataset_id.to_owned(), Arc::clone(job));
        Ok(())
    }

    /// Ask the dataset's job to stop. `false` when it has none.
    fn stop(&self, dataset_id: &str) -> bool {
        self.lock().get(dataset_id).is_some_and(|job| {
            job.stop();
            true
        })
    }

    /// Remove `job`, and only `job`: never a later job on the same dataset.
    pub(crate) fn remove(&self, dataset_id: &str, job: &Arc<Job>) {
        let mut jobs = self.lock();
        if jobs.get(dataset_id).is_some_and(|j| Arc::ptr_eq(j, job)) {
            jobs.remove(dataset_id);
        }
    }

    pub(crate) fn is_active(&self, dataset_id: &str) -> bool {
        self.lock().contains_key(dataset_id)
    }

    pub(crate) fn progress(&self, dataset_id: &str) -> Option<ClassifyProgress> {
        self.lock().get(dataset_id).map(|job| job.snapshot())
    }
}

/// Actionable "can't run inference yet" error, matching the loading state:
/// during the startup autoload no user action is needed; otherwise the
/// Models panel is the fix.
fn models_unready_message(store: &ModelStore) -> String {
    if store.is_loading() {
        "models are still loading — try again in a moment".to_owned()
    } else {
        "models not loaded — download/load them from the Models panel".to_owned()
    }
}

/// Reject when a job is executing on any dataset. A `running` row only
/// counts if its job is registered.
pub(crate) fn ensure_none_active(
    conn: &duckdb::Connection,
    registry: &ClassifyRegistry,
) -> Result<(), String> {
    let mut stmt = conn
        .prepare("SELECT id, title FROM datasets WHERE classify_state = 'running'")
        .map_err(|e| format!("prepare active-job check: {e}"))?;
    let running: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|e| format!("query active-job check: {e}"))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("collect active-job check: {e}"))?;
    if let Some((_, title)) = running.into_iter().find(|(id, _)| registry.is_active(id)) {
        return Err(format!(
            "\u{201c}{title}\u{201d} is classifying. Stop it or wait for it to finish."
        ));
    }
    Ok(())
}

/// Crash recovery, run once at startup before any command can fire. A
/// `running` dataset in a fresh process lost its worker with the previous
/// process; everything it flushed is in the cache, so it becomes `stopped`
/// and classifying again picks up there. Returns how many were swept.
pub fn sweep_interrupted(conn: &duckdb::Connection) -> Result<usize, String> {
    conn.execute(
        "UPDATE datasets SET classify_state = 'stopped', classify_updated_at = ?
         WHERE classify_state = 'running'",
        params![Utc::now().to_rfc3339()],
    )
    .map_err(|e| format!("sweep interrupted classifications: {e}"))
}

/// Classify a dataset with every manifest model. Starting over and resuming
/// are the same thing: only courses without a cached result are computed.
/// Returns once the job is running; progress shows up on `list_datasets`.
#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn classify_dataset(
    dataset_id: String,
    app: AppHandle,
    store: State<'_, ModelStore>,
    boot: State<'_, Boot>,
    jobs: State<'_, ClassifyRegistry>,
    activity: State<'_, Activity>,
) -> Result<(), String> {
    let Services { db, catalog, .. } = boot.ready()?;
    // Not during a delete, prune or compaction (activity.rs): checked here
    // so the call fails at once, and again under the write lock below.
    activity.ensure_idle()?;
    let Some(registry) = store.get() else {
        return Err(models_unready_message(&store));
    };
    // Every manifest level must be loaded before any inference starts, so a
    // job never half-covers the dataset because one model was still
    // downloading.
    let mut models = Vec::new();
    for digit_level in catalog.levels() {
        if registry.by_digit_level(digit_level).is_none() {
            return Err(models_unready_message(&store));
        }
        let model_id = catalog
            .model_id(digit_level)
            .ok_or_else(|| format!("manifest has no model for digit_level {digit_level}"))?;
        models.push(ClassifyModel {
            model_id,
            digit_level,
        });
    }
    if models.is_empty() {
        return Err("manifest defines no models".to_owned());
    }

    let conn = db.rw()?;
    activity.ensure_idle()?;

    let import_state: Option<Option<String>> = conn
        .query_row(
            "SELECT import_state FROM datasets WHERE id = ?",
            params![dataset_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("dataset {dataset_id}: {e}"))?;
    match import_state {
        None => return Err("This dataset no longer exists.".to_owned()),
        Some(state) if state.as_deref() != Some("ready") => {
            return Err(match state.as_deref() {
                Some("deleting") => "This dataset is being deleted.",
                Some("importing") => "This dataset is still importing.",
                Some("failed") => "This dataset's import failed.",
                _ => "This dataset isn't ready to classify.",
            }
            .to_owned());
        }
        Some(_) => {}
    }
    // One job at a time, app-wide. Checked on the held read-write
    // connection, so it is atomic with the UPDATE below.
    ensure_none_active(&conn, &jobs)?;

    let course_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM courses WHERE dataset_id = ?",
            params![dataset_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("count courses for dataset {dataset_id}: {e}"))?;
    if course_count == 0 {
        return Err("This dataset has no courses to classify.".to_owned());
    }

    // Registered before the row says `running` and before the connection is
    // released, so a stop request arriving right after this returns finds
    // the job, and the worker's finalize (which takes the same connection)
    // can't interleave.
    let job = Arc::new(Job::default());
    jobs.register(&dataset_id, &job)?;
    let now = Utc::now().to_rfc3339();
    if let Err(e) = conn.execute(
        "UPDATE datasets SET classify_state = 'running', classify_error = NULL,
                classify_ep = ?, classify_updated_at = ?
         WHERE id = ?",
        params![registry.execution_provider().as_str(), now, dataset_id],
    ) {
        jobs.remove(&dataset_id, &job);
        return Err(format!("mark dataset {dataset_id} classifying: {e}"));
    }
    drop(conn);

    let task = ClassifyTask {
        app: app.clone(),
        pipeline: ClassifyPipeline {
            dataset_id,
            models,
            computed_at: now,
            job,
        },
    };
    // spawn_blocking owns the synchronous ORT calls, off the async executor
    // that serves the polling `list_datasets`.
    tauri::async_runtime::spawn_blocking(move || task.run());
    Ok(())
}

/// Ask a dataset's job to stop. The worker stops at its next batch boundary,
/// after flushing what it classified, and the dataset becomes `stopped`.
/// `false` when the dataset has no job.
#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn stop_classification(dataset_id: String, jobs: State<'_, ClassifyRegistry>) -> bool {
    jobs.stop(&dataset_id)
}

/// Tauri-side wrapper for the background worker: resolves managed state and
/// hands the pipeline its models and database. Owned values only so the
/// spawned closure has no borrowed state to outlive.
struct ClassifyTask {
    app: AppHandle,
    pipeline: ClassifyPipeline,
}

impl ClassifyTask {
    fn run(self) {
        let jobs = self.app.state::<ClassifyRegistry>();
        let release = || jobs.remove(&self.pipeline.dataset_id, &self.pipeline.job);
        // The command that spawned this task already required startup to
        // have finished, so this only fails if that contract breaks.
        let db = match boot::services(&self.app) {
            Ok(services) => &services.db,
            Err(e) => {
                log::error!("classify {}: {e}", self.pipeline.dataset_id);
                release();
                return;
            }
        };
        let outcome = (|| {
            // Clone the registry Arc out of the store once — the worker keeps
            // this snapshot for the whole job even if the store changes later.
            let registry = self
                .app
                .state::<ModelStore>()
                .get()
                .ok_or_else(|| "models not loaded (worker)".to_owned())?;
            let mut loaded = Vec::with_capacity(self.pipeline.models.len());
            for model in &self.pipeline.models {
                loaded.push(registry.by_digit_level(model.digit_level).ok_or_else(|| {
                    format!(
                        "no model loaded for digit_level={} (worker)",
                        model.digit_level
                    )
                })?);
            }
            self.pipeline.execute(db, &loaded)
        })();
        self.pipeline.finalize(db, outcome, release);
    }
}

/// One selected course row: `(content_hash, subject_code, catalog_number,
/// course_title)` — exactly the fields the loop formats and hashes against.
type SelectedCourse = (String, String, String, String);

/// One model a job classifies with, resolved at start.
#[derive(Debug, Clone, Copy)]
pub struct ClassifyModel {
    pub model_id: i64,
    pub digit_level: u8,
}

/// The inference loop and its persistence, decoupled from Tauri managed state
/// so the resume verification harness (`examples/check_resume.rs`) can drive
/// the *real* pipeline — same batching, same flushes, same stop semantics —
/// against a scratch database.
#[derive(Debug)]
pub struct ClassifyPipeline {
    pub dataset_id: String,
    /// Models in classification order. Each level runs to completion before
    /// the next starts, so a resumed job skips finished levels by
    /// construction — their anti-join selects nothing.
    pub models: Vec<ClassifyModel>,
    /// Stamped as `computed_at` on every inference row this job writes.
    pub computed_at: String,
    /// Stop flag and live progress, shared with the registry.
    pub job: Arc<Job>,
}

/// How [`ClassifyPipeline::execute`] ended without an error.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Every model has a result for every course.
    Done,
    /// Stopped with work left.
    Stopped,
}

/// Per-level missing-work stats, measured once at job start (EPI-91): rows
/// referencing a not-yet-cached hash, and the distinct hashes themselves.
/// Row-level progress within a level is interpolated from these (exact at
/// level boundaries) instead of walked row by row.
#[derive(Clone, Copy)]
struct LevelPlan {
    missing_rows: u64,
    missing_unique: u64,
}

impl LevelPlan {
    /// Courses covered for this level once `done` of its missing distinct
    /// inputs are classified.
    fn covered(self, course_count: u64, done: u64) -> u64 {
        let advanced = (self.missing_rows * done)
            .checked_div(self.missing_unique)
            .unwrap_or(0);
        course_count.saturating_sub(self.missing_rows) + advanced
    }
}

impl ClassifyPipeline {
    /// Store the job's end state on the dataset and call `release` (which
    /// drops the job from the registry) while still holding the read-write
    /// connection, so a new `classify_dataset` on this dataset can't slip in
    /// between the two. Runs on every branch, so the row always leaves
    /// `running` unless the process itself dies (the startup sweep repairs
    /// that).
    pub fn finalize(&self, db: &AppDb, outcome: Result<Outcome, String>, release: impl FnOnce()) {
        let Ok(conn) = db.rw() else {
            // Mutex poisoning is unrecoverable; the row stays `running`,
            // which the registry check treats as inactive.
            log::error!("classify {}: rw mutex poisoned", self.dataset_id);
            release();
            return;
        };
        let (state, error) = match outcome {
            Ok(Outcome::Done) => (ClassifyState::Idle, None),
            Ok(Outcome::Stopped) => (ClassifyState::Stopped, None),
            Err(e) => {
                log::error!("classify {}: {e}", self.dataset_id);
                (ClassifyState::Failed, Some(e))
            }
        };
        if let Err(e) = conn.execute(
            "UPDATE datasets SET classify_state = ?, classify_error = ?, classify_updated_at = ?
             WHERE id = ?",
            params![
                state.as_str(),
                error,
                Utc::now().to_rfc3339(),
                &self.dataset_id
            ],
        ) {
            log::error!("classify {}: finalize: {e}", self.dataset_id);
        }
        release();
    }

    /// The batched inference loop (EPI-91/96). Per model, the *distinct*
    /// missing inputs are materialized once (anti-join + GROUP BY hash into a
    /// temp table) and consumed in content-hash keyset windows of
    /// `FLUSH_SIZE`, so the Rust heap never holds more than one window and no
    /// duplicate input is ever walked, formatted, or re-checked against the
    /// cache. Progress per level is interpolated from stats measured at job
    /// start — exact at level boundaries — instead of walked. A resumed job
    /// continues where the last one stopped by construction: finished levels
    /// (and finished hashes within a level) drop out of the anti-join.
    ///
    /// `loaded` must align with `self.models`, one loaded model per entry.
    pub fn execute(&self, db: &AppDb, loaded: &[&LoadedModel]) -> Result<Outcome, String> {
        if loaded.len() != self.models.len() {
            return Err("loaded models do not align with the job's models".to_owned());
        }
        let course_count = self.course_count(db)?;
        let mut plans = Vec::with_capacity(self.models.len());
        for model in &self.models {
            plans.push(self.level_stats(db, model.model_id)?);
        }
        // Publish what is already cached before the first window computes,
        // so a resumed job never reads zero.
        let total = i64::try_from(course_count).unwrap_or(i64::MAX);
        self.job.set_levels(
            self.models
                .iter()
                .zip(&plans)
                .map(|(model, plan)| LevelProgress {
                    digit_level: model.digit_level,
                    done: i64::try_from(plan.covered(course_count, 0)).unwrap_or(i64::MAX),
                    total,
                })
                .collect(),
        );

        let mut last_checkpoint = std::time::Instant::now();
        for (index, (model_ref, model)) in self.models.iter().zip(loaded).enumerate() {
            let Some(plan) = plans.get(index).copied() else {
                continue;
            };
            if plan.missing_unique == 0 {
                continue;
            }
            let batch = crate::inference::batch_size(model.resolved_ep);
            self.materialize_misses(db, model_ref.model_id)?;
            let mut cursor = String::new();
            let mut level_done = 0_u64;
            loop {
                // One window = one flush unit (EPI-89): a single DB
                // round-trip for the read and one for the write per
                // FLUSH_SIZE distinct inputs, while the ONNX calls inside run
                // at the per-EP batch size (EPI-82).
                let window = next_miss_window(db, &cursor)?;
                let Some(last) = window.last() else {
                    break;
                };
                // Checked here, with work in hand, so a stop that lands after
                // the last batch of the last level still ends as `Done`.
                if self.job.stopping() {
                    return Ok(Outcome::Stopped);
                }
                let next_cursor = last.0.clone();
                let (misses, classifications) = self.classify_window(model, batch, &window)?;

                // Flush whatever classified — on a stop that's a prefix of
                // the window (the misses/classifications zip truncates to
                // it); the skipped remainder stays missing and the next
                // job's anti-join picks it up.
                self.flush_batch(db, model_ref, &misses, &classifications)?;
                level_done += classifications.len() as u64;
                self.job
                    .set_done(index, plan.covered(course_count, level_done));
                if classifications.len() < misses.len() {
                    return Ok(Outcome::Stopped);
                }

                // Periodic WAL compaction (EPI-92), best-effort: a plain
                // CHECKPOINT errors harmlessly if a concurrent read
                // transaction is open — the next boundary retries.
                if last_checkpoint.elapsed() >= CHECKPOINT_INTERVAL {
                    let result = db.rw().and_then(|conn| {
                        conn.execute_batch("CHECKPOINT").map_err(|e| e.to_string())
                    });
                    if let Err(e) = result {
                        log::warn!(
                            "classify {}: periodic checkpoint skipped: {e}",
                            self.dataset_id
                        );
                    }
                    last_checkpoint = std::time::Instant::now();
                }

                cursor = next_cursor;
            }
            self.drop_misses(db);
        }
        Ok(Outcome::Done)
    }

    /// Format, length-bucket, and classify one window of distinct missing
    /// inputs. The window is distinct by construction — no cache check, no
    /// dedupe set. Length-bucketing (EPI-82): sorted inputs make each ONNX
    /// sub-batch near-uniform so `BatchLongest` padding does almost no wasted
    /// work (+14% CUDA, +16% CPU measured); order is semantically free
    /// because flush pairs by index and cache keys are content hashes. The
    /// stop flag is honored between sub-batches, so stop latency stays one
    /// ONNX call; on a stop the returned classifications are a prefix of the
    /// returned misses.
    fn classify_window<'w>(
        &self,
        model: &LoadedModel,
        batch: usize,
        window: &'w [SelectedCourse],
    ) -> Result<(Vec<Miss<'w>>, Vec<crate::inference::Classification>), String> {
        let mut misses: Vec<Miss<'w>> = window
            .iter()
            .map(|(content_hash, subject, catalog, title)| Miss {
                content_hash: content_hash.as_str(),
                input: format_input(&CourseInput {
                    subject_code: subject.clone(),
                    catalog_number: catalog.clone(),
                    course_title: title.clone(),
                }),
            })
            .collect();
        misses.sort_by_key(|m| m.input.len());
        let miss_refs: Vec<&str> = misses.iter().map(|m| m.input.as_str()).collect();

        let mut classifications = Vec::with_capacity(misses.len());
        for sub in miss_refs.chunks(batch) {
            let batch_results =
                classify_batch(model, sub).map_err(|e| format!("classify_batch: {e}"))?;
            classifications.extend(batch_results);
            if self.job.stopping() {
                break;
            }
        }
        Ok((misses, classifications))
    }

    fn course_count(&self, db: &AppDb) -> Result<u64, String> {
        let conn = db.ro()?;
        let total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM courses WHERE dataset_id = ?",
                params![&self.dataset_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("count courses: {e}"))?;
        Ok(u64::try_from(total).unwrap_or(0))
    }

    /// Missing-work stats for one model, measured once at job start (EPI-91).
    fn level_stats(&self, db: &AppDb, model_id: i64) -> Result<LevelPlan, String> {
        let conn = db.ro()?;
        let (rows, unique): (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), COUNT(DISTINCT c.content_hash)
                 FROM courses c
                 WHERE c.dataset_id = ?
                   AND NOT EXISTS (
                       SELECT 1 FROM inference_results ir
                       WHERE ir.model_id = ? AND ir.content_hash = c.content_hash
                   )",
                params![&self.dataset_id, model_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| format!("level stats: {e}"))?;
        Ok(LevelPlan {
            missing_rows: u64::try_from(rows).unwrap_or(0),
            missing_unique: u64::try_from(unique).unwrap_or(0),
        })
    }

    /// Materialize one level's distinct missing inputs into a temp table on
    /// the RW connection (EPI-91/67) — one anti-join pass, then windowed
    /// reads keep the Rust heap bounded by `FLUSH_SIZE`. `arg_min` takes all
    /// representative fields from the same (lowest `row_index`) row, so the
    /// formatted input is exactly what that row would produce.
    fn materialize_misses(&self, db: &AppDb, model_id: i64) -> Result<(), String> {
        let conn = db.rw()?;
        conn.execute(
            "CREATE OR REPLACE TEMP TABLE classify_misses AS
             SELECT c.content_hash,
                    arg_min(c.subject_code, c.row_index) AS subject_code,
                    arg_min(c.catalog_number, c.row_index) AS catalog_number,
                    arg_min(c.course_title, c.row_index) AS course_title
             FROM courses c
             WHERE c.dataset_id = ?
               AND NOT EXISTS (
                   SELECT 1 FROM inference_results ir
                   WHERE ir.model_id = ? AND ir.content_hash = c.content_hash
               )
             GROUP BY c.content_hash",
            params![&self.dataset_id, model_id],
        )
        .map_err(|e| format!("materialize misses: {e}"))?;
        Ok(())
    }

    /// Best-effort cleanup of the level's temp table; the next
    /// `materialize_misses` replaces it anyway.
    fn drop_misses(&self, db: &AppDb) {
        if let Ok(conn) = db.rw()
            && let Err(e) = conn.execute_batch("DROP TABLE IF EXISTS classify_misses")
        {
            log::warn!("classify {}: drop misses temp table: {e}", self.dataset_id);
        }
    }

    /// Single RW-mutex acquire per window: insert the window's
    /// classifications for `model` via the Appender. The
    /// `misses`/`classifications` zip truncates to the classified prefix,
    /// which is exactly right for a stop landing mid-window.
    fn flush_batch(
        &self,
        db: &AppDb,
        model: &ClassifyModel,
        misses: &[Miss<'_>],
        classifications: &[crate::inference::Classification],
    ) -> Result<(), String> {
        if classifications.is_empty() {
            return Ok(());
        }
        let conn = db.rw()?;
        let mut appender = conn
            .appender_with_columns(
                "inference_results",
                &[
                    "model_id",
                    "content_hash",
                    "classification",
                    "probability",
                    "logit_argmax",
                    "computed_at",
                    "top2_code",
                    "top2_prob",
                    "top3_code",
                    "top3_prob",
                    "top4_code",
                    "top4_prob",
                    "top5_code",
                    "top5_prob",
                ],
            )
            .map_err(|e| format!("open inference appender: {e}"))?;
        for (miss, classification) in misses.iter().zip(classifications.iter()) {
            // Codes persist in canonical zero-padded form (the model's
            // id2label strings are float-mangled); probability is the
            // softmax confidence, logit_argmax the raw research signal —
            // see docs/model-confidence.md. Ranks 2-5 (EPI-98) persist the
            // same way; rank 1 IS classification/probability.
            let [_, c2, c3, c4, c5] = &classification.top5;
            let code = |c: &crate::inference::TopCandidate| {
                crate::inference::normalize_ccm_code(&c.label, model.digit_level)
            };
            appender
                .append_row(params![
                    model.model_id,
                    miss.content_hash,
                    crate::inference::normalize_ccm_code(&classification.label, model.digit_level),
                    f64::from(classification.probability),
                    f64::from(classification.logit_argmax),
                    self.computed_at.as_str(),
                    code(c2),
                    f64::from(c2.probability),
                    code(c3),
                    f64::from(c3.probability),
                    code(c4),
                    f64::from(c4.probability),
                    code(c5),
                    f64::from(c5.probability),
                ])
                .map_err(|e| format!("inference appender append_row: {e}"))?;
        }
        appender
            .flush()
            .map_err(|e| format!("inference appender flush: {e}"))
    }
}

/// Per-row state for a cache-missed input within one batch. Borrows the
/// content hash from the worker's owned window Vec so `flush_batch` can pair
/// it with the corresponding classification without an extra clone.
struct Miss<'a> {
    content_hash: &'a str,
    input: String,
}

/// Next `FLUSH_SIZE` distinct missing inputs past `cursor`. Reads on the RW
/// connection — temp tables are per-connection, and the RO handle is a
/// different connection.
fn next_miss_window(db: &AppDb, cursor: &str) -> Result<Vec<SelectedCourse>, String> {
    let conn = db.rw()?;
    let mut stmt = conn
        .prepare(
            "SELECT content_hash, subject_code, catalog_number, course_title
             FROM classify_misses WHERE content_hash > ? ORDER BY content_hash LIMIT ?",
        )
        .map_err(|e| format!("prepare miss window: {e}"))?;
    stmt.query_map(
        params![cursor, i64::try_from(FLUSH_SIZE).unwrap_or(i64::MAX)],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )
    .map_err(|e| format!("query miss window: {e}"))?
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| format!("collect miss window: {e}"))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{ClassifyRegistry, ClassifyState, Job, ensure_none_active, sweep_interrupted};
    use crate::boot::Progress;

    /// Two datasets on a migrated in-memory database.
    fn seeded() -> Result<duckdb::Connection, String> {
        let conn = duckdb::Connection::open_in_memory().map_err(|e| e.to_string())?;
        crate::db::migrate(&conn, &Progress::none())?;
        conn.execute_batch(
            "INSERT INTO datasets (id, title, source_kind, imported_at, row_count, import_state,
                                   classify_state)
             VALUES ('a', 'A', 'file', now(), 1, 'ready', 'running'),
                    ('b', 'B', 'file', now(), 1, 'ready', 'failed');",
        )
        .map_err(|e| e.to_string())?;
        Ok(conn)
    }

    fn state(conn: &duckdb::Connection, id: &str) -> Result<ClassifyState, String> {
        let stored: String = conn
            .query_row(
                "SELECT classify_state FROM datasets WHERE id = ?",
                [id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        ClassifyState::parse(&stored)
    }

    /// A dataset has at most one job; stop reaches it; remove takes only the
    /// job it is given, never a later one on the same dataset.
    #[test]
    fn registry_holds_one_job_per_dataset() -> Result<(), String> {
        let registry = ClassifyRegistry::default();
        let first = Arc::new(Job::default());
        registry.register("a", &first)?;
        assert!(registry.register("a", &Arc::new(Job::default())).is_err());
        assert!(registry.stop("a"));
        assert!(!registry.stop("b"));
        assert!(
            registry
                .progress("a")
                .is_some_and(|p| p.stopping && p.levels.is_empty())
        );

        registry.remove("a", &first);
        let second = Arc::new(Job::default());
        registry.register("a", &second)?;
        registry.remove("a", &first);
        assert!(
            registry.is_active("a"),
            "a stale remove dropped the new job"
        );
        registry.remove("a", &second);
        assert!(registry.progress("a").is_none());
        Ok(())
    }

    /// Startup turns `running` into `stopped` and leaves other states.
    #[test]
    fn sweep_stops_running_datasets() -> Result<(), String> {
        let conn = seeded()?;
        assert_eq!(sweep_interrupted(&conn)?, 1);
        assert_eq!(state(&conn, "a")?, ClassifyState::Stopped);
        assert_eq!(state(&conn, "b")?, ClassifyState::Failed);
        Ok(())
    }

    /// A `running` row only blocks a new job while its job is registered.
    #[test]
    fn active_means_registered() -> Result<(), String> {
        let conn = seeded()?;
        let registry = ClassifyRegistry::default();
        ensure_none_active(&conn, &registry)?;
        registry.register("a", &Arc::new(Job::default()))?;
        let err = ensure_none_active(&conn, &registry)
            .err()
            .ok_or("an active job was not reported")?;
        assert!(err.contains("\u{201c}A\u{201d}"), "{err}");
        Ok(())
    }
}
