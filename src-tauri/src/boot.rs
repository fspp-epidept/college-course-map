//! Startup order and the one shutdown path (#229).
//!
//! Startup reads top to bottom here:
//!
//! 1. `tauri-plugin-single-instance` (registered first in `lib.rs`): a second
//!    launch focuses the running instance and exits, before anything below
//!    touches shared state.
//! 2. [`plugin`]: manages [`Boot`], then runs [`PRE_LOGGER`], the steps that
//!    must precede the log plugin. They run on the main thread with no window
//!    and nothing logged, so they stay fast; the one wait is the instance
//!    lock, while a previous process finishes exiting.
//! 3. The log plugin, then opener and dialog.
//! 4. `setup()` → [`start`]: the always-managed state and macOS decorations,
//!    then [`STEPS`] in order on the `boot` thread, so the window paints
//!    while they run. When every step has run, the services they built are
//!    published to [`Boot`], the boot state turns `Ready`, and model autoload
//!    starts. A step error turns it `Failed` and leaves the app running, so
//!    the boot screen can show it (#224).
//!
//! Adding a startup step is adding a row to one of the two tables. Every exit
//! goes through `RunEvent::Exit`, whose app-side body is [`shutdown`].
//!
//! Rules for steps:
//! - A step that moves or deletes files must be safe to kill at any point:
//!   stage, fsync, atomic rename, then clean up. Each step sweeps its own
//!   leftovers when it starts, so the next launch finishes what a killed one
//!   left. The shutdown wait is for cleanliness; correctness must not depend
//!   on it.
//! - No main-thread-blocking Tauri API (menu item mutations, window getters)
//!   on the boot path; post main-thread work with `run_on_main_thread`.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, Manager as _};
use tauri_specta::Event;

use crate::{
    config,
    db::{self, AppDb},
    import, inference, manifest,
    manifest::ModelCatalog,
    models, reset, runs, runtime,
};

/// The startup phases, in order. The boot screen titles each one, so a
/// phase names only work that is really happening: `UpgradingSchema` is
/// entered only while a schema migration runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
pub enum Phase {
    MigratingData,
    OpeningDatabase,
    UpgradingSchema,
    LoadingRuntime,
}

