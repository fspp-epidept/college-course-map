//! ONNX Runtime pack management (EPI-73). With `load-dynamic`, nothing links
//! ONNX Runtime at build time — a *runtime pack* (a repackaged official
//! microsoft/onnxruntime release archive, pinned in `runtimes.toml`) provides
//! the dylib, and [`init_ort`] loads it exactly once at startup via
//! `ort::init_from`. Which execution providers exist is a property of the
//! loaded pack; per-session EP registration happens in `inference.rs`.
//!
//! On-disk layout:
//! - downloaded packs: `<data>/college-course-map/runtimes/<ort_version>/<id>/`
//! - bundled CPU pack: `<resource_dir>/runtimes/cpu/` (version implicit — a
//!   bundle carries exactly one)
//!
//! A pack directory is valid when its dylib exists and its `.sha256` marker
//! matches the manifest archive hash — extraction goes to a `.part` dir and
//! is renamed into place, so a torn install can never be mistaken for a pack.

use std::{
    io::Read as _,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use specta::Type;

/// Name of the hash marker file inside an installed pack dir.
const MARKER: &str = ".sha256";
const RUNTIMES_SUBDIR: &str = "runtimes";
const PRODUCT_DIR: &str = "college-course-map";

/// Execution providers the app knows how to register, in the shape the
/// settings priority list stores. `Cpu` is a real list entry ("allowed as
/// fallback"), not an implicit default — a user who wants CPU-only moves it
/// to the top.
#[derive(Type, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EpKind {
    TensorRt,
    Cuda,
    DirectMl,
    CoreMl,
    Cpu,
}

// serde(rename_all = "lowercase") maps variants to "tensorrt", "cuda",
// "directml", "coreml", "cpu" — the strings runtimes.toml `eps` uses and
// runs.execution_provider stores.

impl EpKind {
    /// The pack id that carries this EP on the current platform.
    #[must_use]
    pub fn pack_id(self) -> &'static str {
        match self {
            Self::TensorRt | Self::Cuda => "cuda",
            Self::DirectMl | Self::CoreMl | Self::Cpu => "cpu",
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TensorRt => "tensorrt",
            Self::Cuda => "cuda",
            Self::DirectMl => "directml",
            Self::CoreMl => "coreml",
            Self::Cpu => "cpu",
        }
    }
}

/// Platform-natural default EP priority. `cuda` above `tensorrt` on purpose:
/// TRT compiles engines per input shape and our `BatchLongest` padding
/// produces variable shapes — TRT stays opt-in-by-reorder until measured
/// (EPI-73 flag 1).
#[must_use]
pub fn default_priority() -> Vec<EpKind> {
    #[cfg(target_os = "macos")]
    {
        vec![EpKind::CoreMl, EpKind::Cpu]
    }
    #[cfg(target_os = "windows")]
    {
        vec![
            EpKind::Cuda,
            EpKind::TensorRt,
            EpKind::DirectMl,
            EpKind::Cpu,
        ]
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        vec![EpKind::Cuda, EpKind::TensorRt, EpKind::Cpu]
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct RuntimeManifest {
    /// ONNX Runtime version every pack is pinned to; must match what the
    /// `ort` crate targets (Cargo.toml comment). Lockstep is enforced by
    /// review, recorded here for the UI and the on-disk version dir.
    pub ort_version: String,
    pub pack: Vec<RuntimePack>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct RuntimePack {
    pub id: String,
    /// Rust target triple this pack's archive is built for.
    pub target: String,
    /// EPs compiled into the pack, most capable first. Display metadata —
    /// `is_available()` against the loaded dylib is the runtime truth.
    pub eps: Vec<String>,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

/// Parse the manifest embedded at compile time. Errors only on a malformed
/// build artifact, so callers treat failure as unrecoverable.
pub fn load_manifest() -> Result<RuntimeManifest, String> {
    toml::from_str(include_str!("../runtimes.toml"))
        .map_err(|e| format!("parse runtimes.toml: {e}"))
}

/// The compile-time target triple, used to select manifest packs. Only the
/// triples the release matrix ships are mapped; anything else is a porting
/// task, not a runtime condition.
pub fn current_target() -> Result<&'static str, String> {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        Ok("x86_64-unknown-linux-gnu")
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        Ok("x86_64-pc-windows-msvc")
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        Ok("aarch64-apple-darwin")
    }
    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64")
    )))]
    {
        Err("no runtime packs are pinned for this platform".to_owned())
    }
}

