//! `cargo run --example clear_user_data` (wrapped as `task db:clear-data`).
//! Wipes the dataset / course / run / result tables but leaves the seeded
//! `models` rows in place so Classify still works. Used between demos to
//! start fresh without re-loading all three ONNX models.

use duckdb::Connection;

fn main() -> anyhow::Result<()> {
    let path = course_classifier_lib::db::db_path().map_err(|e| anyhow::anyhow!(e))?;
    let conn = Connection::open(&path)?;

    // Order matters: child tables first (FKs).
    for table in [
        "inference_results",
        "runs",
        "courses",
        "datasets",
        "source_files",
    ] {
        conn.execute_batch(&format!("DELETE FROM {table}"))?;
    }

    println!(
        "Cleared user data at {}; models table preserved.",
        path.display()
    );
    Ok(())
}