/// One startup step: a row in [`PRE_LOGGER`] or [`STEPS`]. `name` is for
/// the log; `label` is what the boot screen says while the step runs, so it
/// must be true whether or not the step finds work to do.
struct Step {
    phase: Phase,
    name: &'static str,
    label: &'static str,
    run: fn(&mut Ctx<'_>) -> Result<(), String>,
}

/// Steps that run inside [`plugin`], after the single-instance gate and
/// before the log plugin opens `logs/app.log`. All of them prepare the data
/// dir for what follows, hence [`Phase::MigratingData`].
const PRE_LOGGER: &[Step] = &[
    // Termination signals take the same exit path as closing the window
    // (#227). Installed first so a signal during any later step is queued
    // as an event-loop exit instead of killing the process.
    #[cfg(unix)]
    Step {
        phase: Phase::MigratingData,
        name: "install signal handlers",
        label: "Starting up",
        run: install_signals,
    },
    // One process at a time owns the data dir (#233). The single-instance
    // gate only focuses a running window; this also covers a process that
    // starts while the previous one is still exiting.
    Step {
        phase: Phase::MigratingData,
        name: "acquire instance lock",
        label: "Starting up",
        run: acquire_instance_lock,
    },
    // A pending "Reset app data" (#206): renames only, so it is fast and
    // safe to kill. After the lock, so the previous process is gone; before
    // the log plugin, which would reopen `logs/` in the data dir.
    Step {
        phase: Phase::MigratingData,
        name: "apply pending reset",
        label: "Starting up",
        run: apply_reset,
    },
];

/// Steps that run from `setup()`, in order.
const STEPS: &[Step] = &[
    // Before the migration, which would otherwise move a Roaming trash dir.
    Step {
        phase: Phase::MigratingData,
        name: "sweep reset trash",
        label: "Checking for data left over from a reset",
        run: sweep_reset_trash,
    },
    Step {
        phase: Phase::MigratingData,
        name: "migrate data",
        label: "Checking for app data in the old location",
        run: migrate_data,
    },
    Step {
        phase: Phase::OpeningDatabase,
        name: "open database",
        label: "Opening the database file",
        run: open_database,
    },
    Step {
        phase: Phase::OpeningDatabase,
        name: "sweep runs",
        label: "Checking for runs interrupted last time",
        run: sweep_runs,
    },
    Step {
        phase: Phase::OpeningDatabase,
        name: "sweep imports",
        label: "Checking for imports interrupted last time",
        run: sweep_imports,
    },
    Step {
        phase: Phase::OpeningDatabase,
        name: "manifest rows",
        label: "Registering the classification models",
        run: manifest_rows,
    },
    Step {
        phase: Phase::OpeningDatabase,
        name: "checkpoint",
        label: "Saving startup changes to disk",
        run: checkpoint,
    },
    Step {
        phase: Phase::LoadingRuntime,
        name: "load runtime",
        label: "Loading the classification engine",
        run: load_runtime,
    },
];

/// Where startup is. `Failed` keeps the app running with the boot screen
/// showing `message`; the phase it failed in is [`BootState::phase`].
#[derive(Debug, Clone, Default, Serialize, Type)]
#[serde(tag = "status", rename_all = "camelCase")]
pub(crate) enum BootStatus {
    #[default]
    Starting,
    Ready,
    /// `notices` are the non-fatal conditions collected before the failure,
    /// which often explain it. `log_dir` is for the copied error report.
    #[serde(rename_all = "camelCase")]
    Failed {
        message: String,
        notices: Vec<String>,
        log_dir: Option<String>,
    },
}

/// What the boot screen renders. `total == 0` means no total is known yet
/// (indeterminate progress). `seq` grows with every change, so a client
/// that subscribed and then fetched a snapshot keeps whichever is newer.
#[derive(Debug, Clone, Default, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BootState {
    pub seq: u64,
    pub status: BootStatus,
    pub phase: Option<Phase>,
    /// What the current step is doing, in the user's words.
    pub detail: Option<String>,
    pub done: u64,
    pub total: u64,
}

/// Emitted on every boot state change, progress throttled to
/// [`REPORT_INTERVAL`].
#[derive(Type, Serialize, Debug, Clone, Event)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BootStateChanged {
    pub state: BootState,
}

/// The boot state now, for a client that mounts mid-startup or after it.
#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn boot_state(boot: tauri::State<'_, Boot>) -> Result<BootState, String> {
    boot.state
        .lock()
        .map(|tracked| tracked.state.clone())
        .map_err(|_| "boot state lock poisoned".to_owned())
}

/// The least time between two progress events.
const REPORT_INTERVAL: Duration = Duration::from_millis(100);

/// The boot state and when it was last emitted, for the throttle.
#[derive(Default)]
struct Tracked {
    state: BootState,
    emitted: Option<Instant>,
}

/// What commands need from startup, published once every step has run.
pub(crate) struct Services {
    pub db: AppDb,
    pub catalog: ModelCatalog,
    pub runtime: runtime::RuntimeState,
}

/// Startup state, managed before any command can run. Commands reach the
/// database, catalog and runtime through [`Boot::ready`].
#[derive(Default)]
pub(crate) struct Boot {
    services: OnceLock<Services>,
    /// `session.lock`, held for the life of the process. Never unlocked: the
    /// OS releases it when the process ends, however it ends.
    instance_lock: OnceLock<File>,
    /// Set by [`shutdown`]; the runner stops at the next row.
    cancel: AtomicBool,
    /// Where startup is, for [`boot_state`] and [`BootStateChanged`].
    state: Mutex<Tracked>,
    /// Set by [`start`]. Until then (the [`PRE_LOGGER`] steps, with no
    /// window yet) state changes are recorded but not emitted.
    app: OnceLock<AppHandle>,
    /// Notices from [`PRE_LOGGER`], held until [`start`] can log them and
    /// hand them on with its own.
    pre_logger_notices: Mutex<Vec<String>>,
    /// Whether the step runner is done, for [`shutdown`] to wait on.
    finished: Mutex<bool>,
    finished_cv: Condvar,
}