/// Manifest packs applicable to this build, in manifest order.
#[must_use]
pub fn packs_for_target(manifest: &RuntimeManifest) -> Vec<&RuntimePack> {
    let target = current_target().unwrap_or_default();
    manifest
        .pack
        .iter()
        .filter(|p| p.target == target)
        .collect()
}

/// Root for *downloaded* packs (the bundled CPU pack lives in the resource
/// dir instead). `COURSE_CLASSIFIER_RUNTIMES_DIR` overrides for tests and
/// the dev fetch, mirroring `COURSE_CLASSIFIER_MODELS_DIR`.
pub fn runtimes_root() -> Result<PathBuf, String> {
    if let Ok(env) = std::env::var("COURSE_CLASSIFIER_RUNTIMES_DIR") {
        return Ok(PathBuf::from(env));
    }
    dirs::data_dir()
        .map(|dir| dir.join(PRODUCT_DIR).join(RUNTIMES_SUBDIR))
        .ok_or_else(|| "no platform data directory available".to_owned())
}

/// Directory a downloaded pack installs into.
pub fn pack_dir(manifest: &RuntimeManifest, pack: &RuntimePack) -> Result<PathBuf, String> {
    Ok(runtimes_root()?.join(&manifest.ort_version).join(&pack.id))
}

/// The dylib file `ort::init_from` loads, inside an installed pack dir.
/// Official archives place it under `lib/`.
#[must_use]
pub fn dylib_file(pack_dir: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    let name = "onnxruntime.dll";
    #[cfg(target_os = "macos")]
    let name = "libonnxruntime.dylib";
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let name = "libonnxruntime.so";
    pack_dir.join("lib").join(name)
}

/// An installed pack = dylib present + `.sha256` marker matching the
/// manifest archive hash. The marker is written only after a completed,
/// verified extract, and naturally invalidates a pack when the pinned
/// version changes.
#[must_use]
pub fn installed(dir: &Path, pack: &RuntimePack) -> bool {
    dylib_file(dir).exists()
        && std::fs::read_to_string(dir.join(MARKER)).is_ok_and(|m| m.trim() == pack.sha256)
}

/// Download a pack archive, verify its sha256 as it streams, extract it, and
/// rename the result into `dest`. `progress(received_bytes, bytes_per_sec)`
/// is invoked at most every 100 ms. Blocking — callers run it inside
/// `spawn_blocking` (the command) or a plain main (the dev fetch example).
pub fn install_pack(
    client: &reqwest::blocking::Client,
    pack: &RuntimePack,
    dest: &Path,
    progress: &mut dyn FnMut(u64, f64),
) -> Result<(), String> {
    let parent = dest
        .parent()
        .ok_or_else(|| format!("pack dir {} has no parent", dest.display()))?;
    std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;

    let archive = parent.join(format!("{}.archive.part", pack.id));
    download_verified(client, pack, &archive, progress)?;

    let staging = parent.join(format!("{}.part", pack.id));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)
            .map_err(|e| format!("clear stale staging dir {}: {e}", staging.display()))?;
    }
    extract_archive(&archive, &staging)?;
    let _ = std::fs::remove_file(&archive);

    std::fs::write(staging.join(MARKER), &pack.sha256)
        .map_err(|e| format!("write pack marker: {e}"))?;
    if dest.exists() {
        std::fs::remove_dir_all(dest)
            .map_err(|e| format!("remove old pack {}: {e}", dest.display()))?;
    }
    std::fs::rename(&staging, dest).map_err(|e| format!("rename pack into place: {e}"))?;
    Ok(())
}

