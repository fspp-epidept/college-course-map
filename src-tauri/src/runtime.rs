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
/// (EPI-73 flag 1). macOS is CPU-only (EPI-107): `CoreML` fails at run time
/// on `ModernBERT` and the macOS pack no longer claims it, so it would be
/// filtered out anyway — keeping the default honest avoids a dead row in
/// Settings.
#[must_use]
pub fn default_priority() -> Vec<EpKind> {
    #[cfg(target_os = "macos")]
    {
        vec![EpKind::Cpu]
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

/// How a pack's archives unpack into its directory.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PackLayout {
    /// Official onnxruntime archive: strip the single top-level directory so
    /// contents land at `<pack>/lib/...`.
    Onnxruntime,
    /// Support-library wheels (EPI-84): extract only dynamic libraries,
    /// flattened into `<pack>/lib/` regardless of nesting.
    FlatDylibs,
}

#[derive(Deserialize, Debug, Clone)]
pub struct PackArchive {
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Deserialize, Debug, Clone)]
pub struct RuntimePack {
    pub id: String,
    pub display_name: String,
    /// Rust target triple this pack's archives are built for.
    pub target: String,
    /// EPs compiled into a runtime pack, most capable first. A registration
    /// precondition (EPI-104): a provider absent from the loaded pack's list
    /// is never attempted, because `ort` rc.12 turns a failed-then-retried
    /// registration into a native crash. Per-session resolution (which of
    /// the attempted providers actually registered) lives on the loaded
    /// models. Empty for libs packs, which carry no ONNX Runtime.
    pub eps: Vec<String>,
    pub layout: PackLayout,
    /// Companion libs pack (by id) that satisfies this runtime pack's EP
    /// system dependencies when no system-wide install exists (EPI-84).
    #[serde(default)]
    pub libs: Option<String>,
    pub archives: Vec<PackArchive>,
}

impl RuntimePack {
    #[must_use]
    pub fn total_size(&self) -> u64 {
        self.archives.iter().map(|a| a.size).sum()
    }

    /// Marker content identifying this exact pack build: every archive hash,
    /// newline-joined in manifest order. Any re-pin changes it, invalidating
    /// installed copies.
    #[must_use]
    pub fn marker_value(&self) -> String {
        self.archives
            .iter()
            .map(|a| a.sha256.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
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

/// An installed pack = expected payload present + `.sha256` marker matching
/// every pinned archive hash. The marker is written only after a completed,
/// verified extract, and naturally invalidates a pack when the pin changes.
#[must_use]
pub fn installed(dir: &Path, pack: &RuntimePack) -> bool {
    let payload_present = match pack.layout {
        PackLayout::Onnxruntime => dylib_file(dir).exists(),
        PackLayout::FlatDylibs => dir.join("lib").is_dir(),
    };
    payload_present
        && std::fs::read_to_string(dir.join(MARKER)).is_ok_and(|m| m.trim() == pack.marker_value())
}

/// Download every archive of a pack, verifying each sha256 as it streams,
/// extract per the pack's layout, and rename the result into `dest`.
/// `progress(received_bytes, bytes_per_sec)` is cumulative across archives
/// and invoked at most every 100 ms. Blocking — callers run it inside
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

    let staging = parent.join(format!("{}.part", pack.id));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)
            .map_err(|e| format!("clear stale staging dir {}: {e}", staging.display()))?;
    }