impl Boot {
    /// The services, or an error until startup has finished.
    pub(crate) fn ready(&self) -> Result<&Services, String> {
        self.services
            .get()
            .ok_or_else(|| "The app is still starting.".to_owned())
    }

    /// Turn the boot state `Failed`.
    fn fail(&self, message: String, notices: Vec<String>) {
        let log_dir = crate::logging::logs_dir()
            .ok()
            .map(|dir| dir.display().to_string());
        self.update(false, |state| {
            state.status = BootStatus::Failed {
                message,
                notices,
                log_dir,
            };
        });
    }

    /// Apply `change` to the boot state and emit it, unless `throttled` and
    /// the last emit was under [`REPORT_INTERVAL`] ago. Emitting only posts
    /// to the event loop, so this is safe on the boot thread.
    fn update(&self, throttled: bool, change: impl FnOnce(&mut BootState)) {
        let Ok(mut tracked) = self.state.lock() else {
            return;
        };
        change(&mut tracked.state);
        tracked.state.seq += 1;
        let now = Instant::now();
        if throttled
            && tracked
                .emitted
                .is_some_and(|at| now.duration_since(at) < REPORT_INTERVAL)
        {
            return;
        }
        tracked.emitted = Some(now);
        let event = BootStateChanged {
            state: tracked.state.clone(),
        };
        drop(tracked);
        if let Some(app) = self.app.get()
            && let Err(e) = event.emit(app)
        {
            log::warn!("startup: emit boot state: {e}");
        }
    }
}

#[cfg(test)]
impl Boot {
    /// A `Boot` whose shutdown has already asked startup to stop.
    pub(crate) fn cancelled() -> Self {
        Self {
            cancel: AtomicBool::new(true),
            ..Self::default()
        }
    }
}

/// Marks the step runner finished when dropped, so [`shutdown`] stops waiting
/// however the runner ends: published, failed, or panicked. A panic also
/// turns the boot state `Failed`, so the screen doesn't wait forever.
struct Finished<'a>(&'a Boot);

impl Drop for Finished<'_> {
    fn drop(&mut self) {
        if thread::panicking() {
            self.0.fail(
                "Startup stopped unexpectedly. The log has the details.".to_owned(),
                Vec::new(),
            );
        }
        if let Ok(mut finished) = self.0.finished.lock() {
            *finished = true;
        }
        self.0.finished_cv.notify_all();
    }
}

/// [`Boot::ready`] for code holding an `AppHandle` rather than a command's
/// `State`.
pub(crate) fn services(app: &AppHandle) -> Result<&Services, String> {
    app.state::<Boot>().inner().ready()
}

/// Progress and cancel for a long step, free of Tauri so code below the boot
/// path (`AppDb::open_at`, a data-dir copy loop) and its tests can take one.
/// [`Progress::none`] is the form for callers outside startup.
pub struct Progress<'a> {
    boot: Option<&'a Boot>,
}

impl std::fmt::Debug for Progress<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Progress")
            .field("attached", &self.boot.is_some())
            .finish()
    }
}

impl<'a> Progress<'a> {
    /// Never cancelled, reports nowhere.
    #[must_use]
    pub fn none() -> Self {
        Self { boot: None }
    }

    pub(crate) fn of(boot: &'a Boot) -> Self {
        Self { boot: Some(boot) }
    }

    /// Report progress within the current step. Callers report as often as
    /// they like; the event is throttled here, except for the last report
    /// (`done >= total`).
    pub fn report(&self, done: u64, total: u64) {
        if let Some(boot) = self.boot {
            boot.update(done < total, |state| {
                state.done = done;
                state.total = total;
            });
        }
    }

    /// Record the phase startup is in. The runner sets each step's phase; a
    /// step that spans two (`AppDb::open_at` moving on to a backup or
    /// migration) moves it on itself, so a failure reports the right one.
    pub fn phase(&self, phase: Phase) {
        if let Some(boot) = self.boot {
            boot.update(false, |state| {
                state.phase = Some(phase);
                state.detail = None;
                state.done = 0;
                state.total = 0;
            });
        }
    }

