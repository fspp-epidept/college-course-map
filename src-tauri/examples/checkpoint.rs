//! `cargo run --example checkpoint` (wrapped as `task db:checkpoint`).
//! Runs `DuckDB` CHECKPOINT against the app database, compacting the WAL into
//! the main file. Useful after a big async import on an older build that
//! didn't checkpoint automatically — speeds up the first query against the
//! freshly-imported data.

fn main() -> anyhow::Result<()> {
    let path = course_classifier_lib::db::db_path().map_err(|e| anyhow::anyhow!(e))?;
    let conn = duckdb::Connection::open(&path)?;
    conn.execute_batch("CHECKPOINT")?;
    println!("Checkpointed {}", path.display());
    Ok(())
}
