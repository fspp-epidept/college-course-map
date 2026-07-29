//! `cargo run --release --example throughput_bench -- [flags]` (wrapped as
//! `task check:throughput`) — the EPI-82 measurement harness: batched
//! inference throughput over a real CSV on this machine's resolved runtime
//! pack + execution provider.
//!
//! Flags (all optional):
//! - `--csv PATH`   input file (default `data/validation.csv`, panel headers)
//! - `--rows N`     stop after reading N CSV rows (default: whole file)
//! - `--batch N`    override the per-EP batch size from `inference::batch_size`
//! - `--ep NAME`    force the EP priority head (`cpu`, `cuda`, `tensorrt`, …)
//!   instead of the platform default
//! - `--level D`    digit level 2/4/6 (default 2; model must be on disk)
//! - `--bucket`     sort inputs by length before batching, so `BatchLongest`
//!   padding within each sub-batch is near-zero (EPI-82 length-bucketing)
//!
//! Measures the model side only — CSV parse + dedupe are reported separately
//! and the DB flush is deliberately excluded (EPI-82 ranked it second-order,
//! and it is identical across EPs). Dedupe mirrors the run pipeline: unique
//! formatted inputs classify once; the effective rows/sec line counts the
//! duplicate rows that would ride the cache for free.

use std::{collections::HashSet, path::PathBuf, time::Instant};

use course_classifier_lib::{
    format::{CourseInput, format_input},
    inference::{self, classify_batch},
    runtime::{self, EpKind},
};

struct Args {
    csv: PathBuf,
    rows: Option<usize>,
    batch: Option<usize>,
    ep: Option<EpKind>,
    level: u8,
    bucket: bool,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut args = Args {
        csv: PathBuf::from("data/validation.csv"),
        rows: None,
        batch: None,
        ep: None,
        level: 2,
        bucket: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || {
            it.next()
                .ok_or_else(|| anyhow::anyhow!("{flag} needs a value"))
        };
        match flag.as_str() {
            "--csv" => args.csv = PathBuf::from(value()?),
            "--rows" => args.rows = Some(value()?.parse()?),
            "--batch" => args.batch = Some(value()?.parse()?),
            "--level" => args.level = value()?.parse()?,
            "--bucket" => args.bucket = true,
            "--ep" => {
                let name = value()?;
                let ep = serde_json::from_value(serde_json::Value::String(name.clone()))
                    .map_err(|_| anyhow::anyhow!("unknown ep '{name}'"))?;
                args.ep = Some(ep);
            }
            other => anyhow::bail!("unknown flag '{other}' (see example doc comment)"),
        }
    }
    Ok(args)
}

fn model_subdir(level: u8) -> anyhow::Result<&'static str> {
    match level {
        2 => Ok("two-digit"),
        4 => Ok("four-digit"),
        6 => Ok("six-digit"),
        other => anyhow::bail!("--level must be 2, 4, or 6 (got {other})"),
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "row counts are far below f64's 2^53 exact-integer range"
)]
fn rate(rows: usize, elapsed: std::time::Duration) -> f64 {
    rows as f64 / elapsed.as_secs_f64().max(f64::EPSILON)
}

/// CSV parse + run-pipeline-style dedupe: `(rows_read, unique inputs)`.
fn read_inputs(args: &Args) -> anyhow::Result<(usize, Vec<String>)> {
    let mut reader = csv::Reader::from_path(&args.csv)
        .map_err(|e| anyhow::anyhow!("open {}: {e}", args.csv.display()))?;
    let headers = reader.headers()?.clone();
    let col = |name: &str| {
        headers
            .iter()
            .position(|h| h == name)
            .ok_or_else(|| anyhow::anyhow!("column '{name}' missing in {}", args.csv.display()))
    };
    let (subject_col, catalog_col, title_col) = (
        col("sub_pref")?,
        col("course")?,
        col("inventory_course_title")?,
    );
    let mut rows_read = 0usize;
    let mut seen = HashSet::new();
    let mut unique: Vec<String> = Vec::new();
    for record in reader.records() {
        let record = record?;
        rows_read += 1;
        let field = |i: usize| record.get(i).unwrap_or_default().to_owned();
        let input = format_input(&CourseInput {
            subject_code: field(subject_col),
            catalog_number: field(catalog_col),
            course_title: field(title_col),
        });
        if seen.insert(input.clone()) {
            unique.push(input);
        }
        if args.rows.is_some_and(|n| rows_read >= n) {
            break;
        }
    }
    Ok((rows_read, unique))
}

fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    let subdir = model_subdir(args.level)?;

    // Runtime resolution mirrors `runtime_check`: the dev fetch location
    // stands in for the bundle resource dir.
    let manifest = runtime::load_manifest().map_err(anyhow::Error::msg)?;
    let eps: Vec<EpKind> = match args.ep {
        Some(EpKind::Cpu) => vec![EpKind::Cpu],
        Some(ep) => vec![ep, EpKind::Cpu],
        None => runtime::default_priority(),
    };
    let (state, pack_dir) = runtime::resolve_startup_pack(
        &manifest,
        &eps,
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")),
    )
    .map_err(anyhow::Error::msg)?;
    println!(
        "pack                 : {} (ONNX Runtime {})",
        state.pack_id, state.ort_version
    );
    runtime::init_ort(&pack_dir)
        .map_err(|e| anyhow::anyhow!("{e} — run `task runtimes:fetch` first"))?;
    if let Some(libs_dir) = runtime::installed_libs_dir(&manifest, &state) {
        let count = runtime::preload_support_libs(&libs_dir).map_err(anyhow::Error::msg)?;
        println!("preloaded libs       : {count} from {}", libs_dir.display());
    }

    // CSV parse + dedupe, timed separately from inference.
    let parse_started = Instant::now();
    let (rows_read, mut unique) = read_inputs(&args)?;
    // Byte length is a good-enough proxy for token length on course strings;
    // sorted input makes every sub-batch near-uniform so BatchLongest pads
    // almost nothing.
    if args.bucket {
        unique.sort_by_key(String::len);
    }
    println!(
        "csv                  : {rows_read} rows, {} unique inputs, parsed in {:.1?}",
        unique.len(),
        parse_started.elapsed()
    );
    if unique.is_empty() {
        anyhow::bail!("no inputs to classify");
    }

    let root = inference::models_root().map_err(anyhow::Error::msg)?;
    let load_started = Instant::now();
    let model = inference::load_model(&root.join(subdir), args.level, &eps, 0)?;
    let batch = args
        .batch
        .unwrap_or_else(|| inference::batch_size(model.resolved_ep));
    println!(
        "resolved EP          : {} (session built in {:.1?}), batch size {batch}",
        model.resolved_ep.as_str(),
        load_started.elapsed()
    );

    // One untimed warmup batch: the first `session.run` pays lazy init (GPU
    // memory arena, kernel selection) that would skew a short measurement.
    let warmup: Vec<&str> = unique.iter().take(batch).map(String::as_str).collect();
    let warmup_started = Instant::now();
    classify_batch(&model, &warmup)?;
    println!(
        "warmup               : {} rows in {:.1?} (untimed below)",
        warmup.len(),
        warmup_started.elapsed()
    );

    let bench_started = Instant::now();
    let mut done = 0usize;
    let total_batches = unique.len().div_ceil(batch);
    for (index, chunk) in unique.chunks(batch).enumerate() {
        let refs: Vec<&str> = chunk.iter().map(String::as_str).collect();
        classify_batch(&model, &refs)?;
        done += chunk.len();
        if (index + 1) % 50 == 0 || index + 1 == total_batches {
            println!(
                "  batch {:>5}/{total_batches}: {done}/{} unique, {:.0} unique rows/s",
                index + 1,
                unique.len(),
                rate(done, bench_started.elapsed())
            );
        }
    }
    let wall = bench_started.elapsed();
    println!("inference wall       : {wall:.1?}");
    println!(
        "unique throughput    : {:.0} rows/s ({} unique inputs)",
        rate(unique.len(), wall),
        unique.len()
    );
    println!(
        "effective throughput : {:.0} rows/s ({rows_read} CSV rows; duplicates ride the cache)",
        rate(rows_read, wall)
    );
    Ok(())
}
