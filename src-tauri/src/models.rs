//! Connected-build model lifecycle (EPI-56 + the EPI-3 async-load slice):
//! status reporting, first-run download from the pinned HF revisions, and
//! lazy loading into the [`ModelStore`].
//!
//! Downloads run in Rust (blocking reqwest inside `spawn_blocking`) — the
//! `WebView` never talks to the network, so the CSP stays untouched. Every
//! file streams through an incremental sha256 and is deleted on mismatch;
//! files already present with a verified hash are skipped, which makes
//! retry-after-failure a plain re-invoke.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager as _, State};
use tauri_specta::Event;

use crate::{
    inference::{self, ModelStore},
    manifest::{ModelCatalog, files_present},
};

// Download-path-only imports; the airgap flavor compiles the downloader out.
#[cfg(not(feature = "airgap"))]
use crate::manifest::ManifestModel;
#[cfg(not(feature = "airgap"))]
use sha2::{Digest as _, Sha256};
#[cfg(not(feature = "airgap"))]
use std::io::{Read as _, Write as _};

/// Per-file download progress. `received`/`total` are bytes; `bytes_per_sec`
/// is measured over the emission window (EPI-65). The frontend derives
/// percentages.
#[derive(Type, Serialize, Deserialize, Debug, Clone, Event)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelDownloadProgress {
    pub digit_level: u8,
    pub file: String,
    pub received: f64,
    pub total: f64,
    pub bytes_per_sec: f64,
}

/// Coarse "something about model state changed" signal — emitted after a file
/// finishes downloading, a load starts/completes, or either fails. The
/// frontend responds by refetching `models_status`.
#[derive(Type, Serialize, Deserialize, Debug, Clone, Event)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelsStateChanged {}

/// Last-known download position for one digit level, kept server-side so a
/// freshly-mounted client renders the true state from `models_status` without
/// having caught any progress events (EPI-74).
#[derive(Type, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DownloadSnapshot {
    pub file: String,
    pub received: f64,
    pub total: f64,
}

/// In-flight download state, managed in Tauri state (EPI-74). The backend is
/// the source of truth: `download_models` refuses to start while `active`,
/// `models_status` reports `downloading` + per-level positions, and
/// `cancel_download` flips the flag the chunk loop polls. Mirrors the
/// [`ModelStore`] loading-flag pattern.
#[derive(Default)]
pub(crate) struct DownloadState {
    active: std::sync::atomic::AtomicBool,
    cancel: std::sync::atomic::AtomicBool,
    progress: std::sync::Mutex<std::collections::HashMap<u8, DownloadSnapshot>>,
}

