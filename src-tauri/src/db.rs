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
//!
//! Upgrade safety (#204) sits around the migration runner, in [`upgrade`]:
//! a database a newer app wrote is refused with a plain-language message,
//! and one whose schema or `DuckDB` version is about to change is copied to
//! `<db>.pre-<app version>.bak` first. The `app_meta` table records which
//! app and `DuckDB` version last opened the file. Like `schema_version` it
//! is the runner's own bookkeeping, created here and not by a migration.

use std::{
    fs,
    io::{self, Read as _},
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

use duckdb::{Connection, params};

use crate::boot::{Phase, Progress};

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
    (
        5,
        include_str!("../migrations/0005_source_file_encoding.sql"),
    ),
    (
        6,
        include_str!("../migrations/0006_ccm_taxonomy_corrections.sql"),
    ),
    (
        7,
        include_str!("../migrations/0007_dataset_input_profile.sql"),
    ),
    (
        8,
        include_str!("../migrations/0008_cache_without_run_fk.sql"),
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
    /// `app_version` is the running app's version (Tauri's package info, not
    /// the crate's static one), recorded in `app_meta`. `progress` is the
    /// boot path's progress and cancel; tools pass [`Progress::none`]. Errors
    /// are user-facing text.
    pub fn open(app_version: &str, progress: &Progress<'_>) -> Result<Self, String> {
        Self::open_at(db_path()?, app_version, progress)
    }

    /// Same as [`AppDb::open`] but at an explicit path. For tools and the
    /// resume verification harness (`examples/check_resume.rs`), which must
    /// run the real migration + connection setup against a scratch database
    /// instead of the user's.
    pub fn open_at(
        path: PathBuf,
        app_version: &str,
        progress: &Progress<'_>,
    ) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        let (rw, recovery_notice) = match Connection::open(&path) {
            Ok(conn) => (conn, None),
            Err(e) if is_wal_replay_failure(&e) => {
                let set_aside = set_aside_wal(&path, &e)?;
                log::warn!(
                    "startup: WAL replay failed ({e}); set aside as {} and reopened",
                    set_aside.display()
                );
                let conn = Connection::open(&path).map_err(|e| e.to_string())?;
                (conn, Some(recovery_notice(&set_aside)))
            }
            Err(e) if is_newer_storage(&e.to_string()) => return Err(newer_data_message(None)),
            Err(e) => return Err(e.to_string()),
        };
        let rw = upgrade(rw, &path, app_version, progress)?;
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
    suffixed(db, ".wal")
}

/// `path` with `suffix` appended to its file name.
fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
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

/// `<local data>/college-course-map/app.duckdb`.
pub fn db_path() -> Result<PathBuf, String> {
    Ok(crate::paths::data_dir()?.join(DB_FILE))
}

/// How much of the database the pre-upgrade backup copies between progress
/// reports and cancel checks.
const BACKUP_CHUNK: u64 = 8 * 1024 * 1024;

/// What `app_meta` says about the last process that opened the database.
/// Both are `None` for a database from before the table existed (0.5.0 and
/// earlier) and for a fresh one.
#[derive(Debug, Default)]
struct Meta {
    app_version: Option<String>,
    duckdb_version: Option<String>,
}

impl Meta {
    fn read(conn: &Connection) -> Result<Self, String> {
        let mut meta = Self::default();
        if !table_exists(conn, "app_meta")? {
            return Ok(meta);
        }
        let mut stmt = conn
            .prepare("SELECT key, value FROM app_meta")
            .map_err(|e| format!("read app_meta: {e}"))?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get(1)?)))
            .map_err(|e| format!("read app_meta: {e}"))?;
        for row in rows {
            let (key, value) = row.map_err(|e| format!("read app_meta: {e}"))?;
            match key.as_str() {
                "app_version" => meta.app_version = Some(value),
                "duckdb_version" => meta.duckdb_version = Some(value),
                _ => {}
            }
        }
        Ok(meta)
    }
}

