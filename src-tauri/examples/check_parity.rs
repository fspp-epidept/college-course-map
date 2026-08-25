//! `cargo run --release --example check_parity` — verify that Rust ONNX
//! Runtime's argmax matches the Python pipeline's reference output for every
//! (model, input) pair recorded in `scripts/models/output/parity/per_input.json`.
//!
//! `--release` is intentional: debug-build FP behavior can drift enough on
//! near-tied logits to flip the argmax, while the Python reference was captured
//! with the same opt level as Rust release. Mismatches in release mode are real
//! parity failures and must be investigated.
//!
//! Parity is CPU-only by decision (EPI-73). `--ep NAME` (e.g. `--ep coreml`)
//! registers another provider ahead of CPU and reports the same numbers, for
//! measuring whether a provider is fit to ship (EPI-108) — not for gating.

use std::collections::BTreeMap;

use course_classifier_lib::{
    inference::{self, LoadedModel, classify},
    runtime::{self, EpKind},
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct ParityEntry {
    model_subdir: String,
    input: String,
    argmax: usize,
    top3: [usize; 3],
    logit_argmax_value: f32,
}

/// Per the writer of `per_input.json` (verify.py), max ONNX-vs-PyTorch logit
/// diff across all models was ~5.7e-6. The Rust vs Python ONNX comparison
/// should be tighter still (same backend), so allow 5e-5 — well above noise
/// but tight enough to catch real drift.
const LOGIT_TOLERANCE: f32 = 5e-5;

fn main() -> anyhow::Result<()> {
    let eps = parse_eps()?;
    // GPU float math is not bit-identical to the Python reference; a non-CPU
    // provider here is a measurement, not the parity gate.
    runtime::init_ort(&runtime::dev_cpu_pack_dir())
        .map_err(|e| anyhow::anyhow!("{e} — run `task runtimes:fetch` first"))?;

    let parity_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("scripts")
        .join("models")
        .join("output")
        .join("parity")
        .join("per_input.json");

    let entries: Vec<ParityEntry> = serde_json::from_slice(&std::fs::read(&parity_path)?)?;

    // Group by model so each model loads exactly once.
    let mut by_model: BTreeMap<String, Vec<&ParityEntry>> = BTreeMap::new();
    for entry in &entries {
        by_model
            .entry(entry.model_subdir.clone())
            .or_default()
            .push(entry);
    }

    let mut total = 0_u32;
    let mut argmax_matches = 0_u32;
    let mut top3_matches = 0_u32;
    let mut max_logit_diff: f32 = 0.0;
    let mut failures: Vec<String> = Vec::new();

    let root = inference::models_root().map_err(|e| anyhow::anyhow!(e))?;
    for (subdir, items) in by_model {
        let digit_level = match subdir.as_str() {
            "two-digit" => 2,
            "four-digit" => 4,
            "six-digit" => 6,
            other => anyhow::bail!("unknown model subdir: {other}"),
        };
        eprintln!("Loading {subdir} ({} inputs)…", items.len());
        let model = inference::load_model(&root.join(&subdir), digit_level, &eps, 0)?;
        eprintln!(
            "  resolved execution provider: {}",
            model.resolved_ep.as_str()
        );

        for entry in items {
            total += 1;
            let result = run_with_context(&model, entry)?;
            let logit_diff = (result.logit_argmax - entry.logit_argmax_value).abs();
            if logit_diff > max_logit_diff {
                max_logit_diff = logit_diff;
            }
            let argmax_ok = result.label_index == entry.argmax;
            // The fixture predates top-5 extraction and records three ranks;
            // compare against the first three of the Rust top-5.
            let top3_ok = result
                .top5
                .iter()
                .take(3)
                .map(|c| c.index)
                .eq(entry.top3.iter().copied());
            if argmax_ok {
                argmax_matches += 1;
            } else {
                failures.push(format!(
                    "{subdir}/{:?} argmax: rust={} ref={} (logit_diff={logit_diff:.2e})",
                    entry.input, result.label_index, entry.argmax
                ));
            }
            if top3_ok {
                top3_matches += 1;
            }
            if logit_diff > LOGIT_TOLERANCE {
                failures.push(format!(
                    "{subdir}/{:?} logit drift: rust={} ref={} diff={logit_diff:.2e}",
                    entry.input, result.logit_argmax, entry.logit_argmax_value
                ));
            }
        }
    }

    println!();
    println!("Parity report:");
    println!("  total inputs       : {total}");
    println!("  argmax matches     : {argmax_matches} / {total}");
    println!("  top-3 matches      : {top3_matches} / {total}");
    println!("  max logit diff     : {max_logit_diff:.3e}");
    println!("  tolerance          : {LOGIT_TOLERANCE:.0e}");

    if failures.is_empty() {
        println!("\nPARITY: {argmax_matches}/{total} match");
        Ok(())
    } else {
        eprintln!("\nFailures:");
        for f in &failures {
            eprintln!("  - {f}");
        }
        anyhow::bail!("{} parity failure(s)", failures.len());
    }
}

fn run_with_context(
    model: &LoadedModel,
    entry: &ParityEntry,
) -> anyhow::Result<inference::Classification> {
    classify(model, &entry.input).map_err(|e| anyhow::anyhow!("classify {:?}: {e}", entry.input))
}

/// `--ep NAME` → `[NAME, cpu]`; absent (or `cpu`) → `[cpu]`.
fn parse_eps() -> anyhow::Result<Vec<EpKind>> {
    let mut it = std::env::args().skip(1);
    let mut eps = vec![EpKind::Cpu];
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--ep" => {
                let name = it
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--ep needs a value"))?;
                let ep: EpKind = serde_json::from_value(serde_json::Value::String(name.clone()))
                    .map_err(|_| anyhow::anyhow!("unknown ep '{name}'"))?;
                if ep != EpKind::Cpu {
                    eps.insert(0, ep);
                }
            }
            other => anyhow::bail!("unknown flag '{other}' (only --ep NAME)"),
        }
    }
    Ok(eps)
}
