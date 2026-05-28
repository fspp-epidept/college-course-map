//! `cargo run --example seed_demo` (wrapped as `task seed:demo`).
//! Truncates and reseeds demo fixtures. See `src/seed.rs` for what's inserted.

fn main() -> anyhow::Result<()> {
    let db = course_classifier_lib::db::AppDb::open().map_err(|e| anyhow::anyhow!(e))?;
    course_classifier_lib::seed::run(&db).map_err(|e| anyhow::anyhow!(e))?;
    let path = course_classifier_lib::db::db_path().map_err(|e| anyhow::anyhow!(e))?;
    println!("Seeded demo data at {}", path.display());
    Ok(())
}