/// Stream the archive to `dest`, hashing as it goes; delete and error on any
/// mismatch so a torn or tampered download can never be extracted.
fn download_verified(
    client: &reqwest::blocking::Client,
    pack: &RuntimePack,
    dest: &Path,
    progress: &mut dyn FnMut(u64, f64),
) -> Result<(), String> {
    use std::io::Write as _;

    const EMIT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

    let mut response = client
        .get(&pack.url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|e| format!("GET {}: {e}", pack.url))?;

    let mut out = std::io::BufWriter::new(
        std::fs::File::create(dest).map_err(|e| format!("create {}: {e}", dest.display()))?,
    );
    let mut hasher = Sha256::new();
    let mut received: u64 = 0;
    let mut buf = vec![0u8; 1 << 20];
    let started = std::time::Instant::now();
    let mut last_emit = started;
    let mut last_emit_bytes: u64 = 0;
    loop {
        let n = response
            .read(&mut buf)
            .map_err(|e| format!("read {}: {e}", pack.url))?;
        if n == 0 {
            break;
        }
        let chunk = buf.get(..n).unwrap_or_default();
        hasher.update(chunk);
        out.write_all(chunk)
            .map_err(|e| format!("write {}: {e}", dest.display()))?;
        received += n as u64;
        let window = last_emit.elapsed();
        if window >= EMIT_INTERVAL {
            #[expect(
                clippy::cast_precision_loss,
                reason = "byte deltas are far below f64's 2^53 exact-integer range"
            )]
            let bytes_per_sec = (received - last_emit_bytes) as f64 / window.as_secs_f64();
            progress(received, bytes_per_sec);
            last_emit = std::time::Instant::now();
            last_emit_bytes = received;
        }
    }
    out.flush()
        .map_err(|e| format!("flush {}: {e}", dest.display()))?;
    drop(out);
    #[expect(
        clippy::cast_precision_loss,
        reason = "byte counts are far below f64's 2^53 exact-integer range"
    )]
    let avg = received as f64 / started.elapsed().as_secs_f64().max(f64::EPSILON);
    progress(received, avg);

    let digest = format!("{:x}", hasher.finalize());
    if received != pack.size || digest != pack.sha256 {
        let _ = std::fs::remove_file(dest);
        return Err(format!(
            "pack {} failed verification (got {received} bytes, sha256 {digest}; \
             manifest says {} bytes, {}) — archive deleted, retry the download",
            pack.id, pack.size, pack.sha256
        ));
    }
    Ok(())
}

/// Extract the archive into `dest`, stripping the archive's single top-level
/// directory (`onnxruntime-<platform>-<version>/`) so pack contents land at
/// `dest/lib/...` uniformly.
fn extract_archive(archive: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("create {}: {e}", dest.display()))?;
    #[cfg(target_os = "windows")]
    {
        extract_zip(archive, dest)
    }
    #[cfg(not(target_os = "windows"))]
    {
        extract_tgz(archive, dest)
    }
}

