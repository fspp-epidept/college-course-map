//! `DuckDB` connection management + migration runner.
//!
//! One read-write [`Connection`] lives behind a `Mutex` in Tauri state; read-only
//! connections are opened ad-hoc for list/dashboard queries (per the architectural
//! ground rule in `CLAUDE.md`). `DuckDB` connection-open is cheap, so we don't pool.
//!
//! Migrations are hand-rolled: ordered SQL files embedded via `include_str!`,
//! applied in transactions, tracked by a `schema_version` table. This is fine
//! through Phase 3 — consider switching to `refinery` if we ever need
//! down-migrations, parallel branches, or accumulate more than a handful.

use std::{
    fs,
    path::PathBuf,
    sync::{Mutex, MutexGuard},
};

use duckdb::{AccessMode, Config, Connection, params};

const PRODUCT_DIR: &str = "college-course-map";
const DB_FILE: &str = "app.duckdb";

/// Ordered list of migration scripts. Add new entries — never edit or reorder
/// existing ones. Version numbers are monotonic and gap-free by convention.
const MIGRATIONS: &[(u32, &str)] = &[
    (1, include_str!("../migrations/0001_initial.sql")),
    (
        2,
        include_str!("../migrations/0002_dataset_import_state.sql"),
    ),
];

/// Owned read-write connection plus the resolved on-disk path, so read-only
/// consumers can re-open the same database without re-resolving the path.
pub struct AppDb {
    rw: Mutex<Connection>,
    path: PathBuf,
}

impl std::fmt::Debug for AppDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The RW connection isn't `Debug`, so we skip it; the path is the only
        // useful state to surface in logs.
        f.debug_struct("AppDb")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl AppDb {
    /// Resolve the on-disk path, create the parent directory, open the RW
    /// connection, and apply any pending migrations.
    pub fn open() -> Result<Self, String> {
        let path = db_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        let conn = Connection::open(&path).map_err(|e| e.to_string())?;
        migrate(&conn)?;
        Ok(Self {
            rw: Mutex::new(conn),
            path,
        })
    }

    /// Borrow the read-write connection. The mutex is uncontended in single-user
    /// app flows; we hold it briefly per command. Stress test (#49/#50) will
    /// validate that this is fine under inference + dashboard concurrency.
    pub fn rw(&self) -> Result<MutexGuard<'_, Connection>, String> {
        self.rw.lock().map_err(|_| "rw mutex poisoned".to_owned())
    }

    /// Open a fresh read-only connection. Cheap; callers open and drop per query.
    pub(crate) fn ro(&self) -> Result<Connection, String> {
        let cfg = Config::default()
            .access_mode(AccessMode::ReadOnly)
            .map_err(|e| e.to_string())?;
        Connection::open_with_flags(&self.path, cfg).map_err(|e| e.to_string())
    }
}

/// `<data>/college-course-map/app.duckdb` — same product-dir convention used by
/// `config.rs`, but rooted at the platform data dir (not config) per `CLAUDE.md`.
pub fn db_path() -> Result<PathBuf, String> {
    dirs::data_dir()
        .map(|dir| dir.join(PRODUCT_DIR).join(DB_FILE))
        .ok_or_else(|| "no platform data directory available".to_owned())
}

fn migrate(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version    INTEGER PRIMARY KEY,
            applied_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
        )",
    )
    .map_err(|e| format!("create schema_version: {e}"))?;

    let current: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;

    for &(version, sql) in MIGRATIONS {
        if i64::from(version) <= current {
            continue;
        }
        conn.execute_batch("BEGIN")
            .map_err(|e| format!("begin tx for migration {version}: {e}"))?;
        if let Err(e) = conn.execute_batch(sql) {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(format!("migration {version} failed: {e}"));
        }
        if let Err(e) = conn.execute(
            "INSERT INTO schema_version(version) VALUES (?)",
            params![version],
        ) {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(format!(
                "record schema_version after migration {version}: {e}"
            ));
        }
        conn.execute_batch("COMMIT")
            .map_err(|e| format!("commit migration {version}: {e}"))?;
    }
    Ok(())
}
