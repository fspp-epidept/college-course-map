//! Startup order and the one shutdown path (#229).
//!
//! Startup reads top to bottom here:
//!
//! 1. `tauri-plugin-single-instance` (registered first in `lib.rs`): a second
//!    launch focuses the running instance and exits, before anything below
//!    touches shared state.
//! 2. [`plugin`]: manages [`Boot`], then runs [`PRE_LOGGER`], the steps that
//!    must precede the log plugin. They run on the main thread with no window
//!    and nothing logged, so they stay fast.
//! 3. The log plugin, then opener and dialog.
//! 4. `setup()` → [`start`]: the always-managed state, macOS decorations, then
//!    [`STEPS`] in order. When every step has run, the services they built
//!    are published to [`Boot`] and model autoload starts.
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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager as _};

use crate::{
    config, db::AppDb, inference, manifest, manifest::ModelCatalog, models, runs, runtime,
};

/// The startup phases, in order. The names are the ones the boot screen
/// (#224) reports.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Phase {
    MigratingData,
    OpeningDatabase,
    UpgradingSchema,
    LoadingRuntime,
}

/// One startup step: a row in [`PRE_LOGGER`] or [`STEPS`].
struct Step {
    phase: Phase,
    name: &'static str,
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
        run: install_signals,
    },
];

/// Steps that run from `setup()`, in order.
const STEPS: &[Step] = &[
    Step {
        phase: Phase::OpeningDatabase,
        name: "open database",
        run: open_database,
    },
    Step {
        phase: Phase::UpgradingSchema,
        name: "sweep runs",
        run: sweep_runs,
    },
    Step {
        phase: Phase::UpgradingSchema,
        name: "manifest rows",
        run: manifest_rows,
    },
    Step {
        phase: Phase::UpgradingSchema,
        name: "checkpoint",
        run: checkpoint,
    },
    Step {
        phase: Phase::LoadingRuntime,
        name: "load runtime",
        run: load_runtime,
    },
];

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
    /// Set by [`shutdown`]; the runner stops at the next row.
    cancel: AtomicBool,
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

    fn mark_finished(&self) {
        if let Ok(mut finished) = self.finished.lock() {
            *finished = true;
        }
        self.finished_cv.notify_all();
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
pub(crate) struct Progress<'a> {
    cancel: Option<&'a AtomicBool>,
}

impl<'a> Progress<'a> {
    /// Never cancelled, reports nowhere.
    #[expect(
        dead_code,
        reason = "first callers are the harnesses of `AppDb::open_at` (#204)"
    )]
    pub(crate) fn none() -> Self {
        Self { cancel: None }
    }

    fn of(boot: &'a Boot) -> Self {
        Self {
            cancel: Some(&boot.cancel),
        }
    }

    /// Report progress within the current step. Nothing listens yet; the
    /// boot screen (#224) will.
    #[expect(dead_code, reason = "first caller is the data-migration step (#205)")]
    #[expect(clippy::unused_self, reason = "#224 reports through the boot state")]
    pub(crate) fn report(&self, done: u64, total: u64) {
        log::debug!("startup: {done}/{total}");
    }

    /// Whether shutdown has asked startup to stop. Long steps check it
    /// between files or chunks.
    pub(crate) fn cancelled(&self) -> bool {
        self.cancel
            .is_some_and(|cancel| cancel.load(Ordering::Relaxed))
    }
}

/// What a step gets: the app, progress and cancel, notices for Settings, and
/// the slots steps fill.
pub(crate) struct Ctx<'a> {
    pub app: &'a AppHandle,
    boot: &'a Boot,
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
            boot,
            progress: Progress::of(boot),
            notices: Vec::new(),
            db: None,
            catalog: None,
            runtime: None,
        }
    }

    fn db(&self) -> Result<&AppDb, String> {
        self.db
            .as_ref()
            .ok_or_else(|| "database not open".to_owned())
    }
}

/// Run `steps` in order, logging each one's duration. Stops at the first
/// error, or before the next row once shutdown has set the cancel flag.
fn run_steps(steps: &[Step], ctx: &mut Ctx<'_>) -> Result<(), String> {
    for step in steps {
        if ctx.progress.cancelled() {
            return Err(format!("{:?}: startup cancelled", step.phase));
        }
        let started = Instant::now();
        (step.run)(ctx).map_err(|e| format!("{:?}: {}: {e}", step.phase, step.name))?;
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
            run_steps(PRE_LOGGER, &mut Ctx::new(app, &boot))?;
            Ok(())
        })
        .build()
}

/// The `setup()` half of startup: run [`STEPS`] and publish what they built.
pub(crate) fn start(app: &AppHandle) -> Result<(), String> {
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

    let boot = app.state::<Boot>();
    let mut ctx = Ctx::new(app, &boot);
    let result = run_steps(STEPS, &mut ctx).and_then(|()| publish(ctx));
    boot.mark_finished();
    result?;
    models::autoload_if_present(app);
    Ok(())
}

/// Hand the services the steps built to [`Boot`]. Notices collected by steps
/// join the runtime's own, the one startup-conditions surface Settings
/// renders.
fn publish(ctx: Ctx<'_>) -> Result<(), String> {
    let missing = |slot: &str| format!("startup finished without {slot}");
    let db = ctx.db.ok_or_else(|| missing("a database"))?;
    let catalog = ctx.catalog.ok_or_else(|| missing("a model catalog"))?;
    let mut runtime = ctx.runtime.ok_or_else(|| missing("a runtime"))?;
    runtime.notices.extend(ctx.notices);
    ctx.boot
        .services
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
    let Some(boot) = app.try_state::<Boot>() else {
        return;
    };
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

/// Open `DuckDB` and apply migrations. A WAL set aside at open is
/// reported through the notices.
fn open_database(ctx: &mut Ctx<'_>) -> Result<(), String> {
    let db = AppDb::open()?;
    ctx.notices.extend(db.recovery_notice().map(str::to_owned));
    ctx.db = Some(db);
    Ok(())
}

/// Crash recovery: a `running` row in a fresh process is an orphan
/// from a previous one. Flip it to `interrupted` (resumable) before any
/// command can observe it.
fn sweep_runs(ctx: &mut Ctx<'_>) -> Result<(), String> {
    let swept = runs::sweep_orphaned_runs(&*ctx.db()?.rw()?)?;
    if swept > 0 {
        log::info!("startup: swept {swept} orphaned running run(s) to interrupted");
    }
    Ok(())
}

/// Resolve the embedded model manifest against the models table so every
/// digit-level → model-id lookup goes through pinned rows (stale rows from
/// earlier families stay put for their cached results but are never
/// selected).
fn manifest_rows(ctx: &mut Ctx<'_>) -> Result<(), String> {
    let catalog = manifest::resolve_model_rows(&*ctx.db()?.rw()?, manifest::load()?)?;
    ctx.catalog = Some(catalog);
    Ok(())
}

/// Fold the startup writes (migrations, sweep, manifest rows) into the main
/// file: a crash later in this session then orphans only what was
/// written after this point, and the next open has that much less WAL to
/// replay. Not fatal.
fn checkpoint(ctx: &mut Ctx<'_>) -> Result<(), String> {
    if let Err(e) = ctx.db()?.checkpoint() {
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