/// Bring an opened database up to this binary (#204): refuse one a newer app
/// wrote, back it up when its schema or `DuckDB` version is about to change,
/// apply the pending migrations, and record who opened it. Nothing is
/// written before the backup exists, and older backups go only once the
/// upgrade has succeeded.
fn upgrade(
    conn: Connection,
    path: &Path,
    app_version: &str,
    progress: &Progress<'_>,
) -> Result<Connection, String> {
    let schema = schema_version(&conn)?;
    let meta = Meta::read(&conn)?;
    let head = head_version();
    if schema > head {
        return Err(newer_data_message(meta.app_version.as_deref()));
    }
    let library = library_version(&conn)?;
    let changing = schema < head || meta.duckdb_version.as_deref() != Some(library.as_str());
    // A fresh database (schema 0) has nothing to lose.
    let (conn, backup) = if schema > 0 && changing {
        progress.phase(Phase::BackingUp);
        // Close before copying: the file is then complete on its own (no
        // WAL) and no handle of ours holds it, whatever sharing mode the
        // platform gave `DuckDB`.
        conn.execute_batch("CHECKPOINT")
            .map_err(|e| format!("checkpoint before backup: {e}"))?;
        conn.close()
            .map_err(|(_, e)| format!("close before backup: {e}"))?;
        let dest = suffixed(path, &format!(".pre-{app_version}.bak"));
        if let Some(name) = dest.file_name() {
            progress.detail(&format!("Saving a copy as {}", name.to_string_lossy()));
        }
        back_up(path, &dest, progress)?;
        log::info!(
            "startup: database backed up to {} before upgrading (schema {schema} -> {head}, \
             DuckDB {} -> {library})",
            dest.display(),
            meta.duckdb_version.as_deref().unwrap_or("unknown"),
        );
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        (conn, Some(dest))
    } else {
        (conn, None)
    };
    if schema < head {
        progress.phase(Phase::UpgradingSchema);
    }
    migrate(&conn, progress)?;
    stamp(&conn, &meta, app_version, &library)?;
    if let Some(keep) = backup {
        prune_backups(path, &keep);
    }
    Ok(conn)
}

/// The newest schema version this binary knows.
fn head_version() -> i64 {
    MIGRATIONS
        .last()
        .map_or(0, |&(version, _)| i64::from(version))
}

/// The database's schema version; 0 for a fresh database.
fn schema_version(conn: &Connection) -> Result<i64, String> {
    if !table_exists(conn, "schema_version")? {
        return Ok(0);
    }
    conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |row| row.get(0),
    )
    .map_err(|e| e.to_string())
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT COUNT(*) FROM information_schema.tables
         WHERE table_schema = 'main' AND table_name = ?",
        params![name],
        |row| row.get::<_, i64>(0),
    )
    .map(|count| count > 0)
    .map_err(|e| e.to_string())
}

/// The linked `DuckDB` release, e.g. `v1.5.3`.
fn library_version(conn: &Connection) -> Result<String, String> {
    conn.query_row("SELECT library_version FROM pragma_version()", [], |row| {
        row.get(0)
    })
    .map_err(|e| e.to_string())
}

/// `DuckDB` refuses a file whose storage format is newer than the library
/// with one of these two messages (`single_file_block_manager.cpp`). Within
/// 1.x it writes a format older releases still read, so the usual downgrade
/// is caught by the schema check in [`upgrade`]; this covers the day that
/// changes. Recheck the text on a `DuckDB` bump.
fn is_newer_storage(message: &str) -> bool {
    message.contains("Trying to read a database file with version number")
        || message.contains("written with a storage version greater than")
}

/// Shown when the database is newer than this binary. `app_version` is the
/// last version that opened it, so installing that one is always enough.
fn newer_data_message(app_version: Option<&str>) -> String {
    let (which, install) = match app_version {
        Some(v) => (format!(" ({v})"), format!("version {v} or later")),
        None => (String::new(), "the latest version".to_owned()),
    };
    format!(
        "This data was created by a newer version of the app{which}. Install {install} to \
         open it. Nothing was changed."
    )
}

