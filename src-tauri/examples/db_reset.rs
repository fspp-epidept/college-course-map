//! `cargo run --example db_reset` (wrapped as `task db:reset`).
//! Deletes the local `DuckDB` file. Idempotent — no error if it's already gone.

use std::io::ErrorKind;

fn main() -> anyhow::Result<()> {
    let path = course_classifier_lib::db::db_path().map_err(|e| anyhow::anyhow!(e))?;
    match std::fs::remove_file(&path) {
        Ok(()) => println!("Removed {}", path.display()),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            println!("Nothing to remove at {}", path.display());
        }
        Err(e) => return Err(anyhow::anyhow!("remove {}: {e}", path.display())),
    }
    Ok(())
}
