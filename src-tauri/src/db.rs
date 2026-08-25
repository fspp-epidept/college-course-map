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
    path::{Path, PathBuf},
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
    (
        3,
        include_str!("../migrations/0003_ccm_taxonomy_and_confidence.sql"),
    ),
    (
        4,
        include_str!("../migrations/0004_roundtrip_export_top5.sql"),
    ),
];

/// Owned read-write and read-only connections plus the resolved on-disk path.
/// The path is kept for diagnostics and for tools that need to open their own
/// connection (e.g. examples that bypass `AppDb`).
pub struct AppDb {
    rw: Mutex<Connection>,
    ro: Mutex<Connection>,
    path: PathBuf,
    /// Set when [`AppDb::open_at`] had to set an unreplayable WAL aside to
    /// open at all (EPI-105). User-facing copy; surfaced through the
    /// runtime notices in Settings.
    recovery_notice: Option<String>,
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
        Self::open_at(db_path()?)
    }

    /// Same as [`AppDb::open`] but at an explicit path. For tools and the
    /// resume verification harness (`examples/check_resume.rs`), which must
    /// run the real migration + connection setup against a scratch database
    /// instead of the user's.
    pub fn open_at(path: PathBuf) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        let (rw, recovery_notice) = match Connection::open(&path) {
            Ok(conn) => (conn, None),
            Err(e) if is_wal_replay_failure(&e) => {
                let set_aside = set_aside_wal(&path, &e)?;
                eprintln!(
                    "startup: WAL replay failed ({e}); set aside as {} and reopened",
                    set_aside.display()
                );
                let conn = Connection::open(&path).map_err(|e| e.to_string())?;
                (conn, Some(recovery_notice(&set_aside)))
            }
            Err(e) => return Err(e.to_string()),
        };
        migrate(&rw)?;
        let ro = rw.try_clone().map_err(|e| e.to_string())?;
        Ok(Self {
            rw: Mutex::new(rw),
            ro: Mutex::new(ro),
            path,
            recovery_notice,
        })
    }

    /// The user-facing notice from a WAL set-aside at open, if one happened.
    #[must_use]
    pub fn recovery_notice(&self) -> Option<&str> {
        self.recovery_notice.as_deref()
    }

    /// Fold the WAL into the main file (EPI-105). Called after the startup
    /// writes and on clean exit so an unclean exit later orphans the
    /// smallest possible WAL — replay is the one open step this crate can't
    /// make safe (duckdb/duckdb#19712). A plain `CHECKPOINT` errors
    /// harmlessly when another transaction is open; callers log and go on.
    pub fn checkpoint(&self) -> Result<(), String> {
        self.rw()?
            .execute_batch("CHECKPOINT")
            .map_err(|e| e.to_string())
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

/// `DuckDB` wraps every exception raised while replaying `<db>.wal` at open
/// in this prefix (`WriteAheadLog::Replay`). Matching on it — not on any
/// open failure — keeps the set-aside below from ever touching the WAL of a
/// database that failed to open for another reason (a lock held by a second
/// instance, a missing directory), where the WAL is live data.
fn is_wal_replay_failure(e: &duckdb::Error) -> bool {
    e.to_string().contains("replaying WAL")
}

/// Move `<db>.wal` to `<db>.wal.corrupt-<unix seconds>` so the next open
/// starts from the last checkpoint (EPI-105). The file is kept as forensic
/// evidence for the upstream replay bug, never deleted. A replay failure
/// with no WAL on disk is something else entirely — the original error
/// stands.
fn set_aside_wal(db: &Path, cause: &duckdb::Error) -> Result<PathBuf, String> {
    let wal = wal_path(db);
    if !wal.exists() {
        return Err(cause.to_string());
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut set_aside = wal.clone().into_os_string();
    set_aside.push(format!(".corrupt-{secs}"));
    let set_aside = PathBuf::from(set_aside);
    fs::rename(&wal, &set_aside).map_err(|e| {
        format!("WAL replay failed ({cause}) and the WAL could not be set aside: {e}")
    })?;
    Ok(set_aside)
}

/// `DuckDB`'s WAL sits next to the database file as `<db>.wal`.
fn wal_path(db: &Path) -> PathBuf {
    let mut wal = db.as_os_str().to_owned();
    wal.push(".wal");
    PathBuf::from(wal)
}

fn recovery_notice(set_aside: &Path) -> String {
    let name = set_aside.file_name().map_or_else(
        || set_aside.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    format!(
        "The database's write-ahead log could not be replayed at startup and was set \
         aside as \"{name}\", so changes since the last checkpoint were not recovered. \
         Nothing else is affected: classifications recompute from the results cache \
         on the next run, and an interrupted import can be re-imported. The set-aside \
         file is kept for diagnosis."
    )
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
        if let Err(e) = post_migration(version, conn) {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(format!("data hook for migration {version} failed: {e}"));
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

/// Rust data hooks that run inside a migration's transaction, after its SQL.
/// For embedded-data seeding that plain SQL can't express cleanly (CSV loads).
/// Same once-per-database semantics as the SQL: tracked by the identical
/// `schema_version` row, rolled back together on failure. This is the single
/// place database state gets established — don't add seeding at startup.
fn post_migration(version: u32, conn: &Connection) -> Result<(), String> {
    match version {
        3 => seed_ccm_taxonomy(conn),
        _ => Ok(()),
    }
}

/// Insert the CCM taxonomy from CSVs embedded at compile time (sourced from
/// the official `ccm_taxonomy_{two,six}.xlsx`, converted + whitespace-cleaned;
/// see migration 0003). 2-digit rows carry `title_short`, 6-digit rows carry
/// `description`; the government publishes no 4-digit taxonomy.
fn seed_ccm_taxonomy(conn: &Connection) -> Result<(), String> {
    insert_taxonomy_csv(
        conn,
        2,
        include_str!("../migrations/data/ccm_taxonomy_two.csv"),
    )?;
    insert_taxonomy_csv(
        conn,
        6,
        include_str!("../migrations/data/ccm_taxonomy_six.csv"),
    )
}

fn insert_taxonomy_csv(conn: &Connection, digit_level: u8, data: &str) -> Result<(), String> {
    let mut reader = csv::Reader::from_reader(data.as_bytes());
    let headers = reader
        .headers()
        .map_err(|e| format!("taxonomy csv headers: {e}"))?;
    // Column 3 is `title_short` for the 2-digit file, `description` for the
    // 6-digit file; route it to the matching table column.
    let third_is_short = headers.get(2) == Some("title_short");
    let mut stmt = conn
        .prepare(
            "INSERT INTO ccm_taxonomy (digit_level, code, title, title_short, description)
             VALUES (?, ?, ?, ?, ?)",
        )
        .map_err(|e| format!("prepare taxonomy insert: {e}"))?;
    for record in reader.records() {
        let record = record.map_err(|e| format!("taxonomy csv record: {e}"))?;
        let code = record
            .get(0)
            .ok_or_else(|| "taxonomy csv row missing code".to_owned())?;
        let title = record
            .get(1)
            .ok_or_else(|| format!("taxonomy csv row {code} missing title"))?;
        let third = record.get(2).unwrap_or_default();
        let (title_short, description) = if third_is_short {
            (Some(third), None)
        } else {
            (None, Some(third))
        };
        stmt.execute(params![digit_level, code, title, title_short, description])
            .map_err(|e| format!("insert taxonomy row {code}: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{AppDb, migrate, wal_path};

    /// A WAL `DuckDB` can't replay must not keep the app from opening
    /// (EPI-105): it's set aside under a `.corrupt-*` name, the database
    /// opens at its last checkpoint, and the notice is set. The fixture
    /// reproduces the field error exactly — `INTERNAL Error: Failure while
    /// replaying WAL file ... GetDefaultDatabase with no default database
    /// set` — by pairing a database with a WAL whose entries reference a
    /// table it doesn't have (`DuckDB` fumbles the catalog miss during
    /// replay into that internal error).
    #[test]
    fn open_at_sets_aside_unreplayable_wal() -> Result<(), String> {
        let root = std::env::temp_dir().join(format!("ccm-wal-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let donor = root.join("donor.duckdb");
        let foreign_wal = {
            let conn = duckdb::Connection::open(&donor).map_err(|e| e.to_string())?;
            conn.execute_batch(
                "CREATE TABLE not_in_app(x INTEGER); CHECKPOINT;
                 INSERT INTO not_in_app VALUES (1), (2), (3);",
            )
            .map_err(|e| e.to_string())?;
            // Copy while the connection is open — dropping it checkpoints
            // and truncates the WAL.
            let kept = root.join("kept.wal");
            std::fs::copy(wal_path(&donor), &kept).map_err(|e| e.to_string())?;
            kept
        };

        let path = root.join("app.duckdb");
        drop(AppDb::open_at(path.clone())?);
        std::fs::copy(&foreign_wal, wal_path(&path)).map_err(|e| e.to_string())?;

        let db = AppDb::open_at(path.clone())?;
        let notice = db.recovery_notice().ok_or("no recovery notice")?;
        assert!(notice.contains(".wal.corrupt-"), "notice = {notice}");
        assert!(!wal_path(&path).exists(), "bad WAL still in place");
        let set_aside = std::fs::read_dir(&root)
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .contains("app.duckdb.wal.corrupt-")
            });
        assert!(set_aside, "set-aside WAL missing");
        // The database is usable: schema is migrated and writable.
        db.rw()?
            .execute_batch("SELECT COUNT(*) FROM ccm_taxonomy")
            .map_err(|e| e.to_string())?;
        db.checkpoint()?;
        // A healthy open carries no notice.
        drop(db);
        assert!(AppDb::open_at(path)?.recovery_notice().is_none());

        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    /// Full migration chain on a fresh database: schema applies, the 0003
    /// data hook seeds the taxonomy inside the same transaction, and a second
    /// `migrate` call is a no-op (no duplicate seeding).
    #[test]
    fn migrations_apply_and_seed_taxonomy() -> Result<(), String> {
        let conn = duckdb::Connection::open_in_memory().map_err(|e| e.to_string())?;
        migrate(&conn)?;

        let count = |level: i64| -> Result<i64, String> {
            conn.query_row(
                "SELECT COUNT(*) FROM ccm_taxonomy WHERE digit_level = ?",
                [level],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())
        };
        assert_eq!(count(2)?, 48);
        assert_eq!(count(6)?, 2119);

        // Third CSV column routes to the right table column per digit level.
        let (title, short, desc): (String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT title, title_short, description
                 FROM ccm_taxonomy WHERE digit_level = 2 AND code = '01'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(|e| e.to_string())?;
        assert!(title.starts_with("Agriculture"), "title = {title}");
        assert_eq!(short.as_deref(), Some("Agriculture"));
        assert_eq!(desc, None);

        let desc6: Option<String> = conn
            .query_row(
                "SELECT description FROM ccm_taxonomy
                 WHERE digit_level = 6 AND code = '01.0000'",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        assert!(desc6.is_some_and(|d| !d.is_empty()));

        // 0003 also added the research-signal column to the cache.
        conn.prepare("SELECT logit_argmax FROM inference_results")
            .map_err(|e| e.to_string())?;

        // 0004: round-trip header storage + top-5 candidate columns.
        conn.prepare("SELECT original_headers FROM source_files")
            .map_err(|e| e.to_string())?;
        conn.prepare(
            "SELECT top2_code, top2_prob, top3_code, top3_prob,
                    top4_code, top4_prob, top5_code, top5_prob
             FROM inference_results",
        )
        .map_err(|e| e.to_string())?;

        // Re-running is a no-op: schema_version gates both SQL and data hook.
        migrate(&conn)?;
        assert_eq!(count(2)?, 48);
        Ok(())
    }
}
