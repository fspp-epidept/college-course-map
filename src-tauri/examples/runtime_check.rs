//! `cargo run --release --example runtime_check` (wrapped as
//! `task check:runtime`) — diagnostic for the EPI-73 GPU path on *this*
//! machine: which pack resolves, which execution provider actually registers,
//! and how fast one real classification runs on it.
//!
//! Uses the platform-default EP priority (not the app's settings file) so the
//! output answers "what would a fresh install do here". A machine with the
//! GPU pack installed but missing CUDA/cuDNN system libraries will show the
//! designed fallback: pack `cuda`, resolved EP `cpu`, with ort's warning on
//! stderr explaining why registration failed.

use std::path::Path;

use course_classifier_lib::{
    inference::{self, classify},
    runtime,
};

fn main() -> anyhow::Result<()> {
    let manifest = runtime::load_manifest().map_err(anyhow::Error::msg)?;
    let eps = runtime::default_priority();
    let ep_names: Vec<&str> = eps.iter().map(|ep| ep.as_str()).collect();
    println!("platform EP priority : {ep_names:?}");

    // The example has no Tauri resource dir; the dev fetch location plays
    // that role (same layout the bundle ships).
    let (state, pack_dir) =
        runtime::resolve_startup_pack(&manifest, &eps, Path::new(env!("CARGO_MANIFEST_DIR")))
            .map_err(anyhow::Error::msg)?;
    println!(
        "resolved pack        : {} (ONNX Runtime {}, claims {:?})",
        state.pack_id, state.ort_version, state.eps
    );
    println!("pack dir             : {}", pack_dir.display());

    runtime::init_ort(&pack_dir)
        .map_err(|e| anyhow::anyhow!("{e} — run `task runtimes:fetch` first"))?;

    // Mirror the app's startup: preload the companion libs pack when the
    // resolved runtime pack names one and it's downloaded (EPI-84).
    if let Some(libs_dir) = runtime::installed_libs_dir(&manifest, &state) {
        let count = runtime::preload_support_libs(&libs_dir).map_err(anyhow::Error::msg)?;
        println!("preloaded libs       : {count} from {}", libs_dir.display());
    } else {
        println!("preloaded libs       : none (no companion libs pack installed)");
    }

    let root = inference::models_root().map_err(anyhow::Error::msg)?;
    let started = std::time::Instant::now();
    let model = inference::load_model(&root.join("two-digit"), 2, &eps, 0)?;
    println!(
        "resolved EP          : {} (session built in {:.1?})",
        model.resolved_ep.as_str(),
        started.elapsed()
    );

    let input = "EECS 445 --- MACHINE LEARNING";
    let warm = std::time::Instant::now();
    let result = classify(&model, input)?;
    println!(
        "sample classification: {input:?} -> {} (p={:.3}) in {:.1?}",
        result.label,
        result.probability,
        warm.elapsed()
    );
    Ok(())
}