/// Copy the closed database to `dest`, the pre-upgrade backup. An existing
/// `dest` is kept: migrations commit one at a time, so a retry after a
/// failed upgrade would otherwise replace the real pre-upgrade copy with a
/// half-migrated one. The copy reports to `progress` and stops when it is
/// cancelled.
fn back_up(db: &Path, dest: &Path, progress: &Progress<'_>) -> Result<(), String> {
    if dest.exists() {
        return Ok(());
    }
    if wal_path(db).exists() {
        return Err(backup_failed(db, "its write-ahead log is still present"));
    }
    let staging = suffixed(dest, ".tmp");
    copy_staged(db, &staging, dest, progress).map_err(|e| {
        let _ = fs::remove_file(&staging);
        backup_failed(db, &e.to_string())
    })
}

/// Stage, sync, rename: `dest` only ever appears complete, so a kill at any
/// point leaves at most a `.tmp` for the next attempt to replace.
fn copy_staged(src: &Path, staging: &Path, dest: &Path, progress: &Progress<'_>) -> io::Result<()> {
    match fs::remove_file(staging) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let mut from = fs::File::open(src)?;
    let total = from.metadata()?.len();
    let mut to = fs::File::create(staging)?;
    let mut done = 0;
    loop {
        if progress.cancelled() {
            return Err(io::Error::other("startup was cancelled"));
        }
        let copied = io::copy(&mut from.by_ref().take(BACKUP_CHUNK), &mut to)?;
        if copied == 0 {
            break;
        }
        done += copied;
        progress.report(done, total);
    }
    to.sync_all()?;
    drop(to);
    fs::rename(staging, dest)
}

fn backup_failed(db: &Path, cause: &str) -> String {
    let bytes = fs::metadata(db).map_or(0, |m| m.len());
    let tenths_gb = bytes / 100_000_000;
    let size = if tenths_gb >= 10 {
        format!("{}.{} GB", tenths_gb / 10, tenths_gb % 10)
    } else {
        format!("{} MB", bytes.div_ceil(1_000_000).max(1))
    };
    let dir = db.parent().unwrap_or(db).display();
    format!(
        "The app could not back up your data before updating ({cause}). The backup needs \
         about {size} free in {dir}. Free some space and start the app again. Nothing was \
         changed."
    )
}

/// Delete every pre-upgrade backup of `db` except `keep`. Only
/// `<db>.pre-*.bak` matches; other siblings are not this function's.
fn prune_backups(db: &Path, keep: &Path) {
    let (Some(dir), Some(name)) = (db.parent(), db.file_name()) else {
        return;
    };
    let prefix = format!("{}.pre-", name.to_string_lossy());
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
        let is_backup = path.file_name().is_some_and(|file| {
            let file = file.to_string_lossy();
            file.starts_with(&prefix) && file.ends_with(".bak")
        });
        if !is_backup || path == keep {
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => log::info!("startup: removed older backup {}", path.display()),
            Err(e) => log::warn!("startup: older backup {} not removed: {e}", path.display()),
        }
    }
}

/// Record this process in `app_meta`. Writes only what changed, so an
/// ordinary launch leaves the database untouched.
fn stamp(conn: &Connection, meta: &Meta, app_version: &str, library: &str) -> Result<(), String> {
    for (key, stored, value) in [
        ("app_version", &meta.app_version, app_version),
        ("duckdb_version", &meta.duckdb_version, library),
    ] {
        if stored.as_deref() == Some(value) {
            continue;
        }
        conn.execute(
            "INSERT INTO app_meta (key, value) VALUES (?, ?)
             ON CONFLICT (key) DO UPDATE
             SET value = excluded.value, updated_at = now()",
            params![key, value],
        )
        .map_err(|e| format!("record {key} in app_meta: {e}"))?;
    }
    Ok(())
}

