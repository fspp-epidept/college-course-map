//! Clean shutdown on Unix termination signals (#227).
//!
//! A signal whose default action ends the process skips `RunEvent::Exit`,
//! and with it the database exit checkpoint and every plugin's exit hook. Each
//! signal in [`SHUTDOWN`] is turned into `AppHandle::exit` instead, the same
//! path closing the window takes.
//!
//! Deliberately not handled:
//! - `SIGKILL` and `SIGSTOP` can't be caught. Recovery on the next open (WAL
//!   replay, orphaned-run sweep) covers a kill.
//! - Fault signals (`SIGSEGV`, `SIGBUS`, `SIGFPE`, `SIGILL`, `SIGABRT`,
//!   `SIGTRAP`, `SIGSYS`) mean process state can't be trusted to write the
//!   database; the same recovery covers them.
//! - `SIGPIPE` is already ignored by the Rust runtime.
//! - `SIGXCPU` is taken over by the `WebKit` JavaScript engine
//!   (`JavaScriptCore`) after startup, replacing any handler installed here
//!   (verified on Linux); next-open recovery covers it like a kill.
//! - `SIGUSR1`/`SIGUSR2` and the timer signals (`SIGALRM`, `SIGVTALRM`,
//!   `SIGPROF`) belong to whichever linked library uses them.

use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGQUIT, SIGTERM, SIGXFSZ};
use signal_hook::iterator::Signals;
use signal_hook::low_level::signal_name;

/// Signals that ask the process to end: terminal hangup and interrupt, quit,
/// `kill`/logout/shutdown, and the file-size limit.
const SHUTDOWN: &[i32] = &[SIGHUP, SIGINT, SIGQUIT, SIGTERM, SIGXFSZ];

/// Install the handlers and the thread that turns a delivered signal into an
/// event-loop exit. Tauri's `App::run` exits 0 whatever code is requested.
pub(crate) fn install(app: &tauri::AppHandle) -> Result<(), String> {
    let mut signals = Signals::new(SHUTDOWN).map_err(|e| e.to_string())?;
    let app = app.clone();
    std::thread::Builder::new()
        .name("signals".to_owned())
        .spawn(move || {
            for signal in signals.forever() {
                log::info!(
                    "exit: {} received, shutting down",
                    signal_name(signal).unwrap_or("signal")
                );
                app.exit(0);
            }
        })
        .map_err(|e| format!("spawn signal thread: {e}"))?;
    Ok(())
}
