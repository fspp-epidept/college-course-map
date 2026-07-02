//! Demo data seeding. Called by `cargo run --example seed_demo`, wrapped as
//! `task seed:demo`. Idempotent in the destructive sense: every run truncates
//! the dataset/run/result tables and reinserts the same fixtures, so the dev
//! loop is `task db:reset` (delete file) or `task seed:demo` (refresh data) —
//! whichever you prefer.
//!
//! No CSV ingest yet; the rows here are hand-fabricated so the Datasets / Runs
//! activity tabs render something during Phase 3 UI work.

use blake3::Hasher;
use chrono::Utc;
use duckdb::params;
use uuid::Uuid;

use crate::{
    db::AppDb,
    format::{CourseInput, format_input},
};

/// Hardcoded fixture courses. Three pairs across two source files; identical
/// content across pairs exercises the cache-by-content_hash design.
const FIXTURES: &[(&str, &str, &str)] = &[
    ("MATH", "101", "Calculus I"),
    ("MATH", "102", "Calculus II"),
    ("ENGL", "120", "Composition"),
    ("CS", "150", "Intro to Programming"),
    ("CS", "250", "Data Structures"),
    ("BIOL", "110", "Introductory Biology"),
];

/// Insert a representative dataset: two source files, two file-backed datasets,
/// 12 courses, three CCM models, one completed run, and 12 inference results
/// (one per course, for the 6-digit model).
pub fn run(db: &AppDb) -> Result<(), String> {
    let conn = db.rw()?;
    let now = Utc::now().to_rfc3339();

    conn.execute_batch("BEGIN").map_err(|e| e.to_string())?;
    if let Err(e) = seed_inner(&conn, &now) {
        let _ = conn.execute_batch("ROLLBACK");
        return Err(e);
    }
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    Ok(())
}

fn seed_inner(conn: &duckdb::Connection, now: &str) -> Result<(), String> {
    truncate_all(conn)?;
    let (sf1, sf2) = seed_source_files(conn, now)?;
    let (ds1, ds2) = seed_datasets(conn, now, sf1, sf2)?;
    let content_hashes = seed_courses(conn, &ds1, &ds2)?;
    let model_six = seed_models(conn)?;
    let run_id = seed_run(conn, &ds1, model_six, now)?;
    seed_results(conn, model_six, &content_hashes, &run_id, now)
}

/// FK-safe truncation. Explicit per-table because `DELETE FROM` doesn't cascade
/// in our schema (intentional — we never want cascades at runtime).
fn truncate_all(conn: &duckdb::Connection) -> Result<(), String> {
    for table in [
        "inference_results",
        "runs",
        "courses",
        "datasets",
        "models",
        "source_files",
    ] {
        conn.execute_batch(&format!("DELETE FROM {table}"))
            .map_err(|e| format!("clear {table}: {e}"))?;
    }
    Ok(())
}

fn seed_source_files(conn: &duckdb::Connection, now: &str) -> Result<(i64, i64), String> {
    // Hashes are placeholder-deterministic — real ingest computes blake3 over
    // file bytes; here we synthesize a stable string per file label.
    let sf1: i64 = conn
        .query_row(
            "INSERT INTO source_files
                (path, display_name, imported_at, imported_hash, size_bytes)
             VALUES (?, ?, ?, ?, ?) RETURNING id",
            params![
                "/demo/fall-2025.csv",
                "Fall 2025 transcripts",
                now,
                fake_file_hash("fall-2025"),
                12_345_i64,
            ],
            |row| row.get(0),
        )
        .map_err(|e| format!("insert source_files (sf1): {e}"))?;
    let sf2: i64 = conn
        .query_row(
            "INSERT INTO source_files
                (path, display_name, imported_at, imported_hash, size_bytes)
             VALUES (?, ?, ?, ?, ?) RETURNING id",
            params![
                "/demo/spring-2026.csv",
                "Spring 2026 transcripts",
                now,
                fake_file_hash("spring-2026"),
                14_002_i64,
            ],
            |row| row.get(0),
        )
        .map_err(|e| format!("insert source_files (sf2): {e}"))?;
    Ok((sf1, sf2))
}