#[cfg(not(target_os = "windows"))]
fn extract_tgz(archive: &Path, dest: &Path) -> Result<(), String> {
    let file =
        std::fs::File::open(archive).map_err(|e| format!("open {}: {e}", archive.display()))?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(std::io::BufReader::new(file)));
    for entry in tar
        .entries()
        .map_err(|e| format!("read archive {}: {e}", archive.display()))?
    {
        let mut entry = entry.map_err(|e| format!("read archive entry: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("archive entry path: {e}"))?
            .into_owned();
        let stripped: PathBuf = path.components().skip(1).collect();
        if stripped.as_os_str().is_empty() {
            continue;
        }
        let target = dest.join(stripped);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        // unpack() refuses paths escaping `target`'s dir and recreates
        // symlinks (the official tgz ships `libonnxruntime.so` as one).
        entry
            .unpack(&target)
            .map_err(|e| format!("extract {}: {e}", target.display()))?;
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn extract_zip(archive: &Path, dest: &Path) -> Result<(), String> {
    let file =
        std::fs::File::open(archive).map_err(|e| format!("open {}: {e}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|e| format!("read archive {}: {e}", archive.display()))?;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| format!("read archive entry {i}: {e}"))?;
        // enclosed_name() is the zip-slip guard: entries that would escape
        // the destination resolve to None and are skipped.
        let Some(path) = entry.enclosed_name() else {
            continue;
        };
        let stripped: PathBuf = path.components().skip(1).collect();
        if stripped.as_os_str().is_empty() {
            continue;
        }
        let target = dest.join(stripped);
        if entry.is_dir() {
            std::fs::create_dir_all(&target)
                .map_err(|e| format!("create {}: {e}", target.display()))?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        let mut out = std::fs::File::create(&target)
            .map_err(|e| format!("create {}: {e}", target.display()))?;
        std::io::copy(&mut entry, &mut out)
            .map_err(|e| format!("extract {}: {e}", target.display()))?;
    }
    Ok(())
}

/// What the running process loaded, managed as Tauri state for the Settings
/// UI: which pack, which ONNX Runtime version, and which EPs the pack claims
/// to carry (manifest metadata; per-session resolution lives on the loaded
/// models).
#[derive(Debug, Clone)]
pub struct RuntimeState {
    pub pack_id: String,
    pub ort_version: String,
    pub eps: Vec<String>,
}

/// Choose the pack this process will load, from the user's EP priority list:
/// the first EP whose pack is installed wins. `Cpu` in the list — or nothing
/// installed — resolves to the bundled CPU pack under `resource_dir`, which
/// ships with every build and is the terminal fallback by construction.
pub fn resolve_startup_pack(
    manifest: &RuntimeManifest,
    eps: &[EpKind],
    resource_dir: &Path,
) -> Result<(RuntimeState, PathBuf), String> {
    let packs = packs_for_target(manifest);
    for ep in eps {
        if *ep == EpKind::Cpu {
            break;
        }
        let Some(pack) = packs.iter().find(|p| p.id == ep.pack_id()) else {
            continue;
        };
        let dir = pack_dir(manifest, pack)?;
        if installed(&dir, pack) {
            return Ok((
                RuntimeState {
                    pack_id: pack.id.clone(),
                    ort_version: manifest.ort_version.clone(),
                    eps: pack.eps.clone(),
                },
                dir,
            ));
        }
    }
    let cpu_eps = packs
        .iter()
        .find(|p| p.id == "cpu")
        .map_or_else(|| vec!["cpu".to_owned()], |p| p.eps.clone());
    Ok((
        RuntimeState {
            pack_id: "cpu".to_owned(),
            ort_version: manifest.ort_version.clone(),
            eps: cpu_eps,
        },
        resource_dir.join(RUNTIMES_SUBDIR).join("cpu"),
    ))
}

/// Dev/check-harness location of the fetched CPU pack (`task runtimes:fetch`):
/// `src-tauri/runtimes/cpu` — the same directory `tauri.conf.json` bundles as
/// a resource. Compile-time path; examples only, never the app.
#[must_use]
pub fn dev_cpu_pack_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(RUNTIMES_SUBDIR)
        .join("cpu")
}

/// Load ONNX Runtime from an installed pack directory. Must run before any
/// `ort` API (i.e. before any session is built) and takes effect exactly once
/// per process — switching packs requires an app relaunch.
pub fn init_ort(pack_dir: &Path) -> Result<(), String> {
    let dylib = dylib_file(pack_dir);
    let committed = ort::init_from(&dylib)
        .map_err(|e| format!("load ONNX Runtime from {}: {e}", dylib.display()))?
        .commit();
    if !committed {
        // Another environment was already configured — a caller ordering bug,
        // not a user condition: init_ort must run before any session exists.
        return Err("ONNX Runtime environment was already committed".to_owned());
    }
    Ok(())
}

// ---- IPC surface (registered in lib.rs::specta_builder) ----

/// Per-pack download progress, mirroring `models::ModelDownloadProgress`.
#[derive(Type, Serialize, Deserialize, Debug, Clone, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimeDownloadProgress {
    pub pack_id: String,
    pub received: f64,
    pub total: f64,
    pub bytes_per_sec: f64,
}

/// Coarse "runtime pack state changed" signal — emitted when a pack install
/// finishes or fails; the frontend refetches `runtime_status`.
#[derive(Type, Serialize, Deserialize, Debug, Clone, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimeStateChanged {}

#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimePackStatus {
    pub id: String,
    /// EPs the pack claims to carry (manifest metadata).
    pub eps: Vec<String>,
    pub size_bytes: f64,
    pub installed: bool,
    /// Whether this is the pack the running process loaded.
    pub active: bool,
}

#[derive(Type, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimeStatus {
    pub ort_version: String,
    /// Pack the running process loaded (fixed until relaunch).
    pub active_pack_id: String,
    /// EP the loaded models actually run on; `None` until models load.
    pub resolved_ep: Option<String>,
    /// Platform EPs in the shape the settings priority list stores, for the
    /// settings UI to render reorderable rows without hardcoding.
    pub platform_default_priority: Vec<EpKind>,
    pub packs: Vec<RuntimePackStatus>,
}

#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn runtime_status(
    state: tauri::State<'_, RuntimeState>,
    store: tauri::State<'_, crate::inference::ModelStore>,
) -> Result<RuntimeStatus, String> {
    let manifest = load_manifest()?;
    let packs = packs_for_target(&manifest)
        .into_iter()
        .map(|pack| {
            // The bundled CPU pack ships with every build: always installed.
            let installed = pack.id == "cpu"
                || pack_dir(&manifest, pack)
                    .map(|dir| installed(&dir, pack))
                    .unwrap_or(false);
            #[expect(
                clippy::cast_precision_loss,
                reason = "pack sizes are far below f64's 2^53 exact-integer range"
            )]
            RuntimePackStatus {
                id: pack.id.clone(),
                eps: pack.eps.clone(),
                size_bytes: pack.size as f64,
                installed,
                active: pack.id == state.pack_id,
            }
        })
        .collect();
    Ok(RuntimeStatus {
        ort_version: manifest.ort_version,
        active_pack_id: state.pack_id.clone(),
        resolved_ep: store
            .get()
            .map(|registry| registry.execution_provider().as_str().to_owned()),
        platform_default_priority: default_priority(),
        packs,
    })
}

