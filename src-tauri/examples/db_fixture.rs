//! `cargo run --example db_fixture` (wrapped as `task db:fixture`).
//!
//! Writes the upgrade-test fixture for this build's database state: a
//! migrated, seeded database saved as
//! `fixtures/db/schema-v<N>_duckdb-<version>.duckdb.gz`. The tests in `db.rs`
//! require one for the current state and open every fixture in the
//! directory, so run this in the PR that adds a migration or re-pins
//! `DuckDB` (#204). A fixture records what that state looked like on disk; it
//! is never regenerated.

use std::{fs, io, path::PathBuf};

use anyhow::{Context as _, bail};
use course_classifier_lib::{boot::Progress, db::AppDb, seed};
use flate2::{Compression, write::GzEncoder};

fn main() -> anyhow::Result<()> {
    let scratch = std::env::temp_dir().join(format!("ccm-db-fixture-{}", std::process::id()));
    fs::create_dir_all(&scratch).context("create scratch dir")?;
    let path = scratch.join("app.duckdb");

    let name = {
        let db = AppDb::open_at(path.clone(), "fixture", &Progress::none())
            .map_err(anyhow::Error::msg)?;
        seed::run(&db).map_err(anyhow::Error::msg)?;
        db.checkpoint().map_err(anyhow::Error::msg)?;
        let conn = db.rw().map_err(anyhow::Error::msg)?;
        let schema: i64 =
            conn.query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))?;
        let duckdb: String =
            conn.query_row("SELECT library_version FROM pragma_version()", [], |r| {
                r.get(0)
            })?;
        // Same format as `fixture_exists_for_head` in db.rs.
        format!("schema-v{schema}_duckdb-{duckdb}.duckdb.gz")
    };

    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/db");
    let dest = dir.join(&name);
    if dest.exists() {
        bail!(
            "{} already exists; fixtures are never regenerated",
            dest.display()
        );
    }
    fs::create_dir_all(&dir).context("create fixtures dir")?;
    let mut packed = GzEncoder::new(
        fs::File::create(&dest).context("create fixture")?,
        Compression::best(),
    );
    io::copy(
        &mut fs::File::open(&path).context("open scratch db")?,
        &mut packed,
    )
    .context("pack fixture")?;
    packed.finish().context("finish fixture")?;
    fs::remove_dir_all(&scratch).context("remove scratch dir")?;
    println!("Wrote {}", dest.display());
    Ok(())
}