impl DownloadState {
    /// Claim the in-flight flag. Returns false when a download is already
    /// running — callers must not start a second one. Resets the cancel flag
    /// and progress snapshots for the new attempt.
    #[cfg(not(feature = "airgap"))]
    fn try_begin(&self) -> bool {
        if self.active.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return false;
        }
        self.cancel
            .store(false, std::sync::atomic::Ordering::SeqCst);
        if let Ok(mut progress) = self.progress.lock() {
            progress.clear();
        }
        true
    }

    #[cfg(not(feature = "airgap"))]
    fn end(&self) {
        self.active
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Request cancellation of the in-flight download, if any. Returns whether
    /// a download was active to receive the request.
    fn request_cancel(&self) -> bool {
        let active = self.is_active();
        if active {
            self.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        active
    }

    #[cfg(not(feature = "airgap"))]
    fn cancelled(&self) -> bool {
        self.cancel.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[cfg(not(feature = "airgap"))]
    fn record(&self, digit_level: u8, snapshot: DownloadSnapshot) {
        if let Ok(mut progress) = self.progress.lock() {
            progress.insert(digit_level, snapshot);
        }
    }

    fn snapshot(&self, digit_level: u8) -> Option<DownloadSnapshot> {
        self.progress
            .lock()
            .ok()
            .and_then(|progress| progress.get(&digit_level).cloned())
    }
}

#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelStatus {
    pub digit_level: u8,
    pub display_name: String,
    pub hf_repo: String,
    pub revision: String,
    pub files_total: u32,
    /// Files on disk with the manifest's exact size. Full sha256 verification
    /// happens during download, not on status polls.
    pub files_present: u32,
    pub total_bytes: f64,
    pub loaded: bool,
    pub loading: bool,
    /// Whether a download is in flight app-wide (same value on every row).
    /// This — not any component-local pending flag — gates the Download
    /// button (EPI-74).
    pub downloading: bool,
    /// This level's last-known download position, when a download has
    /// touched it. Hydrates a freshly-mounted progress bar.
    pub download: Option<DownloadSnapshot>,
}

/// Resolve where the active flavor keeps its models: the bundle's resource
/// dir for airgap (read-only, guaranteed populated), the data-dir models path
/// otherwise (populated by `download_models` or `task models:install`).
pub(crate) fn active_models_root(app: &AppHandle) -> Result<PathBuf, String> {
    #[cfg(feature = "airgap")]
    {
        app.path()
            .resource_dir()
            .map(|dir| dir.join("models"))
            .map_err(|e| format!("resolve bundle resource dir: {e}"))
    }
    #[cfg(not(feature = "airgap"))]
    {
        let _ = app; // AppHandle only needed by the airgap arm
        inference::models_root()
    }
}

#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn models_status(
    app: AppHandle,
    catalog: State<'_, ModelCatalog>,
    store: State<'_, ModelStore>,
    downloads: State<'_, DownloadState>,
) -> Result<Vec<ModelStatus>, String> {
    let root = active_models_root(&app)?;
    let loaded = store.is_loaded();
    let loading = store.is_loading();
    let downloading = downloads.is_active();
    Ok(catalog
        .manifest
        .model
        .iter()
        .map(|entry| ModelStatus {
            digit_level: entry.digit_level,
            display_name: entry.display_name.clone(),
            hf_repo: entry.hf_repo.clone(),
            revision: entry.revision.clone(),
            files_total: u32::try_from(entry.files.len()).unwrap_or(u32::MAX),
            files_present: u32::try_from(files_present(&root, entry)).unwrap_or(u32::MAX),
            #[expect(
                clippy::cast_precision_loss,
                reason = "file sizes are far below f64's 2^53 exact-integer range"
            )]
            total_bytes: entry.files.iter().map(|f| f.size as f64).sum(),
            loaded,
            loading,
            downloading,
            download: downloads.snapshot(entry.digit_level),
        })
        .collect())
}

/// Request cancellation of the in-flight model download. The chunk loop
/// honors it within one read; the partial file is deleted. Returns `false`
/// when no download was active.
#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri injects State by value; cannot be taken by reference at the macro layer"
)]
pub(crate) fn cancel_download(downloads: State<'_, DownloadState>) -> bool {
    downloads.request_cancel()
}

/// Load models from disk into the store. No-op when already loaded; errors if
/// a load is in flight or files are missing. Slow (~5-15 s), so it runs on a
/// blocking thread and the UI shows `loading` from `models_status`.
#[tauri::command]
#[specta::specta]
pub(crate) async fn load_models(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || load_now(&app))
        .await
        .map_err(|e| format!("load task panicked: {e}"))?
}

/// Clear the store and load fresh — the settings path for changes that only
/// take effect at session build time (EPI-73: EP priority reorder). Unlike
/// `load_models`, already-loaded is not a no-op. A run in flight finishes on
/// its `Arc` of the old registry; new runs see the new sessions.
#[tauri::command]
#[specta::specta]
pub(crate) async fn reload_models(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let store = app.state::<ModelStore>();
        store.clear()?;
        let _ = ModelsStateChanged {}.emit(&app);
        load_now(&app)
    })
    .await
    .map_err(|e| format!("reload task panicked: {e}"))?
}