    /// Say what the current step is doing, in the user's words. The runner
    /// sets each step's label; a step says more when it finds real work
    /// (moving data out of Roaming, creating a new database).
    pub fn detail(&self, text: &str) {
        if let Some(boot) = self.boot {
            boot.update(false, |state| {
                state.detail = Some(text.to_owned());
                state.done = 0;
                state.total = 0;
            });
        }
    }

    /// The phase last recorded, if any.
    fn recorded(&self) -> Option<Phase> {
        self.boot.and_then(|boot| {
            boot.state
                .lock()
                .ok()
                .and_then(|tracked| tracked.state.phase)
        })
    }

    /// Whether shutdown has asked startup to stop. Long steps check it
    /// between files or chunks.
    #[must_use]
    pub fn cancelled(&self) -> bool {
        self.boot
            .is_some_and(|boot| boot.cancel.load(Ordering::Relaxed))
    }
}

/// What a step gets: the app, progress and cancel, notices for Settings, and
/// the slots steps fill.
pub(crate) struct Ctx<'a> {
    pub app: &'a AppHandle,
    pub progress: Progress<'a>,
    /// Non-fatal startup conditions, shown in Settings through the runtime
    /// notices.
    pub notices: Vec<String>,
    pub db: Option<AppDb>,
    pub catalog: Option<ModelCatalog>,
    pub runtime: Option<runtime::RuntimeState>,
}

impl<'a> Ctx<'a> {
    fn new(app: &'a AppHandle, boot: &'a Boot) -> Self {
        Self {
            app,
            progress: Progress::of(boot),
            notices: Vec::new(),
            db: None,
            catalog: None,
            runtime: None,
        }
    }
}

/// The database a step needs, taken as `opened(ctx.db.as_ref())` so the
/// borrow covers that slot only and the step can still push notices.
pub(crate) fn opened(db: Option<&AppDb>) -> Result<&AppDb, String> {
    db.ok_or_else(|| "database not open".to_owned())
}

/// A step's error, kept apart from the step's name: the log gets both, the
/// boot screen only the message.
struct StepError {
    step: &'static str,
    message: String,
}

impl std::fmt::Display for StepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.step, self.message)
    }
}

/// Run `steps` in order, logging each one's duration (a no-op for
/// [`PRE_LOGGER`], which runs before the logger). Stops at the first
/// error, or before the next row once shutdown has set the cancel flag.
/// The phase of an error is the one last recorded.
fn run_steps(steps: &[Step], ctx: &mut Ctx<'_>) -> Result<(), StepError> {
    for step in steps {
        if ctx.progress.cancelled() {
            return Err(StepError {
                step: step.name,
                message: "startup cancelled".to_owned(),
            });
        }
        ctx.progress.phase(step.phase);
        ctx.progress.detail(step.label);
        let started = Instant::now();
        (step.run)(ctx).map_err(|message| StepError {
            step: step.name,
            message,
        })?;
        log::info!("startup: {} ({:?})", step.name, started.elapsed());
    }
    Ok(())
}

/// The boot plugin. Register it right after single-instance and before the
/// log plugin.
pub(crate) fn plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri::plugin::Builder::new("boot")
        .setup(|app, _api| {
            app.manage(Boot::default());
            let boot = app.state::<Boot>();
            let mut ctx = Ctx::new(app, &boot);
            run_steps(PRE_LOGGER, &mut ctx).map_err(|e| e.to_string())?;
            if let Ok(mut notices) = boot.pre_logger_notices.lock() {
                *notices = ctx.notices;
            }
            Ok(())
        })
        .build()
}

