//! `cargo run --release --example check_resume` — EPI-39: kill a
//! classification mid-flight with SIGKILL, restart, sweep, classify again,
//! and verify the invariants that make resume safe:
//!
//! 1. after the crash the dataset is still `running`; `sweep_interrupted`
//!    marks it `stopped` (the startup path)
//! 2. classifying again finishes with **no duplicate** `(model_id,
//!    content_hash)` and **no missing** rows, and leaves the dataset `idle`
//! 3. nothing already computed is recomputed (first-leg rows keep their
//!    first-leg `computed_at` stamp)
//!
//! The child process runs the *real* `ClassifyPipeline` (same batching, same
//! flushes) against a scratch database; the parent SIGKILLs it
//! once progress passes a threshold, then resumes in-process. Requires the
//! two-digit model on disk (same gating as `check_parity`), so this is a
//! `task check:resume` target, not part of `task check`.

use std::{
    io::{BufRead, BufReader},
    process::{Command, Stdio},
    sync::Arc,
};

use anyhow::{Context, bail};
use chrono::Utc;
use course_classifier_lib::{
    boot::Progress,
    classify::{ClassifyModel, ClassifyPipeline, Job, Outcome, sweep_interrupted},
    db::AppDb,
    format::{CourseInput, format_input},
    inference::{self, LoadedModel},
    manifest,
    runtime::{self, EpKind},
};
use duckdb::params;

/// Kill the child once its progress reaches this — the first flushed
/// super-chunk (EPI-89), so SIGKILL lands mid-second-super-chunk. Derived
/// from the real flush cadence instead of drifting.
#[expect(
    clippy::cast_possible_wrap,
    reason = "FLUSH_SIZE is a small constant, far below i64::MAX"
)]
const KILL_AFTER_ROWS: i64 = course_classifier_lib::classify::FLUSH_SIZE as i64;
/// Synthetic dataset size. Must exceed the flush cadence by enough that the
/// child is still mid-run after its first flush on any realistic CPU (EPI-89:
/// progress is only visible per super-chunk, so the kill can't land before
/// one lands); small enough that the resume leg finishes in tens of seconds.
const ROWS: i64 = 3 * KILL_AFTER_ROWS;
const DIGIT_LEVEL: u8 = 2;
const DATASET_ID: &str = "check-resume-dataset";

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--child") => child_main(&mut args),
        Some(other) => bail!("unknown argument {other}"),
        None => parent_main(),
    }
}

/// Child mode: run the real pipeline against the scratch DB, printing
/// `PROGRESS <done>` lines the parent watches for its kill signal. This
/// process is meant to die by SIGKILL — nothing here finalizes early.
fn child_main(args: &mut impl Iterator<Item = String>) -> anyhow::Result<()> {
    let db_path = args.next().context("child: missing db path")?;
    let model_id: i64 = args
        .next()
        .context("child: missing model id")?
        .parse()
        .context("child: model id not an i64")?;

    let db =
        AppDb::open_at(db_path.into(), "dev", &Progress::none()).map_err(anyhow::Error::msg)?;
    let model = load_active_model()?;
    let pipeline = pipeline(model_id);
    println!("CHILD READY");

    std::thread::scope(|scope| -> anyhow::Result<()> {
        let worker = scope.spawn(|| {
            let outcome = pipeline.execute(&db, &[&model]);
            pipeline.finalize(&db, outcome, || {});
        });
        // Progress reporter: snapshots the job while the worker writes,
        // which is what `list_datasets` hands the polling UI.
        while !worker.is_finished() {
            std::thread::sleep(std::time::Duration::from_millis(150));
            let done: i64 = pipeline.job.snapshot().levels.iter().map(|l| l.done).sum();
            println!("PROGRESS {done}");
        }
        worker
            .join()
            .map_err(|_| anyhow::anyhow!("child worker panicked"))?;
        Ok(())
    })?;
    // Only reached if the parent never killed us — the parent treats a clean
    // child exit as a test failure (the kill threshold was never hit).
    println!("CHILD COMPLETED");
    Ok(())
}