/// Synchronous body shared by the `load_models` command and startup autoload.
pub(crate) fn load_now(app: &AppHandle) -> Result<(), String> {
    let store = app.state::<ModelStore>();
    if store.is_loaded() {
        return Ok(());
    }
    if !store.try_begin_loading() {
        return Err("a model load is already in progress".to_owned());
    }
    let _ = ModelsStateChanged {}.emit(app);
    let result = (|| {
        let root = active_models_root(app)?;
        // The EP priority list and CPU thread cap are read at load time, so a
        // settings change takes effect by re-triggering a model load — no
        // restart. (Switching runtime *packs* is the part that needs a
        // relaunch; see runtime.rs.)
        let settings = crate::config::read_settings()?;
        // Only providers the loaded pack carries are ever attempted
        // (EPI-104): a settings entry the pack lacks is dropped here, not
        // discovered by a failed registration.
        let runtime = app.state::<crate::runtime::RuntimeState>();
        let eps = runtime.registrable(&settings.execution_providers);
        for skipped in settings
            .execution_providers
            .iter()
            .filter(|ep| !eps.contains(ep))
        {
            eprintln!(
                "execution provider {}: not in runtime pack '{}', skipped",
                skipped.as_str(),
                runtime.pack_id
            );
        }
        let registry = inference::load_all_models(&root, &eps, settings.max_cpu_threads)
            .map_err(|e| e.to_string())?;
        store.set(registry)
    })();
    store.end_loading();
    let _ = ModelsStateChanged {}.emit(app);
    // A load failure with "on disk" files is the dead-end EPI-75 hit: a
    // size-exact-but-torn file that `files_present` can't see. Re-verify
    // against the manifest and delete mismatches so the Download button
    // becomes the visible repair path.
    #[cfg(not(feature = "airgap"))]
    let result = result.map_err(|e| match repair_corrupt_files(app) {
        Ok(0) => e,
        Ok(removed) => {
            let _ = ModelsStateChanged {}.emit(app);
            format!(
                "{e} — {removed} model file(s) failed manifest verification and \
                 were removed; use Download models to re-fetch them"
            )
        }
        Err(repair_err) => {
            eprintln!("post-load-failure repair: {repair_err}");
            e
        }
    });
    result
}

/// Startup hook: load in the background when every manifest file is already
/// on disk (always true for airgap; true post-first-run for connected). Keeps
/// the window responsive during the ~15 s load instead of blocking setup.
pub(crate) fn autoload_if_present(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let all_present = (|| -> Result<bool, String> {
            let root = active_models_root(&app)?;
            let catalog = app.state::<ModelCatalog>();
            Ok(catalog
                .manifest
                .model
                .iter()
                .all(|entry| files_present(&root, entry) == entry.files.len()))
        })();
        match all_present {
            Ok(true) => {
                if let Err(e) = load_now(&app) {
                    eprintln!("model autoload failed: {e}");
                }
            }
            Ok(false) => {} // connected first run: Models panel offers download
            Err(e) => eprintln!("model autoload skipped: {e}"),
        }
    });
}

/// Download every missing manifest file from its pinned HF revision into the
/// data-dir models path, sha256-verifying as it streams. Connected builds
/// only — the airgap bundle ships its models read-only in the resource dir.
#[tauri::command]
#[specta::specta]
pub(crate) async fn download_models(app: AppHandle) -> Result<(), String> {
    #[cfg(feature = "airgap")]
    {
        let _ = app;
        Err("airgap build: models are bundled with the installer".to_owned())
    }
    #[cfg(not(feature = "airgap"))]
    {
        // One download at a time, app-wide (EPI-74): two concurrent
        // download_all racers would interleave writes and progress events.
        if !app.state::<DownloadState>().try_begin() {
            return Err("a model download is already in progress".to_owned());
        }
        let result = tauri::async_runtime::spawn_blocking({
            let app = app.clone();
            move || download_all(&app)
        })
        .await
        .map_err(|e| format!("download task panicked: {e}"));
        app.state::<DownloadState>().end();
        let _ = ModelsStateChanged {}.emit(&app);
        result?
    }
}