/// The `setup()` half of startup: manage the always-on state, then spawn
/// [`run`] and return, so the window paints while the steps run.
pub(crate) fn start(app: &AppHandle) -> Result<(), String> {
    let boot = app.state::<Boot>();
    // Models load lazily: the store starts empty and a
    // background thread fills it when the manifest files are already on
    // disk (always, for airgap; post-download for connected). Commands that
    // need models error cleanly until then.
    app.manage(inference::ModelStore::default());
    // Download in-flight guard + progress snapshots, managed before
    // autoload so models_status can always resolve it.
    app.manage(models::DownloadState::default());
    // Tracks per-run cancellation flags so `pause_run` can signal an
    // in-flight worker.
    app.manage(runs::RunRegistry::default());
    // macOS keeps native chrome: the base window config is frameless (for
    // the custom Windows/Linux titlebar), so re-enable decorations. See
    // decision #102.
    #[cfg(target_os = "macos")]
    if let Some(window) = app.get_webview_window("main") {
        window
            .set_decorations(true)
            .map_err(|e| format!("window decorations: {e}"))?;
    }

    boot.app
        .set(app.clone())
        .map_err(|_| "startup ran twice".to_owned())?;
    let app = app.clone();
    thread::Builder::new()
        .name("boot".to_owned())
        .spawn(move || run(&app))
        .map_err(|e| format!("spawn boot thread: {e}"))?;
    Ok(())
}

/// The `boot` thread: run [`STEPS`], then publish what they built and turn
/// `Ready`, or turn `Failed`. A run stopped by shutdown is neither: it drops
/// its [`Ctx`] (closing the database) and emits nothing.
fn run(app: &AppHandle) {
    let boot = app.state::<Boot>();
    let finished = Finished(&boot);
    let mut ctx = Ctx::new(app, &boot);
    if let Ok(mut notices) = boot.pre_logger_notices.lock() {
        ctx.notices = std::mem::take(&mut notices);
    }
    for notice in &ctx.notices {
        log::warn!("startup: {notice}");
    }
    let result = run_steps(STEPS, &mut ctx);
    if ctx.progress.cancelled() {
        drop(ctx);
        log::info!("startup: stopped for exit");
        return;
    }
    let phase = ctx
        .progress
        .recorded()
        .map_or_else(|| "no phase".to_owned(), |phase| format!("{phase:?}"));
    if let Err(e) = result {
        log::error!("startup failed in {phase}: {e}");
        boot.fail(e.message, ctx.notices);
        return;
    }
    if let Err(e) = publish(&boot, ctx) {
        log::error!("startup failed in {phase}: {e}");
        boot.fail(e, Vec::new());
        return;
    }
    boot.update(false, |state| {
        state.status = BootStatus::Ready;
        state.detail = None;
        state.done = 0;
        state.total = 0;
    });
    drop(finished);
    #[cfg(target_os = "macos")]
    crate::menu::enable_boot_items(app);
    // An exit requested as startup finished must not race a model load
    // (#231).
    if !boot.cancel.load(Ordering::Relaxed) {
        models::autoload_if_present(app);
    }
}

/// Hand the services the steps built to [`Boot`]. Notices collected by steps
/// join the runtime's own, the one startup-conditions surface Settings
/// renders.
fn publish(boot: &Boot, ctx: Ctx<'_>) -> Result<(), String> {
    let missing = |slot: &str| format!("startup finished without {slot}");
    let db = ctx.db.ok_or_else(|| missing("a database"))?;
    let catalog = ctx.catalog.ok_or_else(|| missing("a model catalog"))?;
    let mut runtime = ctx.runtime.ok_or_else(|| missing("a runtime"))?;
    runtime.notices.extend(ctx.notices);
    boot.services
        .set(Services {
            db,
            catalog,
            runtime,
        })
        .map_err(|_| "startup ran twice".to_owned())
}

/// How long [`shutdown`] waits for the step runner to stop.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);

/// The app's `RunEvent::Exit` body: stop startup, wait for it (bounded), then
/// checkpoint if a database is open. Tauri leaves `run` via `process::exit`,
/// so managed state is never dropped and `DuckDB` never gets its close-time
/// checkpoint; doing it here means a clean exit leaves no WAL for
/// the next launch to replay. Best effort: an in-flight run's flush holds
/// the writer briefly, and a refused checkpoint just leaves the WAL for the
/// next open.
pub(crate) fn shutdown(app: &AppHandle) {
    let boot = app.state::<Boot>();
    boot.cancel.store(true, Ordering::Relaxed);
    if let Ok(finished) = boot.finished.lock() {
        let waited = boot
            .finished_cv
            .wait_timeout_while(finished, SHUTDOWN_WAIT, |finished| !*finished);
        if waited.is_ok_and(|(_, timeout)| timeout.timed_out()) {
            log::warn!("exit: startup still running after {SHUTDOWN_WAIT:?}");
        }
    }
    if let Ok(services) = boot.ready() {
        match services.db.checkpoint() {
            Ok(()) => log::info!("exit: database checkpointed"),
            Err(e) => log::warn!("exit: checkpoint skipped: {e}"),
        }
    }
}