pub(crate) fn migrate(conn: &Connection, progress: &Progress<'_>) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version    INTEGER PRIMARY KEY,
            applied_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE IF NOT EXISTS app_meta (
            key        VARCHAR PRIMARY KEY,
            value      VARCHAR NOT NULL,
            updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
        )",
    )
    .map_err(|e| format!("create schema_version and app_meta: {e}"))?;

    let current = schema_version(conn)?;

    let pending: Vec<_> = MIGRATIONS
        .iter()
        .filter(|&&(version, _)| i64::from(version) > current)
        .collect();
    for (index, &&(version, sql)) in pending.iter().enumerate() {
        progress.detail(&format!(
            "Applying update {} of {}",
            index + 1,
            pending.len()
        ));
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
        // 0006 empties the table so existing databases pick up the corrected
        // CSVs; fresh databases seed at 3 and again at 6.
        3 | 6 => seed_ccm_taxonomy(conn),
        _ => Ok(()),
    }
}

/// Insert the CCM taxonomy from CSVs embedded at compile time (converted from
/// the official `ccm_taxonomy_{two,six}.xlsx`, then corrected against the NCES
/// 2012-162rev PDF; see migrations 0003 and 0006). 2-digit rows carry `title_short`, 6-digit rows carry
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
    use std::path::{Path, PathBuf};

    use super::{
        AppDb, Meta, back_up, head_version, is_newer_storage, library_version, migrate,
        newer_data_message, schema_version, suffixed, wal_path,
    };
    use crate::boot::{Boot, Phase, Progress};

    /// The `DuckDB` release the pinned `duckdb` crate bundles. A bump moves
    /// the pin in Cargo.toml and this constant together, and
    /// `fixture_exists_for_head` then asks for a fixture written by the new
    /// version (`task db:fixture`).
    const DUCKDB_VERSION: &str = "v1.5.3";

    /// Tables a migration must carry rows across.
    const USER_TABLES: [&str; 6] = [
        "source_files",
        "datasets",
        "courses",
        "models",
        "runs",
        "inference_results",
    ];

    /// A fresh, empty directory for one test.
    fn scratch(name: &str) -> Result<PathBuf, String> {
        let root = std::env::temp_dir().join(format!("ccm-db-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        Ok(root)
    }

    fn raw(path: &Path) -> Result<duckdb::Connection, String> {
        duckdb::Connection::open(path).map_err(|e| e.to_string())
    }

    fn row_counts(conn: &duckdb::Connection) -> Result<Vec<i64>, String> {
        USER_TABLES
            .iter()
            .map(|table| {
                conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                    .map_err(|e| format!("count {table}: {e}"))
            })
            .collect()
    }

    /// Every committed upgrade fixture, oldest schema first.
    fn fixtures() -> Result<Vec<PathBuf>, String> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/db");
        let mut found: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map_err(|e| format!("read {}: {e}", dir.display()))?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.to_string_lossy().ends_with(".duckdb.gz"))
            .collect();
        found.sort();
        Ok(found)
    }

    /// Fixtures are stored gzipped; unpack one to `dest`.
    fn unpack(fixture: &Path, dest: &Path) -> Result<(), String> {
        let packed = std::fs::File::open(fixture).map_err(|e| e.to_string())?;
        let mut out = std::fs::File::create(dest).map_err(|e| e.to_string())?;
        std::io::copy(&mut flate2::read::GzDecoder::new(packed), &mut out)
            .map_err(|e| format!("unpack {}: {e}", fixture.display()))?;
        Ok(())
    }

    fn backups(root: &Path) -> Result<Vec<String>, String> {
        let mut names: Vec<String> = std::fs::read_dir(root)
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".pre-"))
            .collect();
        names.sort();
        Ok(names)
    }

    /// The pin, the constant and the linked library agree.
    #[test]
    fn duckdb_version_is_the_pinned_one() -> Result<(), String> {
        let conn = duckdb::Connection::open_in_memory().map_err(|e| e.to_string())?;
        assert_eq!(library_version(&conn)?, DUCKDB_VERSION);
        Ok(())
    }

    /// A fresh database is stamped and not backed up, and opening it again
    /// with the same app writes nothing.
    #[test]
    fn fresh_database_is_stamped_once() -> Result<(), String> {
        let root = scratch("fresh")?;
        let path = root.join("app.duckdb");
        let stamped_at = |db: &AppDb| -> Result<String, String> {
            db.rw()?
                .query_row(
                    "SELECT string_agg(updated_at::VARCHAR, ',' ORDER BY key) FROM app_meta",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())
        };

        let db = AppDb::open_at(path.clone(), "0.6.0", &Progress::none())?;
        let meta = Meta::read(&*db.rw()?)?;
        assert_eq!(meta.app_version.as_deref(), Some("0.6.0"));
        assert_eq!(meta.duckdb_version.as_deref(), Some(DUCKDB_VERSION));
        let first = stamped_at(&db)?;
        drop(db);

        let db = AppDb::open_at(path, "0.6.0", &Progress::none())?;
        assert_eq!(stamped_at(&db)?, first);
        assert!(backups(&root)?.is_empty());
        Ok(())
    }

    /// A database whose schema is ahead of this binary is refused before
    /// anything is written, and the message names the version to install.
    #[test]
    fn newer_schema_is_refused() -> Result<(), String> {
        let root = scratch("newer")?;
        let path = root.join("app.duckdb");
        drop(AppDb::open_at(path.clone(), "9.9.9", &Progress::none())?);
        raw(&path)?
            .execute(
                "INSERT INTO schema_version(version) VALUES (?)",
                [head_version() + 1],
            )
            .map_err(|e| e.to_string())?;

        let err = AppDb::open_at(path.clone(), "0.6.0", &Progress::none())
            .err()
            .ok_or("a newer database opened")?;
        assert_eq!(err, newer_data_message(Some("9.9.9")));
        assert!(err.contains("Install version 9.9.9 or later"), "{err}");
        assert!(backups(&root)?.is_empty());
        let conn = raw(&path)?;
        assert_eq!(schema_version(&conn)?, head_version() + 1);
        assert_eq!(Meta::read(&conn)?.app_version.as_deref(), Some("9.9.9"));
        Ok(())
    }

    /// `DuckDB`'s two refusals of a newer storage format, verbatim (the first
    /// reproduced with 1.4.4 against a v1.5.0-format file, the second from
    /// the 1.5.3 source), map to the plain-language message. Other open
    /// failures keep their own text.
    #[test]
    fn newer_storage_refusals_are_recognised() {
        assert!(is_newer_storage(
            "IO Error: Trying to read a database file with version number 68, but we can only \
             read versions between 64 and 67.\nThe database file was created with an newer \
             version of DuckDB."
        ));
        assert!(is_newer_storage(
            "Invalid Input Error: Error opening \"app.duckdb\": file was written with a storage \
             version greater than the latest version supported by this DuckDB instance. Try \
             opening the file with a newer version of DuckDB."
        ));
        assert!(!is_newer_storage(
            "IO Error: Could not set lock on file \"app.duckdb\": Conflicting lock is held"
        ));
        assert!(newer_data_message(None).contains("Install the latest version"));
    }

    /// A `DuckDB` version change alone triggers the backup. The new backup
    /// holds the pre-upgrade state, older backups go once the open has
    /// succeeded, and the compaction swap's `.bak` is not ours to delete.
    #[test]
    fn duckdb_change_backs_up_and_prunes() -> Result<(), String> {
        let root = scratch("duckdb-change")?;
        let path = root.join("app.duckdb");
        drop(AppDb::open_at(path.clone(), "0.6.0", &Progress::none())?);
        raw(&path)?
            .execute_batch("UPDATE app_meta SET value = 'v0.0.0' WHERE key = 'duckdb_version'")
            .map_err(|e| e.to_string())?;
        let older = suffixed(&path, ".pre-0.6.0.bak");
        let compaction = suffixed(&path, ".bak");
        std::fs::write(&older, b"older backup").map_err(|e| e.to_string())?;
        std::fs::write(&compaction, b"compaction").map_err(|e| e.to_string())?;

        let db = AppDb::open_at(path.clone(), "0.7.0", &Progress::none())?;
        let meta = Meta::read(&*db.rw()?)?;
        assert_eq!(meta.app_version.as_deref(), Some("0.7.0"));
        assert_eq!(meta.duckdb_version.as_deref(), Some(DUCKDB_VERSION));
        assert_eq!(backups(&root)?, ["app.duckdb.pre-0.7.0.bak"]);
        assert!(compaction.exists(), "compaction .bak was deleted");
        let backup = Meta::read(&raw(&suffixed(&path, ".pre-0.7.0.bak"))?)?;
        assert_eq!(backup.app_version.as_deref(), Some("0.6.0"));
        assert_eq!(backup.duckdb_version.as_deref(), Some("v0.0.0"));
        Ok(())
    }

    /// The boot screen names only work that happens: migrations enter
    /// `UpgradingSchema`, a backup enters `BackingUp` and reports its bytes,
    /// and an ordinary launch enters neither.
    #[test]
    fn phases_follow_the_work() -> Result<(), String> {
        let fixture = fixtures()?.into_iter().next().ok_or("no fixtures")?;
        let root = scratch("phases")?;
        let path = root.join("app.duckdb");
        unpack(&fixture, &path)?;
        let pending = head_version() - schema_version(&raw(&path)?)?;

        let boot = Boot::default();
        drop(AppDb::open_at(path.clone(), "0.6.0", &Progress::of(&boot))?);
        let state = boot.state()?;
        assert_eq!(state.phase, Some(Phase::UpgradingSchema));
        assert_eq!(
            state.detail,
            Some(format!("Applying update {pending} of {pending}"))
        );

        let boot = Boot::default();
        drop(AppDb::open_at(path.clone(), "0.6.0", &Progress::of(&boot))?);
        let state = boot.state()?;
        assert_eq!((state.phase, state.detail), (None, None));

        raw(&path)?
            .execute_batch("UPDATE app_meta SET value = 'v0.0.0' WHERE key = 'duckdb_version'")
            .map_err(|e| e.to_string())?;
        let boot = Boot::default();
        drop(AppDb::open_at(path, "0.7.0", &Progress::of(&boot))?);
        let state = boot.state()?;
        assert_eq!(state.phase, Some(Phase::BackingUp));
        assert_eq!(
            state.detail.as_deref(),
            Some("Saving a copy as app.duckdb.pre-0.7.0.bak")
        );
        assert!(state.total > 0 && state.done == state.total, "{state:?}");
        Ok(())
    }

    /// The copy is staged: a cancel or a stale `.tmp` never leaves a partial
    /// `.bak`, and an existing backup is kept as it is.
    #[test]
    fn backup_is_staged_and_never_overwritten() -> Result<(), String> {
        let root = scratch("staged")?;
        let src = root.join("app.duckdb");
        let dest = suffixed(&src, ".pre-test.bak");
        let staging = suffixed(&dest, ".tmp");
        std::fs::write(&src, b"database bytes").map_err(|e| e.to_string())?;

        let cancelled = Boot::cancelled();
        assert!(back_up(&src, &dest, &Progress::of(&cancelled)).is_err());
        assert!(!dest.exists() && !staging.exists());

        // A `.tmp` left by a killed copy is replaced.
        std::fs::write(&staging, b"half a copy").map_err(|e| e.to_string())?;
        back_up(&src, &dest, &Progress::none())?;
        assert_eq!(
            std::fs::read(&dest).map_err(|e| e.to_string())?,
            b"database bytes"
        );
        assert!(!staging.exists());

        std::fs::write(&src, b"half-migrated").map_err(|e| e.to_string())?;
        back_up(&src, &dest, &Progress::none())?;
        assert_eq!(
            std::fs::read(&dest).map_err(|e| e.to_string())?,
            b"database bytes"
        );
        Ok(())
    }

    /// No backup, no migration: when the copy cannot be written the open
    /// fails and the database stays at its old schema.
    #[test]
    fn failed_backup_blocks_migration() -> Result<(), String> {
        let fixture = fixtures()?.into_iter().next().ok_or("no fixtures")?;
        let root = scratch("backup-fails")?;
        let path = root.join("app.duckdb");
        unpack(&fixture, &path)?;
        let before = schema_version(&raw(&path)?)?;
        assert!(before < head_version(), "oldest fixture is already at head");
        // A directory squatting on the staging name makes the copy fail.
        std::fs::create_dir(suffixed(&path, ".pre-test.bak.tmp")).map_err(|e| e.to_string())?;

        let err = AppDb::open_at(path.clone(), "test", &Progress::none())
            .err()
            .ok_or("opened without a backup")?;
        assert!(err.contains("could not back up your data"), "{err}");
        assert!(err.contains("Nothing was changed"), "{err}");
        assert_eq!(schema_version(&raw(&path)?)?, before);
        Ok(())
    }

    /// The state this binary writes has a fixture (#204). Fails after adding
    /// a migration or re-pinning `DuckDB` until `task db:fixture` is run.
    #[test]
    fn fixture_exists_for_head() -> Result<(), String> {
        // Same format as examples/db_fixture.rs.
        let name = format!(
            "schema-v{}_duckdb-{DUCKDB_VERSION}.duckdb.gz",
            head_version()
        );
        assert!(
            fixtures()?.iter().any(|path| path.ends_with(&name)),
            "fixtures/db/{name} is missing: run `task db:fixture` and commit the file"
        );
        Ok(())
    }

    /// Every fixture (a database as an earlier schema or `DuckDB` version
    /// left it on disk) upgrades to head behind a backup of its original
    /// state, keeps its rows, and survives a write, close and reopen.
    #[test]
    fn fixtures_upgrade_and_round_trip() -> Result<(), String> {
        for fixture in fixtures()? {
            let label = fixture
                .file_stem()
                .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
            let root = scratch(&label)?;
            let path = root.join("app.duckdb");
            unpack(&fixture, &path)?;
            let (schema, written_by, before) = {
                let conn = raw(&path)?;
                (
                    schema_version(&conn)?,
                    Meta::read(&conn)?.duckdb_version,
                    row_counts(&conn)?,
                )
            };

            let db = AppDb::open_at(path.clone(), "test", &Progress::none())?;
            {
                let conn = db.rw()?;
                assert_eq!(schema_version(&conn)?, head_version(), "{label}");
                assert_eq!(row_counts(&conn)?, before, "{label}");
                let meta = Meta::read(&conn)?;
                assert_eq!(meta.app_version.as_deref(), Some("test"), "{label}");
                assert_eq!(meta.duckdb_version.as_deref(), Some(DUCKDB_VERSION));
                conn.execute_batch(
                    "INSERT INTO source_files (path, display_name, imported_at, imported_hash)
                     VALUES ('round-trip.csv', 'round trip', now(), 'round-trip')",
                )
                .map_err(|e| e.to_string())?;
            }

            let backup = suffixed(&path, ".pre-test.bak");
            let changed = schema < head_version() || written_by.as_deref() != Some(DUCKDB_VERSION);
            assert_eq!(backup.exists(), changed, "{label}");
            if changed {
                let conn = raw(&backup)?;
                assert_eq!(schema_version(&conn)?, schema, "{label}");
                assert_eq!(row_counts(&conn)?, before, "{label}");
            }

            db.checkpoint()?;
            drop(db);
            let db = AppDb::open_at(path, "test", &Progress::none())?;
            let written: i64 = db
                .rw()?
                .query_row(
                    "SELECT COUNT(*) FROM source_files WHERE path = 'round-trip.csv'",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            assert_eq!(written, 1, "{label}");
        }
        Ok(())
    }

    /// 0008 on a populated database (the v7 fixture: cached results whose
    /// `computed_by_run` references a run): the run can now be deleted and
    /// its results stay, the cache keeps its primary key and its foreign key
    /// to `models`, a `models` row nothing references can still be deleted
    /// (the `RENAME` hazard the migration avoids), and the secondary indexes
    /// are gone.
    #[test]
    fn cache_rebuild_frees_runs_and_drops_secondary_indexes() -> Result<(), String> {
        let fixture = fixtures()?
            .into_iter()
            .find(|path| path.to_string_lossy().contains("schema-v7_"))
            .ok_or("no v7 fixture")?;
        let root = scratch("cache-rebuild")?;
        let path = root.join("app.duckdb");
        unpack(&fixture, &path)?;
        let db = AppDb::open_at(path, "test", &Progress::none())?;
        let conn = db.rw()?;
        let count = |sql: &str| -> Result<i64, String> {
            conn.query_row(sql, [], |r| r.get(0))
                .map_err(|e| format!("{sql}: {e}"))
        };

        let results = count("SELECT COUNT(*) FROM inference_results")?;
        assert!(results > 0, "fixture has no cached results");
        assert_eq!(
            count(
                "SELECT COUNT(*) FROM inference_results ir
                 JOIN runs r ON r.id = ir.computed_by_run"
            )?,
            results,
            "fixture results don't reference a run"
        );
        conn.execute_batch("DELETE FROM runs")
            .map_err(|e| format!("delete runs: {e}"))?;
        assert_eq!(count("SELECT COUNT(*) FROM inference_results")?, results);

        let duplicate = conn.execute_batch(
            "INSERT INTO inference_results (model_id, content_hash, classification, computed_at)
             SELECT model_id, content_hash, 'x', now() FROM inference_results LIMIT 1",
        );
        assert!(duplicate.is_err(), "primary key not enforced");
        let orphan = conn.execute_batch(
            "INSERT INTO inference_results (model_id, content_hash, classification, computed_at)
             VALUES (-1, 'h', 'x', now())",
        );
        assert!(orphan.is_err(), "models foreign key not enforced");
        conn.execute_batch(
            "INSERT INTO models (id, hf_repo, hf_revision, model_type, precision)
             VALUES (-1, 'r', 'v', '2', 'f32');
             DELETE FROM models WHERE id = -1;",
        )
        .map_err(|e| format!("delete an unreferenced models row: {e}"))?;

        assert_eq!(
            count(
                "SELECT COUNT(*) FROM duckdb_indexes()
                 WHERE table_name IN ('courses', 'inference_results')"
            )?,
            0,
            "secondary indexes remain"
        );
        Ok(())
    }

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
        drop(AppDb::open_at(path.clone(), "test", &Progress::none())?);
        std::fs::copy(&foreign_wal, wal_path(&path)).map_err(|e| e.to_string())?;

        let db = AppDb::open_at(path.clone(), "test", &Progress::none())?;
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
        assert!(
            AppDb::open_at(path, "test", &Progress::none())?
                .recovery_notice()
                .is_none()
        );

        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    /// Full migration chain on a fresh database: schema applies, the 0003
    /// data hook seeds the taxonomy inside the same transaction, and a second
    /// `migrate` call is a no-op (no duplicate seeding).
    #[test]
    fn migrations_apply_and_seed_taxonomy() -> Result<(), String> {
        let conn = duckdb::Connection::open_in_memory().map_err(|e| e.to_string())?;
        migrate(&conn, &Progress::none())?;

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

        // 0005: source file encoding, defaulting pre-existing rows to UTF-8.
        conn.execute_batch(
            "INSERT INTO source_files (path, display_name, imported_at, imported_hash)
             VALUES ('a.csv', 'a', now(), 'h')",
        )
        .map_err(|e| e.to_string())?;
        let encoding: Option<String> = conn
            .query_row("SELECT encoding FROM source_files", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        assert_eq!(encoding.as_deref(), Some("utf-8"));

        // 0006: corrected CSVs re-seeded (mojibake, truncated title, and
        // neighbour-description contamination fixed against the NCES PDF).
        let six = |code: &str| -> Result<(String, String), String> {
            conn.query_row(
                "SELECT title, description FROM ccm_taxonomy
                 WHERE digit_level = 6 AND code = ?",
                [code],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| e.to_string())
        };
        assert_eq!(six("05.0207")?.0, "Women\u{2019}s Studies.");
        assert!(six("13.9998")?.0.ends_with("Group Process in Education."));
        assert!(
            six("40.0201")?
                .1
                .starts_with("A general course that focuses on the planetary")
        );

        // 0007: the import worker's input profile, NULL for older datasets.
        conn.prepare("SELECT input_profile FROM datasets")
            .map_err(|e| e.to_string())?;

        // Re-running is a no-op: schema_version gates both SQL and data hook.
        migrate(&conn, &Progress::none())?;
        assert_eq!(count(2)?, 48);
        Ok(())
    }
}