#[cfg(not(feature = "airgap"))]
fn download_all(app: &AppHandle) -> Result<(), String> {
    let root = inference::models_root()?;
    let catalog = app.state::<ModelCatalog>();
    let client = reqwest::blocking::Client::builder()
        .user_agent("college-course-map")
        .timeout(None) // model files are ~600 MB; no global deadline
        .build()
        .map_err(|e| format!("build http client: {e}"))?;

    let downloads = app.state::<DownloadState>();
    for entry in &catalog.manifest.model {
        let dir = root.join(&entry.app_subdir);
        std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        // Leftover temp files from crashed or cancelled attempts are dead
        // weight (each attempt uses a fresh name, EPI-75) — sweep them.
        sweep_stale_parts(&dir);
        for file in &entry.files {
            if downloads.cancelled() {
                return Err("download cancelled".to_owned());
            }
            let dest = dir.join(&file.name);
            if verified_on_disk(&dest, file.size, &file.sha256) {
                continue;
            }
            download_one(app, &client, entry, file, &dest)?;
            let _ = ModelsStateChanged {}.emit(app);
        }
    }
    Ok(())
}

/// Remove leftover `*.part-*` temp files (and pre-EPI-75 `*.part` ones) from
/// a model directory. Best-effort: a temp that can't be removed only wastes
/// disk, it can never be promoted to a model file.
#[cfg(not(feature = "airgap"))]
fn sweep_stale_parts(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let is_part = std::path::Path::new(&name)
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext == "part" || ext.starts_with("part-"));
        if is_part {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// A file counts as verified when both size and sha256 match the manifest.
/// Only consulted for skip decisions before downloading — cheap size check
/// first, hash only when the size matches.
#[cfg(not(feature = "airgap"))]
fn verified_on_disk(path: &std::path::Path, size: u64, sha256: &str) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if meta.len() != size {
        return false;
    }
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hasher.update(buf.get(..n).unwrap_or_default()),
            Err(_) => return false,
        }
    }
    format!("{:x}", hasher.finalize()) == sha256
}