#[cfg(unix)]
fn install_signals(ctx: &mut Ctx<'_>) -> Result<(), String> {
    crate::signals::install(ctx.app)
}

/// How long [`lock_instance`] waits for another process to let go: longer
/// than [`SHUTDOWN_WAIT`] and the exit hang of #231.
const LOCK_WAIT: Duration = Duration::from_secs(15);
const LOCK_RETRY: Duration = Duration::from_millis(100);
const LOCK_BUSY: &str =
    "The app is already running or still closing. Wait a moment and start it again.";

/// The instance lock's file name, beside the database.
pub(crate) const INSTANCE_LOCK: &str = "session.lock";

/// Take `session.lock` beside the database and keep it on [`Boot`].
fn acquire_instance_lock(ctx: &mut Ctx<'_>) -> Result<(), String> {
    let db_path = db::db_path()?;
    let dir = db_path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", db_path.display()))?;
    let file = lock_instance(&dir.join(INSTANCE_LOCK), LOCK_WAIT)?;
    ctx.app
        .state::<Boot>()
        .instance_lock
        .set(file)
        .map_err(|_| "instance lock taken twice".to_owned())
}

/// Open `path` (created, with its directory, if missing) and take an
/// exclusive lock on it, retrying while another process holds it for up to
/// `wait`. The lock lasts as long as the returned `File` is open.
fn lock_instance(path: &Path, wait: Duration) -> Result<File, String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    // Read+write, not append: Windows refuses to lock append-only handles.
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    let deadline = Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                thread::sleep(LOCK_RETRY);
            }
            Err(TryLockError::WouldBlock) => return Err(LOCK_BUSY.to_owned()),
            Err(TryLockError::Error(e)) => return Err(format!("lock {}: {e}", path.display())),
        }
    }
}

/// Move 0.5.x data out of the Windows Roaming profile (#205). Never fails
/// startup: what can't be moved stays put, is retried next launch, and is
/// reported through the notices.
#[expect(
    clippy::unnecessary_wraps,
    reason = "every step row shares one signature"
)]
fn migrate_data(ctx: &mut Ctx<'_>) -> Result<(), String> {
    for outcome in crate::paths::migrate_legacy_data(&ctx.progress) {
        match outcome {
            Ok(msg) => log::info!("migrate: {msg}"),
            Err(msg) => {
                log::warn!("migrate: {msg}");
                ctx.notices
                    .push(format!("Moving app data to its new location: {msg}"));
            }
        }
    }
    Ok(())
}

/// Apply a pending reset (#206). Not fatal: a failure is a notice, and the
/// marker is consumed either way.
#[expect(
    clippy::unnecessary_wraps,
    reason = "every step row shares one signature"
)]
fn apply_reset(ctx: &mut Ctx<'_>) -> Result<(), String> {
    // Pre-logger: a notice is logged once the logger is up. Success shows
    // in the log as the trash sweep that follows.
    if let Err(e) = reset::apply_pending() {
        ctx.notices
            .push(format!("Resetting app data did not finish: {e}"));
    }
    Ok(())
}

/// Delete what a reset moved aside (#206). Not fatal: retried next launch.
#[expect(
    clippy::unnecessary_wraps,
    reason = "every step row shares one signature"
)]
fn sweep_reset_trash(ctx: &mut Ctx<'_>) -> Result<(), String> {
    if let Err(e) = reset::sweep_trash() {
        log::warn!("reset: {e}");
        ctx.notices
            .push(format!("Deleting reset app data did not finish: {e}"));
    }
    Ok(())
}

