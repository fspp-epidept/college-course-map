//! `DuckDB` connection management + migration runner.
//!
//! One **shared `DuckDB` instance**, two `Connection`s behind `Mutex`es in
//! Tauri state: a read-write handle held briefly per write, and a read handle
//! cloned from it ([`Connection::try_clone`]) for list/dashboard reads. The
//! clone matters: a *separate* read-only instance (`open_with_flags`) is a
//! point-in-time snapshot frozen at open and never observes the RW instance's
//! later commits — so polling reads (`list_datasets`, `get_run`) would show an
//! import or run stuck at zero forever. Connections cloned from one instance
//! share `DuckDB`'s MVCC, so reads see committed writes immediately. The read
//! handle is therefore not access-mode read-only; it's only handed to read
//! commands by convention (`ro()` is `pub(crate)`), and the cached clone keeps
//! per-read open cost (tens to hundreds of ms under writer load) at zero. If
//! reads ever serialize badly under heavy concurrency, clone more handles into
//! a small pool rather than reopening per call.
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

use duckdb::{Connection, params};

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

/// Owned read-write and read-only connections plus the resolved on-disk path.
/// The path is kept for diagnostics and for tools that need to open their own
/// connection (e.g. examples that bypass `AppDb`).
pub struct AppDb {
    rw: Mutex<Connection>,
    ro: Mutex<Connection>,
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
    /// connection, apply any pending migrations, then clone a read handle off
    /// it. The clone is taken **after** migrations and shares the same instance,
    /// so it sees the migrated schema and every subsequent committed write.
    pub fn open() -> Result<Self, String> {
        let path = db_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        let rw = Connection::open(&path).map_err(|e| e.to_string())?;
        migrate(&rw)?;
        let ro = rw.try_clone().map_err(|e| e.to_string())?;
        Ok(Self {
            rw: Mutex::new(rw),
            ro: Mutex::new(ro),
            path,
        })
    }

    /// Borrow the read-write connection. The mutex is uncontended in single-user
    /// app flows; we hold it briefly per command. Stress test (#49/#50) will
    /// validate that this is fine under inference + dashboard concurrency.
    pub fn rw(&self) -> Result<MutexGuard<'_, Connection>, String> {
        self.rw.lock().map_err(|_| "rw mutex poisoned".to_owned())
    }

    /// Borrow the shared read connection (a clone of the RW instance, so it
    /// observes committed writes live). Cheap (no open cost), but serializes
    /// reads — fine while the only reader callers are the IPC list/dashboard
    /// queries. Read-only by convention, not by access mode: only read commands
    /// should take this handle. The `MutexGuard` derefs to `Connection` so
    /// existing `conn.prepare(...)` call sites are unchanged.
    pub(crate) fn ro(&self) -> Result<MutexGuard<'_, Connection>, String> {
        self.ro.lock().map_err(|_| "ro mutex poisoned".to_owned())
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