    let mut done_bytes: u64 = 0;
    for (index, archive) in pack.archives.iter().enumerate() {
        let archive_file = parent.join(format!("{}.archive{index}.part", pack.id));
        download_verified(client, archive, &archive_file, &mut |received, bps| {
            progress(done_bytes + received, bps);
        })?;
        let is_zip = Path::new(&archive.url)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("zip") || ext.eq_ignore_ascii_case("whl"));
        extract_archive(&archive_file, &staging, pack.layout, is_zip)?;
        let _ = std::fs::remove_file(&archive_file);
        done_bytes += archive.size;
    }

    std::fs::write(staging.join(MARKER), pack.marker_value())
        .map_err(|e| format!("write pack marker: {e}"))?;
    if dest.exists() {
        std::fs::remove_dir_all(dest)
            .map_err(|e| format!("remove old pack {}: {e}", dest.display()))?;
    }
    std::fs::rename(&staging, dest).map_err(|e| format!("rename pack into place: {e}"))?;
    Ok(())
}

/// Remove a damaged downloaded pack so `installed()` turns false and the
/// Settings UI offers the download again (EPI-86). Best-effort: on failure
/// the next launch retries the same invalidation.
pub fn invalidate_pack(dir: &Path) {
    if let Err(e) = std::fs::remove_dir_all(dir) {
        eprintln!("failed to remove damaged pack {}: {e}", dir.display());
    }
}

/// Delete leftover `*.part` staging entries (archive files and extract dirs)
/// under the runtimes root (EPI-88). `install_pack` only cleans up when the
/// same pack's download is retried, so a mid-download kill would otherwise
/// leak partial archives forever. Runs at startup, before any download can
/// be in flight. Best-effort; returns how many entries were removed.
#[must_use]
pub fn sweep_partial_downloads(root: &Path) -> usize {
    let Ok(versions) = std::fs::read_dir(root) else {
        return 0;
    };
    let mut swept = 0usize;
    for version_dir in versions.flatten().filter(|e| e.path().is_dir()) {
        let Ok(entries) = std::fs::read_dir(version_dir.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "part") {
                continue;
            }
            let removed = if path.is_dir() {
                std::fs::remove_dir_all(&path).is_ok()
            } else {
                std::fs::remove_file(&path).is_ok()
            };
            if removed {
                swept += 1;
            }
        }
    }
    swept
}

/// Stream one archive to `dest`, hashing as it goes; delete and error on any
/// mismatch so a torn or tampered download can never be extracted.
fn download_verified(
    client: &reqwest::blocking::Client,
    archive: &PackArchive,
    dest: &Path,
    progress: &mut dyn FnMut(u64, f64),
) -> Result<(), String> {
    use std::io::Write as _;

    const EMIT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

    let mut response = client
        .get(&archive.url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|e| format!("GET {}: {e}", archive.url))?;

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
            .map_err(|e| format!("read {}: {e}", archive.url))?;
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
    if received != archive.size || digest != archive.sha256 {
        let _ = std::fs::remove_file(dest);
        return Err(format!(
            "{} failed verification (got {received} bytes, sha256 {digest}; \
             manifest says {} bytes, {}) — archive deleted, retry the download",
            archive.url, archive.size, archive.sha256
        ));
    }
    Ok(())
}

fn is_dylib_name(name: &std::ffi::OsStr) -> bool {
    let lossy = name.to_string_lossy();
    lossy.contains(".so") || lossy.ends_with(".dll") || lossy.ends_with(".dylib")
}

/// Where an archive entry lands under the pack dir, per layout. `None` =
/// skip the entry.
fn entry_target(path: &Path, layout: PackLayout) -> Option<PathBuf> {
    match layout {
        // Strip the archive's single top-level directory
        // (`onnxruntime-<platform>-<version>/`) so contents land at `lib/...`.
        PackLayout::Onnxruntime => {
            let stripped: PathBuf = path.components().skip(1).collect();
            (!stripped.as_os_str().is_empty()).then_some(stripped)
        }
        // Wheels nest libs per component (`nvidia/<comp>/lib/...`); keep only
        // dynamic libraries, flattened into `lib/`.
        PackLayout::FlatDylibs => {
            let name = path.file_name()?;
            is_dylib_name(name).then(|| Path::new("lib").join(name))
        }
    }
}