/// Open `DuckDB` and apply migrations. A WAL set aside at open is
/// reported through the notices.
fn open_database(ctx: &mut Ctx<'_>) -> Result<(), String> {
    if !db::db_path()?.exists() {
        ctx.progress.detail("Creating a new database");
    }
    let version = ctx.app.package_info().version.to_string();
    let db = AppDb::open(&version, &ctx.progress)?;
    ctx.notices.extend(db.recovery_notice().map(str::to_owned));
    ctx.db = Some(db);
    Ok(())
}

/// Crash recovery: a `running` row in a fresh process is an orphan
/// from a previous one. Flip it to `interrupted` (resumable) before any
/// command can observe it.
fn sweep_runs(ctx: &mut Ctx<'_>) -> Result<(), String> {
    let swept = runs::sweep_orphaned_runs(&*opened(ctx.db.as_ref())?.rw()?)?;
    if swept > 0 {
        log::info!("startup: swept {swept} orphaned running run(s) to interrupted");
    }
    Ok(())
}

/// Same for imports, which don't resume: an `importing` dataset becomes
/// `failed` with a plain-language message.
fn sweep_imports(ctx: &mut Ctx<'_>) -> Result<(), String> {
    let swept = import::sweep_orphaned_imports(&*opened(ctx.db.as_ref())?.rw()?)?;
    if swept > 0 {
        log::info!("startup: marked {swept} interrupted import(s) as failed");
    }
    Ok(())
}

/// Resolve the embedded model manifest against the models table so every
/// digit-level → model-id lookup goes through pinned rows (stale rows from
/// earlier families stay put for their cached results but are never
/// selected).
fn manifest_rows(ctx: &mut Ctx<'_>) -> Result<(), String> {
    let catalog =
        manifest::resolve_model_rows(&*opened(ctx.db.as_ref())?.rw()?, manifest::load()?)?;
    ctx.catalog = Some(catalog);
    Ok(())
}

/// Fold the startup writes (migrations, sweep, manifest rows) into the main
/// file: a crash later in this session then orphans only what was
/// written after this point, and the next open has that much less WAL to
/// replay. Not fatal.
fn checkpoint(ctx: &mut Ctx<'_>) -> Result<(), String> {
    if let Err(e) = opened(ctx.db.as_ref())?.checkpoint() {
        log::warn!("startup: checkpoint skipped: {e}");
    }
    Ok(())
}

/// Load ONNX Runtime: with `load-dynamic` nothing is linked, so the
/// dylib must be loaded before any session exists. Pack choice follows the
/// settings EP priority (GPU pack when installed, bundled CPU pack
/// otherwise) and is fixed for the process lifetime; switching packs
/// requires a relaunch.
fn load_runtime(ctx: &mut Ctx<'_>) -> Result<(), String> {
    let settings = config::read_settings().map_err(|e| format!("read settings: {e}"))?;
    let resource_dir = ctx
        .app
        .path()
        .resource_dir()
        .map_err(|e| format!("resolve bundle resource dir: {e}"))?;
    let state = runtime::startup(&settings, &resource_dir)?;
    log::info!(
        "startup: ONNX Runtime {} loaded from pack '{}'",
        state.ort_version,
        state.pack_id
    );
    ctx.runtime = Some(state);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{LOCK_BUSY, lock_instance};

    /// A second handle can't take the lock while the first holds it, gives
    /// up with the busy message after its bound, and gets it once the
    /// first is closed.
    #[test]
    fn lock_instance_excludes_a_second_handle() -> Result<(), String> {
        let dir = std::env::temp_dir().join(format!("ccm-lock-test-{}", std::process::id()));
        let path = dir.join("session.lock");
        let held = lock_instance(&path, Duration::ZERO)?;

        let wait = Duration::from_millis(300);
        let started = Instant::now();
        let busy = lock_instance(&path, wait).err();
        assert_eq!(busy.as_deref(), Some(LOCK_BUSY));
        assert!(started.elapsed() >= wait);

        drop(held);
        let retaken = lock_instance(&path, Duration::ZERO);
        assert!(retaken.is_ok());
        drop(retaken);
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())
    }
}