fn seed_datasets(
    conn: &duckdb::Connection,
    now: &str,
    sf1: i64,
    sf2: i64,
) -> Result<(String, String), String> {
    let ds1 = Uuid::new_v4().to_string();
    let ds2 = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO datasets
            (id, title, source_kind, source_file_id, imported_at, row_count)
         VALUES (?, ?, 'file', ?, ?, ?)",
        params![ds1, "Fall 2025 transcripts", sf1, now, 6_i64],
    )
    .map_err(|e| format!("insert datasets (ds1): {e}"))?;
    conn.execute(
        "INSERT INTO datasets
            (id, title, source_kind, source_file_id, imported_at, row_count)
         VALUES (?, ?, 'file', ?, ?, ?)",
        params![ds2, "Spring 2026 transcripts", sf2, now, 6_i64],
    )
    .map_err(|e| format!("insert datasets (ds2): {e}"))?;
    Ok((ds1, ds2))
}

/// Insert the same fixtures into both datasets so cache reuse is visible —
/// identical `content_hash` across datasets, single `inference_results` row.
/// Returns the hashes (one per fixture) so the results seeder can target them.
fn seed_courses(conn: &duckdb::Connection, ds1: &str, ds2: &str) -> Result<Vec<String>, String> {
    let mut hashes: Vec<String> = Vec::with_capacity(FIXTURES.len());
    for (i, &(subj, num, title)) in FIXTURES.iter().enumerate() {
        let hash = content_hash(subj, num, title);
        let row_index = i64::try_from(i).unwrap_or(0);
        for dataset_id in [ds1, ds2] {
            conn.execute(
                "INSERT INTO courses
                    (dataset_id, row_index, subject_code, catalog_number,
                     course_title, content_hash)
                 VALUES (?, ?, ?, ?, ?, ?)",
                params![dataset_id, row_index, subj, num, title, hash],
            )
            .map_err(|e| format!("insert courses: {e}"))?;
        }
        hashes.push(hash);
    }
    Ok(hashes)
}

/// Seed the models table from the embedded manifest — the same rows the app
/// resolves at startup, so demo results reference real pinned models instead
/// of placeholders. Returns the 6-digit model id, which the demo run targets.
fn seed_models(conn: &duckdb::Connection) -> Result<i64, String> {
    let catalog = crate::manifest::resolve_model_rows(conn, crate::manifest::load()?)?;
    catalog
        .model_id(6)
        .ok_or_else(|| "manifest has no 6-digit model".to_owned())
}

fn seed_run(
    conn: &duckdb::Connection,
    ds1: &str,
    model_six: i64,
    now: &str,
) -> Result<String, String> {
    let run_id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO runs
            (id, dataset_id, description, state, model_ids,
             rows_total, rows_processed,
             unique_inputs_total, unique_inputs_done, cache_hits,
             created_at, started_at, completed_at, execution_provider)
         VALUES (?, ?, ?, 'completed', ?,
                 ?, ?,
                 ?, ?, ?,
                 ?, ?, ?, ?)",
        params![
            run_id,
            ds1,
            "Demo run: Fall 2025 transcripts × 6-digit",
            serde_json::to_string(&[model_six]).map_err(|e| e.to_string())?,
            6_i64,
            6_i64,
            6_i64,
            6_i64,
            0_i64,
            now,
            now,
            now,
            "cpu",
        ],
    )
    .map_err(|e| format!("insert runs: {e}"))?;
    Ok(run_id)
}

/// One result per unique `content_hash`, 6-digit model. Codes are placeholders —
/// shape and accounting are correct as-is; values change when real inference
/// lands.
fn seed_results(
    conn: &duckdb::Connection,
    model_six: i64,
    hashes: &[String],
    run_id: &str,
    now: &str,
) -> Result<(), String> {
    // Canonical zero-padded codes (match ccm_taxonomy) so demo rows exercise
    // the taxonomy join like real runs do.
    let demo_codes = [
        "27.0101", "27.0102", "23.0101", "11.0701", "11.0798", "26.0101",
    ];
    for (i, hash) in hashes.iter().enumerate() {
        let code = demo_codes.get(i).copied().unwrap_or("00.0000");
        conn.execute(
            "INSERT INTO inference_results
                (model_id, content_hash, classification, probability,
                 logit_argmax, computed_at, computed_by_run)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![model_six, hash, code, 0.94_f64, 7.8_f64, now, run_id],
        )
        .map_err(|e| format!("insert inference_results: {e}"))?;
    }
    Ok(())
}

fn content_hash(subj: &str, num: &str, title: &str) -> String {
    let input = format_input(&CourseInput {
        subject_code: subj.to_owned(),
        catalog_number: num.to_owned(),
        course_title: title.to_owned(),
    });
    let mut h = Hasher::new();
    h.update(input.as_bytes());
    h.finalize().to_hex().to_string()
}

fn fake_file_hash(label: &str) -> String {
    let mut h = Hasher::new();
    h.update(b"seed-file:");
    h.update(label.as_bytes());
    h.finalize().to_hex().to_string()
}