/// Extract an archive into `dest` per the pack layout. Wheels and Windows
/// onnxruntime releases are zips; Linux/macOS onnxruntime releases are tgz.
fn extract_archive(
    archive: &Path,
    dest: &Path,
    layout: PackLayout,
    is_zip: bool,
) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("create {}: {e}", dest.display()))?;
    if is_zip {
        extract_zip(archive, dest, layout)
    } else {
        extract_tgz(archive, dest, layout)
    }
}

#[cfg(not(target_os = "windows"))]
fn extract_tgz(archive: &Path, dest: &Path, layout: PackLayout) -> Result<(), String> {
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
        let Some(relative) = entry_target(&path, layout) else {
            continue;
        };
        let target = dest.join(relative);
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

/// The Windows onnxruntime release is a zip and never a tgz; compiling the
/// tar/flate2 path out keeps the dependency tree honest.
#[cfg(target_os = "windows")]
fn extract_tgz(archive: &Path, _dest: &Path, _layout: PackLayout) -> Result<(), String> {
    Err(format!(
        "tgz archive {} unsupported on Windows — the manifest should pin zips here",
        archive.display()
    ))
}

fn extract_zip(archive: &Path, dest: &Path, layout: PackLayout) -> Result<(), String> {
    let file =
        std::fs::File::open(archive).map_err(|e| format!("open {}: {e}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|e| format!("read archive {}: {e}", archive.display()))?;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| format!("read archive entry {i}: {e}"))?;
        if entry.is_dir() {
            continue;
        }
        // enclosed_name() is the zip-slip guard: entries that would escape
        // the destination resolve to None and are skipped.
        let Some(path) = entry.enclosed_name() else {
            continue;
        };
        let Some(relative) = entry_target(&path, layout) else {
            continue;
        };
        let target = dest.join(relative);
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

/// Preload every dynamic library under `dir` (recursively) into the process
/// so a subsequently-registered EP's `NEEDED` deps resolve without a system
/// install or `LD_LIBRARY_PATH` (EPI-84). Load order is discovered by
/// fixpoint: each pass retries what failed (deps not loaded yet); a pass
/// with no progress stops. Returns how many libraries loaded. Must run
/// before EP registration (i.e. before models load); handles stay loaded
/// for the process lifetime by design.
pub fn preload_support_libs(dir: &Path) -> Result<usize, String> {
    let mut pending = Vec::new();
    collect_dylibs(dir, 0, &mut pending)?;
    let mut loaded = 0usize;
    loop {
        let before = pending.len();
        pending.retain(|path| ort::util::preload_dylib(path.as_os_str()).is_err());
        loaded += before - pending.len();
        if pending.is_empty() || pending.len() == before {
            break;
        }
    }
    for path in &pending {
        eprintln!("support lib not preloaded: {}", path.display());
    }
    Ok(loaded)
}

/// Recursively gather dynamic-library files. Depth-capped: pack layouts are
/// `lib/*.so`, and user-pointed CUDA dirs (venv `site-packages/nvidia`) nest
/// at most `component/lib/…`.
fn collect_dylibs(dir: &Path, depth: u8, out: &mut Vec<PathBuf>) -> Result<(), String> {
    if depth > 3 {
        return Ok(());
    }
    let entries = std::fs::read_dir(dir).map_err(|e| format!("read dir {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("read dir entry in {}: {e}", dir.display()))?;
        let path = entry.path();
        if path.is_dir() {
            collect_dylibs(&path, depth + 1, out)?;
        } else if path.file_name().is_some_and(is_dylib_name) {
            out.push(path);
        }
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
    /// Startup conditions the user must be able to see without a terminal
    /// (EPI-87): damaged-pack fallback, missing CUDA library directory,
    /// failed preloads — and, from `lib.rs`, a database WAL set aside at
    /// open (EPI-105). Fixed for the process lifetime; surfaced by
    /// `runtime_status`.
    pub notices: Vec<String>,
}

impl RuntimeState {
    /// The user's EP priority list restricted to providers the loaded pack
    /// claims to carry (EPI-104), order preserved. `Cpu` always passes: it's
    /// the implicit provider in every ONNX Runtime build and the fallback
    /// boundary `register_eps` stops at. Attempting a provider the dylib
    /// lacks is not a harmless "not used" — see `inference::FailedEps`.
    #[must_use]
    pub fn registrable(&self, priority: &[EpKind]) -> Vec<EpKind> {
        priority
            .iter()
            .copied()
            .filter(|ep| *ep == EpKind::Cpu || self.eps.iter().any(|e| e == ep.as_str()))
            .collect()
    }
}

/// Choose the pack this process will load. An explicitly preferred pack
/// (EPI-94, Settings → Compute "Make active") wins when installed — an
/// explicit choice can never be shadowed by manifest order. Otherwise the
/// user's EP priority list is scanned: the first EP with an *installed* pack
/// claiming it wins (manifest order breaks ties). `Cpu` — preferred, in the
/// list, or nothing installed — resolves to the bundled CPU pack under
/// `resource_dir`, which ships with every build and is the terminal fallback
/// by construction.
pub fn resolve_startup_pack(
    manifest: &RuntimeManifest,
    preferred_pack: Option<&str>,
    eps: &[EpKind],
    resource_dir: &Path,
) -> Result<(RuntimeState, PathBuf), String> {
    let packs = packs_for_target(manifest);
    let state_for = |pack: &RuntimePack| RuntimeState {
        pack_id: pack.id.clone(),
        ort_version: manifest.ort_version.clone(),
        eps: pack.eps.clone(),
        notices: Vec::new(),
    };
    match preferred_pack {
        Some("cpu") => return Ok(bundled_cpu(manifest, &packs, resource_dir)),
        Some(id) => {
            // Runtime packs only (`eps` non-empty) — a libs pack carries no
            // ONNX Runtime. Not-installed falls through to the scan.
            if let Some(pack) = packs.iter().find(|p| p.id == id && !p.eps.is_empty()) {
                let dir = pack_dir(manifest, pack)?;
                if installed(&dir, pack) {
                    return Ok((state_for(pack), dir));
                }
            }
        }
        None => {}
    }
    for ep in eps {
        if *ep == EpKind::Cpu {
            break;
        }
        for pack in packs
            .iter()
            .filter(|p| p.id != "cpu" && p.eps.iter().any(|e| e == ep.as_str()))
        {
            let dir = pack_dir(manifest, pack)?;
            if installed(&dir, pack) {
                return Ok((state_for(pack), dir));
            }
        }
    }
    Ok(bundled_cpu(manifest, &packs, resource_dir))
}

/// The bundled CPU pack — always present, the terminal fallback.
fn bundled_cpu(
    manifest: &RuntimeManifest,
    packs: &[&RuntimePack],
    resource_dir: &Path,
) -> (RuntimeState, PathBuf) {
    let cpu_eps = packs
        .iter()
        .find(|p| p.id == "cpu")
        .map_or_else(|| vec!["cpu".to_owned()], |p| p.eps.clone());
    (
        RuntimeState {
            pack_id: "cpu".to_owned(),
            ort_version: manifest.ort_version.clone(),
            eps: cpu_eps,
            notices: Vec::new(),
        },
        resource_dir.join(RUNTIMES_SUBDIR).join("cpu"),
    )
}

/// The installed companion libs-pack directory for the pack this process
/// loaded, if the manifest names one and it's downloaded. `None` for CPU
/// packs, libs-less GPU packs, or a not-yet-downloaded companion.
#[must_use]
pub fn installed_libs_dir(manifest: &RuntimeManifest, state: &RuntimeState) -> Option<PathBuf> {
    let packs = packs_for_target(manifest);
    let libs_id = packs
        .iter()
        .find(|p| p.id == state.pack_id)?
        .libs
        .as_deref()?;
    let libs_pack = packs.iter().find(|p| p.id == libs_id)?;
    let dir = pack_dir(manifest, libs_pack).ok()?;
    installed(&dir, libs_pack).then_some(dir)
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

/// Full runtime startup: sweep partial downloads (EPI-88), resolve and load
/// the pack — falling back to the bundled CPU pack when a downloaded pack is
/// damaged (EPI-86) — and preload GPU support libraries (EPI-84). Degraded
/// outcomes are recorded as notices for the Settings UI (EPI-87).
pub(crate) fn startup(
    settings: &crate::config::Settings,
    resource_dir: &Path,
) -> Result<RuntimeState, String> {
    let manifest = load_manifest()?;
    // No download can be in flight this early — clear partial archives left
    // by a mid-download kill.
    let swept = runtimes_root().map_or(0, |root| sweep_partial_downloads(&root));
    if swept > 0 {
        eprintln!("startup: swept {swept} partial pack download(s)");
    }
    let (mut state, pack_dir) = resolve_startup_pack(
        &manifest,
        settings.preferred_pack.as_deref(),
        &settings.execution_providers,
        resource_dir,
    )?;
    // A downloaded pack that fails to load is damaged (the install protocol
    // is atomic, so this is post-install damage: disk corruption, quarantine,
    // manual deletion). Remove it and fall through to the bundled CPU pack
    // instead of failing startup with no UI to recover from. A failed
    // `init_from` does not commit the environment, so the second init is
    // valid; bundled-pack failure stays fatal — the install itself is broken.
    if let Err(e) = init_ort(&pack_dir) {
        if state.pack_id == "cpu" {
            return Err(e);
        }
        eprintln!(
            "startup: runtime pack '{}' failed to load: {e}",
            state.pack_id
        );
        invalidate_pack(&pack_dir);
        let failed = state.pack_id.clone();
        let (cpu_state, cpu_dir) =
            resolve_startup_pack(&manifest, Some("cpu"), &[EpKind::Cpu], resource_dir)?;
        init_ort(&cpu_dir)?;
        state = cpu_state;
        state.notices.push(format!(
            "The '{failed}' runtime pack was damaged and has been removed — running \
             on CPU. Re-download the pack to restore GPU support."
        ));
    }
    // GPU support libraries: preload CUDA/cuDNN dylibs so EP registration at
    // model-load time resolves them without a system install. Precedence: the
    // user-pointed directory (conda/venv CUDA), else the pack's downloaded
    // companion libs pack. Failure to preload is not fatal — registration
    // falls back exactly as when no libs exist — but every degraded case
    // leaves a notice.
    let user_dir = match settings.cuda_library_dir.as_deref() {
        Some(dir) if Path::new(dir).is_dir() => Some(Path::new(dir)),
        Some(dir) => {
            state.notices.push(format!(
                "The CUDA library directory '{dir}' no longer exists and was skipped. \
                 Update or clear it under Settings \u{2192} Inference."
            ));
            None
        }
        None => None,
    };
    let libs_dir = installed_libs_dir(&manifest, &state);
    if let Some(dir) = user_dir.or(libs_dir.as_deref()) {
        match preload_support_libs(dir) {
            Ok(count) => eprintln!(
                "startup: preloaded {count} GPU support libs from {}",
                dir.display()
            ),
            Err(e) => {
                eprintln!("startup: GPU support lib preload skipped: {e}");
                state.notices.push(format!(
                    "GPU support libraries could not be loaded from {}: {e}",
                    dir.display()
                ));
            }
        }
    } else if packs_for_target(&manifest)
        .iter()
        .find(|p| p.id == state.pack_id)
        .is_some_and(|p| p.libs.is_some())
    {
        state.notices.push(
            "CUDA support libraries are not installed, so the CUDA provider cannot \
             start. Download the support pack below or point at an existing CUDA \
             library directory."
                .to_owned(),
        );
    }
    Ok(state)
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
    pub display_name: String,
    /// EPs the pack claims to carry (manifest metadata; empty = libs pack).
    pub eps: Vec<String>,
    pub size_bytes: f64,
    pub installed: bool,
    /// Whether this is the pack the running process loaded (always false for
    /// libs packs — they are preloaded next to a runtime pack, not loaded as
    /// one).
    pub active: bool,
    /// Companion libs pack id, when this runtime pack has one (EPI-84).
    pub libs: Option<String>,
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
    /// Startup runtime conditions worth a warning in Settings (EPI-87):
    /// damaged-pack fallback, missing CUDA directory, failed preloads.
    pub notices: Vec<String>,
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
                || pack_dir(&manifest, pack).is_ok_and(|dir| installed(&dir, pack));
            #[expect(
                clippy::cast_precision_loss,
                reason = "pack sizes are far below f64's 2^53 exact-integer range"
            )]
            RuntimePackStatus {
                id: pack.id.clone(),
                display_name: pack.display_name.clone(),
                eps: pack.eps.clone(),
                size_bytes: pack.total_size() as f64,
                installed,
                active: pack.id == state.pack_id,
                libs: pack.libs.clone(),
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
        notices: state.notices.clone(),
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
    let manifest = load_manifest()?;
    let packs = packs_for_target(&manifest);
    let pack = packs
        .iter()
        .find(|p| p.id == pack_id)
        .ok_or_else(|| format!("no pack '{pack_id}' for this platform"))?;
    let client = reqwest::blocking::Client::builder()
        .user_agent("college-course-map")
        .timeout(None) // GPU/libs packs are 200 MB - 1.2 GB; no global deadline
        .build()
        .map_err(|e| format!("build http client: {e}"))?;
    let result = install_pack_with_progress(app, &client, &manifest, pack).and_then(|()| {
        // One Download action fetches everything the backend needs (EPI-94):
        // a runtime pack's companion support-libs pack rides along, so users
        // never learn they were two files.
        match &pack.libs {
            Some(libs_id) => {
                let libs = packs
                    .iter()
                    .find(|p| &p.id == libs_id)
                    .ok_or_else(|| format!("manifest names missing libs pack '{libs_id}'"))?;
                install_pack_with_progress(app, &client, &manifest, libs)
            }
            None => Ok(()),
        }
    });
    use tauri_specta::Event as _;
    let _ = RuntimeStateChanged {}.emit(app);
    result
}

/// Install one pack (skipping if already installed), streaming progress
/// events keyed by its id.
#[cfg(not(feature = "airgap"))]
fn install_pack_with_progress(
    app: &tauri::AppHandle,
    client: &reqwest::blocking::Client,
    manifest: &RuntimeManifest,
    pack: &RuntimePack,
) -> Result<(), String> {
    use tauri_specta::Event as _;

    let dest = pack_dir(manifest, pack)?;
    if installed(&dest, pack) {
        return Ok(());
    }
    let total = pack.total_size();
    #[expect(
        clippy::cast_precision_loss,
        reason = "pack sizes are far below f64's 2^53 exact-integer range"
    )]
    install_pack(client, pack, &dest, &mut |received, bytes_per_sec| {
        let _ = RuntimeDownloadProgress {
            pack_id: pack.id.clone(),
            received: received as f64,
            total: total as f64,
            bytes_per_sec,
        }
        .emit(app);
    })
}

/// Restart the app (EPI-94): the one way to switch runtime packs, since ONNX
/// Runtime loads exactly once per process.
#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn relaunch_app(app: tauri::AppHandle) {
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::{EpKind, RuntimeState, load_manifest};

    fn state(eps: &[&str]) -> RuntimeState {
        RuntimeState {
            pack_id: "test".to_owned(),
            ort_version: "0".to_owned(),
            eps: eps.iter().map(|e| (*e).to_owned()).collect(),
            notices: Vec::new(),
        }
    }

    /// The settings priority only keeps providers the loaded pack claims,
    /// in the user's order, with `Cpu` always allowed through (EPI-104).
    #[test]
    fn registrable_filters_priority_by_pack_eps() {
        let windows_default = [
            EpKind::Cuda,
            EpKind::TensorRt,
            EpKind::DirectMl,
            EpKind::Cpu,
        ];
        // The shipped CPU-only pack: nothing but CPU survives — DirectML is
        // never attempted, which is the whole fix.
        assert_eq!(state(&["cpu"]).registrable(&windows_default), [EpKind::Cpu]);
        // A CUDA pack keeps the GPU entries it carries, in priority order.
        assert_eq!(
            state(&["cuda", "tensorrt"]).registrable(&windows_default),
            [EpKind::Cuda, EpKind::TensorRt, EpKind::Cpu]
        );
        // User reorder is respected; `Cpu` passes even if the pack list
        // omits it.
        assert_eq!(
            state(&["tensorrt"]).registrable(&[EpKind::Cpu, EpKind::TensorRt]),
            [EpKind::Cpu, EpKind::TensorRt]
        );
    }

    /// The startup sweep must remove `.part` archives and staging dirs while
    /// leaving installed pack directories untouched (EPI-88).
    #[test]
    fn sweep_removes_only_part_entries() -> Result<(), String> {
        let root = std::env::temp_dir().join(format!("ccm-sweep-test-{}", std::process::id()));
        let version = root.join("1.24.2");
        let staging = version.join("cuda13.part");
        let archive = version.join("cuda13.archive0.part");
        let installed = version.join("cpu");
        for dir in [&staging, &installed] {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        for file in [
            &staging.join("libx.so"),
            &archive,
            &installed.join(".sha256"),
        ] {
            std::fs::write(file, b"x").map_err(|e| e.to_string())?;
        }

        let swept = super::sweep_partial_downloads(&root);

        assert_eq!(swept, 2, "one staging dir + one archive file");
        assert!(!staging.exists());
        assert!(!archive.exists());
        assert!(
            installed.join(".sha256").exists(),
            "installed pack untouched"
        );
        std::fs::remove_dir_all(&root).map_err(|e| e.to_string())?;
        Ok(())
    }

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
            assert!(!pack.archives.is_empty(), "{}: no archives", pack.id);
            for archive in &pack.archives {
                assert_eq!(
                    archive.sha256.len(),
                    64,
                    "{}: malformed sha256",
                    archive.url
                );
                assert!(archive.size > 0);
            }
            // Runtime packs (they carry EPs) must pin the lockstep ONNX
            // Runtime version in their URLs; libs packs pin NVIDIA versions.
            if !pack.eps.is_empty() {
                assert_eq!(pack.layout, super::PackLayout::Onnxruntime, "{}", pack.id);
                for archive in &pack.archives {
                    assert!(
                        archive.url.contains(&manifest.ort_version),
                        "{}: url not pinned to ort_version {}",
                        archive.url,
                        manifest.ort_version
                    );
                }
            }
            for ep in &pack.eps {
                serde_json::from_value::<EpKind>(serde_json::Value::String(ep.clone()))
                    .map_err(|e| format!("{}: unknown ep {ep}: {e}", pack.id))?;
            }
            // A `libs` reference must point at a real libs pack for the same
            // target.
            if let Some(libs_id) = &pack.libs {
                assert!(
                    manifest.pack.iter().any(|p| &p.id == libs_id
                        && p.target == pack.target
                        && p.layout == super::PackLayout::FlatDylibs),
                    "{}: libs pack '{libs_id}' missing for target {}",
                    pack.id,
                    pack.target
                );
            }
        }
        Ok(())
    }
}