#[cfg(not(feature = "airgap"))]
fn download_one(
    app: &AppHandle,
    client: &reqwest::blocking::Client,
    entry: &ManifestModel,
    file: &crate::manifest::ManifestFile,
    dest: &std::path::Path,
) -> Result<(), String> {
    // `resolve` serves the file content at the pinned commit; HF redirects
    // large files to its CDN, which reqwest follows.
    let url = format!(
        "https://huggingface.co/{}/resolve/{}/{}",
        entry.hf_repo, entry.revision, file.name
    );
    let mut response = client
        .get(&url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|e| format!("GET {url}: {e}"))?;

    // Stream to a per-attempt temp file; only rename into place after the
    // hash checks out, so a torn download can never be mistaken for a model.
    // The unique name + create_new mean no two writers can ever share a path
    // (EPI-75: a fixed `.part` path let concurrent racers truncate and
    // interleave each other's file, promoting torn bytes into place).
    let part = dest.with_extension(format!("part-{}", uuid::Uuid::new_v4()));
    let mut out = std::io::BufWriter::new(
        std::fs::File::create_new(&part).map_err(|e| format!("create {}: {e}", part.display()))?,
    );
    let mut hasher = Sha256::new();
    let mut received: u64 = 0;
    let mut buf = vec![0u8; 1 << 20];
    // Progress is rate-limited (EPI-65): `read()` returns network-sized
    // chunks (8-16 KB), and emitting per chunk pushed tens of thousands of
    // IPC events per file through the WebView — the event storm, not the
    // network, was the throughput ceiling. One emit per interval also gives
    // a stable window for the speed measurement.
    const EMIT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);
    let started = std::time::Instant::now();
    let mut last_emit = started;
    let mut last_emit_bytes: u64 = 0;
    let downloads = app.state::<DownloadState>();
    #[expect(
        clippy::cast_precision_loss,
        reason = "file sizes are far below f64's 2^53 exact-integer range"
    )]
    let emit_progress = |received: u64, bytes_per_sec: f64| {
        // The event drives mounted listeners; the snapshot is what
        // `models_status` serves to freshly-mounted clients (EPI-74).
        downloads.record(
            entry.digit_level,
            DownloadSnapshot {
                file: file.name.clone(),
                received: received as f64,
                total: file.size as f64,
            },
        );
        let _ = ModelDownloadProgress {
            digit_level: entry.digit_level,
            file: file.name.clone(),
            received: received as f64,
            total: file.size as f64,
            bytes_per_sec,
        }
        .emit(app);
    };
    loop {
        if downloads.cancelled() {
            drop(out);
            let _ = std::fs::remove_file(&part);
            return Err("download cancelled".to_owned());
        }
        let n = response
            .read(&mut buf)
            .map_err(|e| format!("read {url}: {e}"))?;
        if n == 0 {
            break;
        }
        let chunk = buf.get(..n).unwrap_or_default();
        hasher.update(chunk);
        out.write_all(chunk)
            .map_err(|e| format!("write {}: {e}", part.display()))?;
        received += n as u64;
        let window = last_emit.elapsed();
        if window >= EMIT_INTERVAL {
            #[expect(
                clippy::cast_precision_loss,
                reason = "byte deltas are far below f64's 2^53 exact-integer range"
            )]
            let bytes_per_sec = (received - last_emit_bytes) as f64 / window.as_secs_f64();
            emit_progress(received, bytes_per_sec);
            last_emit = std::time::Instant::now();
            last_emit_bytes = received;
        }
    }
    // Final emit so the frontend always sees 100%; speed is the whole-file
    // average, which is also the number to compare against a curl baseline.
    #[expect(
        clippy::cast_precision_loss,
        reason = "byte counts are far below f64's 2^53 exact-integer range"
    )]
    let avg = received as f64 / started.elapsed().as_secs_f64().max(f64::EPSILON);
    emit_progress(received, avg);
    out.flush()
        .map_err(|e| format!("flush {}: {e}", part.display()))?;
    drop(out);

    let digest = format!("{:x}", hasher.finalize());
    if received != file.size || digest != file.sha256 {
        let _ = std::fs::remove_file(&part);
        return Err(format!(
            "{}/{} failed verification (got {received} bytes, sha256 {digest}; \
             manifest says {} bytes, {}) — file deleted, retry the download",
            entry.hf_repo, file.name, file.size, file.sha256
        ));
    }
    // The stream hash covers what the network delivered; re-verify what
    // actually landed on disk before promoting it (EPI-75: interference from
    // another writer once passed the stream check while the file was torn).
    if !verified_on_disk(&part, file.size, &file.sha256) {
        let _ = std::fs::remove_file(&part);
        return Err(format!(
            "{}/{}: on-disk bytes do not match the manifest after download — \
             file deleted, retry the download",
            entry.hf_repo, file.name
        ));
    }
    // Windows refuses rename onto an existing path; a stale/corrupt dest is
    // exactly what a repair re-download replaces, so clear it first.
    if dest.exists() {
        let _ = std::fs::remove_file(dest);
    }
    std::fs::rename(&part, dest).map_err(|e| format!("rename into place: {e}"))?;
    Ok(())
}