/// A job classifying the dataset with the two-digit model.
fn pipeline(model_id: i64) -> ClassifyPipeline {
    ClassifyPipeline {
        dataset_id: DATASET_ID.to_owned(),
        models: vec![ClassifyModel {
            model_id,
            digit_level: DIGIT_LEVEL,
        }],
        computed_at: Utc::now().to_rfc3339(),
        job: Arc::new(Job::new(&[DIGIT_LEVEL], ROWS)),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "linear crash → sweep → resume → verify scenario; splitting it would scatter the invariant story across helpers"
)]
fn parent_main() -> anyhow::Result<()> {
    let scratch = std::env::temp_dir().join(format!("check-resume-{}", std::process::id()));
    let db_path = scratch.join("app.duckdb");
    let db_path_str = db_path
        .to_str()
        .context("scratch path not utf-8")?
        .to_owned();

    // --- Seed: a `running` dataset, its courses, and the model rows ---
    let model_id = {
        let db = AppDb::open_at(db_path.clone(), "dev", &Progress::none())
            .map_err(anyhow::Error::msg)?;
        let conn = db.rw().map_err(anyhow::Error::msg)?;
        let catalog =
            manifest::resolve_model_rows(&conn, manifest::load().map_err(anyhow::Error::msg)?)
                .map_err(anyhow::Error::msg)?;
        let model_id = catalog
            .model_id(DIGIT_LEVEL)
            .context("manifest has no two-digit model")?;

        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO datasets (id, title, source_kind, imported_at, row_count, import_state,
                                   classify_state, classify_ep, classify_updated_at)
             VALUES (?, 'check-resume', 'manual', ?, ?, 'ready', 'running', 'cpu', ?)",
            params![DATASET_ID, now, ROWS, now],
        )
        .context("insert dataset")?;

        let mut insert = conn
            .prepare(
                "INSERT INTO courses
                    (dataset_id, row_index, subject_code, catalog_number, course_title, content_hash)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .context("prepare course insert")?;
        for i in 0..ROWS {
            let course = CourseInput {
                subject_code: "TEST".to_owned(),
                catalog_number: format!("{i:03}"),
                course_title: format!("Synthetic resilience course number {i}"),
            };
            let formatted = format_input(&course);
            let content_hash = blake3::hash(formatted.as_bytes()).to_hex().to_string();
            insert
                .execute(params![
                    DATASET_ID,
                    i,
                    course.subject_code,
                    course.catalog_number,
                    course.course_title,
                    content_hash,
                ])
                .context("insert course")?;
        }

        model_id
        // db drops here: the child must be the only process holding the file.
    };

    // --- Leg 1: child runs the real pipeline; SIGKILL past the threshold ---
    eprintln!("spawning child; will SIGKILL after {KILL_AFTER_ROWS} rows…");
    let exe = std::env::current_exe().context("current_exe")?;
    let mut child = Command::new(exe)
        .args(["--child", &db_path_str, &model_id.to_string()])
        .stdout(Stdio::piped())
        .spawn()
        .context("spawn child")?;
    let stdout = child.stdout.take().context("child stdout")?;

    let mut killed = false;
    for line in BufReader::new(stdout).lines() {
        let line = line.context("read child stdout")?;
        if line == "CHILD COMPLETED" {
            break;
        }
        if let Some(processed) = line.strip_prefix("PROGRESS ") {
            let processed: i64 = processed.parse().unwrap_or(0);
            eprintln!("  child progress: {processed}/{ROWS}");
            if processed >= KILL_AFTER_ROWS {
                child.kill().context("SIGKILL child")?;
                killed = true;
                break;
            }
        }
    }
    child.wait().context("wait child")?;
    if !killed {
        bail!("child completed before the kill threshold — increase ROWS or lower KILL_AFTER_ROWS");
    }

    // --- Restart: reopen, verify orphan, sweep (the EPI-38 startup path) ---
    let db = AppDb::open_at(db_path, "dev", &Progress::none()).map_err(anyhow::Error::msg)?;
    let conn = db.rw().map_err(anyhow::Error::msg)?;

    let state = classify_state(&conn)?;
    if state != "running" {
        bail!("expected an orphaned 'running' dataset after SIGKILL, found '{state}'");
    }
    let swept = sweep_interrupted(&conn).map_err(anyhow::Error::msg)?;
    if swept != 1 || classify_state(&conn)? != "stopped" {
        bail!("sweep_interrupted swept {swept} datasets, expected 1 now 'stopped'");
    }

    let pre_crash: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM inference_results WHERE model_id = ?",
            params![model_id],
            |r| r.get(0),
        )
        .context("count pre-crash results")?;
    if pre_crash == 0 || pre_crash >= ROWS {
        bail!("pre-crash result count {pre_crash} not strictly between 0 and {ROWS}");
    }
    eprintln!("crash left {pre_crash}/{ROWS} results committed; resuming…");

    let leg1_stamp: String = conn
        .query_row(
            "SELECT DISTINCT computed_at::VARCHAR FROM inference_results WHERE model_id = ?",
            params![model_id],
            |r| r.get(0),
        )
        .context("read leg-1 stamp (should be exactly one distinct value)")?;

    // --- Leg 2: classify again in-process (what classify_dataset does) ---
    conn.execute(
        "UPDATE datasets SET classify_state = 'running' WHERE id = ?",
        params![DATASET_ID],
    )
    .context("mark classifying")?;
    drop(conn);

    let model = load_active_model()?;
    let pipeline = pipeline(model_id);
    let outcome = pipeline.execute(&db, &[&model]);
    let finished = matches!(outcome, Ok(Outcome::Done));
    pipeline.finalize(&db, outcome, || {});
    let progress: i64 = pipeline.job.snapshot().levels.iter().map(|l| l.done).sum();

    // --- Verify ---
    let conn = db.rw().map_err(anyhow::Error::msg)?;
    let state = classify_state(&conn)?;
    let (total, distinct): (i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), COUNT(DISTINCT content_hash)
             FROM inference_results WHERE model_id = ?",
            params![model_id],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )
        .context("count final results")?;
    let leg1_survivors: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM inference_results
             WHERE model_id = ? AND computed_at::VARCHAR = ?",
            params![model_id, leg1_stamp],
            |r| r.get::<_, i64>(0),
        )
        .context("count leg-1 survivors")?;
    drop(conn);

    let mut failures = Vec::new();
    if !finished || state != "idle" {
        failures.push(format!("final state = {state}, expected idle after Done"));
    }
    if progress != ROWS {
        failures.push(format!("progress = {progress}, expected {ROWS}"));
    }
    if total != ROWS {
        failures.push(format!(
            "result rows = {total}, expected {ROWS} (missing rows)"
        ));
    }
    if total != distinct {
        failures.push(format!(
            "{total} rows but {distinct} distinct hashes (duplicate work)"
        ));
    }
    if leg1_survivors != pre_crash {
        failures.push(format!(
            "leg-1 rows {leg1_survivors} != pre-crash count {pre_crash} (recomputed cached work)"
        ));
    }

    std::fs::remove_dir_all(&scratch).ok();

    if failures.is_empty() {
        println!(
            "check_resume PASS: {pre_crash} rows survived SIGKILL, resume computed {} more, \
             {ROWS}/{ROWS} unique results, no duplicates, no recompute",
            ROWS - pre_crash
        );
        Ok(())
    } else {
        for f in &failures {
            eprintln!("FAIL: {f}");
        }
        bail!(
            "check_resume failed ({} invariant(s) violated)",
            failures.len()
        );
    }
}

/// Load the manifest-active model for the digit level under test from the
/// same on-disk location the app uses. Also loads ONNX Runtime from the dev
/// CPU pack — deterministic across the kill/resume legs, and each process
/// (parent, child) calls this exactly once, satisfying init-once.
fn load_active_model() -> anyhow::Result<LoadedModel> {
    runtime::init_ort(&runtime::dev_cpu_pack_dir())
        .map_err(|e| anyhow::anyhow!("{e} — run `task runtimes:fetch` first"))?;
    let manifest = manifest::load().map_err(anyhow::Error::msg)?;
    let entry = manifest
        .model
        .iter()
        .find(|m| m.digit_level == DIGIT_LEVEL)
        .context("manifest has no entry for the test digit level")?;
    let root = inference::models_root().map_err(anyhow::Error::msg)?;
    inference::load_model(
        &root.join(&entry.app_subdir),
        DIGIT_LEVEL,
        &[EpKind::Cpu],
        0,
    )
}

fn classify_state(conn: &duckdb::Connection) -> anyhow::Result<String> {
    conn.query_row(
        "SELECT classify_state FROM datasets WHERE id = ?",
        params![DATASET_ID],
        |r| r.get(0),
    )
    .context("read classify_state")
}