/// Download + verify + install a runtime pack into the data dir. The new pack
/// is picked up at the next app launch (init-once); the UI says so. Connected
/// builds only — airgap has no network by definition.
#[tauri::command]
#[specta::specta]
pub(crate) async fn download_runtime(app: tauri::AppHandle, pack_id: String) -> Result<(), String> {
    #[cfg(feature = "airgap")]
    {
        let _ = (app, pack_id);
        Err("airgap build: runtime packs cannot be downloaded".to_owned())
    }
    #[cfg(not(feature = "airgap"))]
    {
        tauri::async_runtime::spawn_blocking(move || download_runtime_blocking(&app, &pack_id))
            .await
            .map_err(|e| format!("download task panicked: {e}"))?
    }
}

#[cfg(not(feature = "airgap"))]
fn download_runtime_blocking(app: &tauri::AppHandle, pack_id: &str) -> Result<(), String> {
    use tauri_specta::Event as _;

    let manifest = load_manifest()?;
    let pack = packs_for_target(&manifest)
        .into_iter()
        .find(|p| p.id == pack_id)
        .ok_or_else(|| format!("no pack '{pack_id}' for this platform"))?;
    let dest = pack_dir(&manifest, pack)?;
    if installed(&dest, pack) {
        return Ok(());
    }
    let client = reqwest::blocking::Client::builder()
        .user_agent("college-course-map")
        .timeout(None) // GPU packs are ~200-280 MB; no global deadline
        .build()
        .map_err(|e| format!("build http client: {e}"))?;
    #[expect(
        clippy::cast_precision_loss,
        reason = "pack sizes are far below f64's 2^53 exact-integer range"
    )]
    let result = install_pack(&client, pack, &dest, &mut |received, bytes_per_sec| {
        let _ = RuntimeDownloadProgress {
            pack_id: pack.id.clone(),
            received: received as f64,
            total: pack.size as f64,
            bytes_per_sec,
        }
        .emit(app);
    });
    let _ = RuntimeStateChanged {}.emit(app);
    result
}

#[cfg(test)]
mod tests {
    use super::{EpKind, load_manifest};

    /// The committed runtimes.toml must parse into the typed manifest: every
    /// release-matrix target has a cpu pack, hashes are well-formed, and all
    /// `eps` strings round-trip through `EpKind`. Catches a hand-edited
    /// manifest at test time instead of first launch.
    #[test]
    fn embedded_manifest_is_well_formed() -> Result<(), String> {
        let manifest = load_manifest()?;
        for target in [
            "x86_64-unknown-linux-gnu",
            "x86_64-pc-windows-msvc",
            "aarch64-apple-darwin",
        ] {
            assert!(
                manifest
                    .pack
                    .iter()
                    .any(|p| p.target == target && p.id == "cpu"),
                "{target}: missing cpu pack"
            );
        }
        for pack in &manifest.pack {
            assert_eq!(pack.sha256.len(), 64, "{}: malformed sha256", pack.url);
            assert!(pack.size > 0);
            assert!(
                pack.url.contains(&manifest.ort_version),
                "{}: url not pinned to ort_version {}",
                pack.url,
                manifest.ort_version
            );
            for ep in &pack.eps {
                serde_json::from_value::<EpKind>(serde_json::Value::String(ep.clone()))
                    .map_err(|e| format!("{}: unknown ep {ep}: {e}", pack.id))?;
            }
        }
        Ok(())
    }
}
