//! `cargo run --example models_install` (wrapped as `task models:install`).
//!
//! Copies the three converted-ONNX model directories from
//! `scripts/models/output/` into the resolved `models_root()` (the platform
//! data dir under `college-course-map/models/`, unless
//! `COURSE_CLASSIFIER_MODELS_DIR` overrides). Skips any digit-level whose
//! destination already exists — re-run after `task models:clean` if you need
//! to refresh.

use std::path::{Path, PathBuf};

use course_classifier_lib::inference;

/// (source dir under scripts/models/output, app-facing dest dir). The app's
/// on-disk layout is family-agnostic (two/four/six-digit); the app-active
/// family decides which converted outputs land there — `ModernBERT` per
/// EPI-56. Mirrors the `app_subdir` mapping in `scripts/models/_lib/models.py`.
const DIGIT_DIRS: &[(&str, &str)] = &[
    ("two-digit-modernbert", "two-digit"),
    ("four-digit-modernbert", "four-digit"),
    ("six-digit-modernbert", "six-digit"),
];

fn main() -> anyhow::Result<()> {
    let dest_root = inference::models_root().map_err(|e| anyhow::anyhow!(e))?;
    let src_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("scripts")
        .join("models")
        .join("output");

    if !src_root.exists() {
        anyhow::bail!(
            "source models directory missing: {} — run `task models:convert` first",
            src_root.display()
        );
    }

    std::fs::create_dir_all(&dest_root)?;
    println!("Installing models into {}", dest_root.display());

    for (src_dir, dest_dir) in DIGIT_DIRS {
        let src = src_root.join(src_dir);
        let dest = dest_root.join(dest_dir);
        if !src.exists() {
            anyhow::bail!("missing source: {}", src.display());
        }
        if dest.exists() {
            println!("  {dest_dir}: skipped (already present)");
            continue;
        }
        copy_dir_all(&src, &dest)?;
        println!("  {dest_dir}: copied from {src_dir}");
    }

    println!("Done. Set COURSE_CLASSIFIER_MODELS_DIR to override this location.");
    Ok(())
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let target = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}