/// Post-load-failure repair (EPI-75): fully re-verify every manifest file and
/// delete the ones whose on-disk bytes don't match. `files_present` (size-only
/// by design) can report a torn file as "on disk", leaving the UI offering a
/// Load button that can never succeed; deleting the corrupt file makes the
/// existing Download button the visible repair path. Returns how many files
/// were removed.
#[cfg(not(feature = "airgap"))]
fn repair_corrupt_files(app: &AppHandle) -> Result<usize, String> {
    let root = inference::models_root()?;
    let catalog = app.state::<ModelCatalog>();
    let mut removed = 0;
    for entry in &catalog.manifest.model {
        for file in &entry.files {
            let path = root.join(&entry.app_subdir).join(&file.name);
            if path.exists() && !verified_on_disk(&path, file.size, &file.sha256) {
                std::fs::remove_file(&path)
                    .map_err(|e| format!("remove corrupt {}: {e}", path.display()))?;
                removed += 1;
            }
        }
    }
    Ok(removed)
}

#[cfg(all(test, not(feature = "airgap")))]
mod tests {
    use sha2::{Digest as _, Sha256};

    /// EPI-74: the guard admits exactly one download, cancel only reaches an
    /// active download, and a fresh attempt starts with a clean cancel flag
    /// and empty snapshots.
    #[test]
    fn download_state_guard_and_cancel_lifecycle() {
        let state = super::DownloadState::default();
        assert!(!state.is_active());
        assert!(!state.request_cancel(), "idle: nothing to cancel");

        assert!(state.try_begin());
        assert!(state.is_active());
        assert!(!state.try_begin(), "second claim must be refused");

        state.record(
            6,
            super::DownloadSnapshot {
                file: "model.onnx".to_owned(),
                received: 10.0,
                total: 100.0,
            },
        );
        assert!(state.snapshot(6).is_some());
        assert!(state.snapshot(2).is_none());

        assert!(state.request_cancel());
        assert!(state.cancelled());
        state.end();
        assert!(!state.is_active());

        // A new attempt resets the cancel flag and stale snapshots.
        assert!(state.try_begin());
        assert!(!state.cancelled());
        assert!(state.snapshot(6).is_none());
        state.end();
    }

    /// EPI-75: the sweep removes per-attempt temps (and legacy `.part`
    /// files) while leaving real model files alone.
    #[test]
    fn sweep_removes_only_part_files() -> Result<(), String> {
        let dir = std::env::temp_dir().join(format!("sweep-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        for name in [
            "model.onnx",
            "tokenizer.json",
            "model.part",
            "model.part-3f2a",
            "tokenizer.part-9c1d",
        ] {
            std::fs::write(dir.join(name), b"x").map_err(|e| e.to_string())?;
        }
        super::sweep_stale_parts(&dir);
        let mut remaining: Vec<String> = std::fs::read_dir(&dir)
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        remaining.sort();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(remaining, ["model.onnx", "tokenizer.json"]);
        Ok(())
    }

    /// Network test (ignored by default): fetch the smallest manifest file
    /// from its pinned revision via the exact URL scheme `download_one` uses
    /// and confirm the manifest hash matches. Proves the pinned revisions are
    /// reachable and the resolve-URL format is right without a 1.8 GB pull.
    /// Run explicitly: `cargo test --lib download_url -- --ignored`
    #[test]
    #[ignore = "hits huggingface.co"]
    fn download_url_scheme_and_hash_agree_with_manifest() -> Result<(), String> {
        let manifest = crate::manifest::load()?;
        let entry = manifest
            .model
            .iter()
            .find(|m| m.digit_level == 2)
            .ok_or("no 2-digit model")?;
        let file = entry
            .files
            .iter()
            .min_by_key(|f| f.size)
            .ok_or("no files")?;
        let url = format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            entry.hf_repo, entry.revision, file.name
        );
        let body = reqwest::blocking::get(&url)
            .and_then(reqwest::blocking::Response::error_for_status)
            .and_then(reqwest::blocking::Response::bytes)
            .map_err(|e| format!("GET {url}: {e}"))?;
        assert_eq!(body.len() as u64, file.size);
        let digest = format!("{:x}", Sha256::digest(&body));
        assert_eq!(digest, file.sha256);
        Ok(())
    }
}
