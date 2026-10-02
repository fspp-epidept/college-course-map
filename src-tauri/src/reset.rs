//! Reset app data (#206). The Settings action can't delete the data folders
//! in place: the database, the log file and the loaded ONNX Runtime pack are
//! all open, and Windows refuses to delete open files. So `request_reset`
//! only writes a marker; the frontend then relaunches through
//! `relaunch_app`. In the next process [`apply_pending`] runs as a
//! pre-logger boot step, after the instance lock (so the old process is
//! gone) and before anything opens the data dir, and [`sweep_trash`] runs
//! as the first `MigratingData` step once the logger is up.
//!
//! Kill-safe by construction: `apply_pending` only renames entries into a
//! `.reset-trash` dir in their own root (same volume, so each rename is
//! atomic) and drops the marker after the rename pass. A process killed
//! during the pass leaves the marker, and the next launch finishes the
//! pass. The slow delete is `sweep_trash`, which also clears whatever a
//! killed sweep left behind.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{boot, paths};

const MARKER: &str = "reset-pending";
const KEEP_SETTINGS: &str = "keep-settings";
const ALL: &str = "all";
const TRASH: &str = ".reset-trash";

/// Write the reset marker. The caller relaunches; the next process does the
/// deleting.
#[tauri::command]
#[specta::specta]
pub(crate) fn request_reset(keep_settings: bool) -> Result<(), String> {
    let data = paths::data_dir()?;
    fs::create_dir_all(&data).map_err(|e| format!("create {}: {e}", data.display()))?;
    let mode = if keep_settings { KEEP_SETTINGS } else { ALL };
    fs::write(data.join(MARKER), mode).map_err(|e| format!("write reset marker: {e}"))
}

/// Every root a reset clears, deduplicated (macOS shares config and data;
/// Windows shares config and the legacy Roaming data dir). The legacy dir
/// is included so the Roaming migration can't move old data back in. No
/// cache root: on Windows it is the data dir; only the `CoreML` leaf.
fn roots() -> Result<Roots, String> {
    Ok(Roots {
        config: paths::config_dir()?,
        others: vec![
            paths::data_dir()?,
            paths::legacy_data_dir()?,
            paths::coreml_cache_dir()?,
        ],
    })
}

struct Roots {
    config: PathBuf,
    others: Vec<PathBuf>,
}

impl Roots {
    /// Each distinct root with the entries a reset leaves in it: settings
    /// and themes when kept, which live in the config root.
    fn with_keep<'a>(&'a self, keep: &'a [&'a str]) -> Vec<(&'a Path, &'a [&'a str])> {
        let mut out: Vec<(&Path, &[&str])> = vec![(&self.config, keep)];
        for dir in &self.others {
            if !out.iter().any(|(root, _)| *root == dir.as_path()) {
                out.push((dir, &[]));
            }
        }
        out
    }
}

/// Pre-logger step: if a reset is pending, move every entry of every root
/// into that root's trash. `Ok(false)`: none was pending. The marker is
/// removed after the pass whatever its outcome, so a failure is reported
/// once and never re-runs against data created afterwards.
pub(crate) fn apply_pending() -> Result<bool, String> {
    apply_at(&paths::data_dir()?.join(MARKER), &roots()?)
}

fn apply_at(marker: &Path, roots: &Roots) -> Result<bool, String> {
    let mode = match fs::read_to_string(marker) {
        Ok(mode) => mode,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(format!("read reset marker: {e}")),
    };
    let result = match mode.as_str() {
        KEEP_SETTINGS => trash_all(roots, &paths::CONFIG_ENTRIES),
        ALL => trash_all(roots, &[]),
        other => Err(format!("unknown reset mode {other:?}; nothing was deleted")),
    };
    fs::remove_file(marker).map_err(|e| format!("remove reset marker: {e}"))?;
    result.map(|()| true)
}

fn trash_all(roots: &Roots, keep: &[&str]) -> Result<(), String> {
    // One batch dir per pass, so a pass never collides with trash a failed
    // sweep left behind.
    let batch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis()
        .to_string();
    for (root, keep) in roots.with_keep(keep) {
        trash(root, keep, &batch).map_err(|e| format!("{}: {e}", root.display()))?;
    }
    Ok(())
}

/// Rename every entry of `root` into `root/.reset-trash/<batch>/`, except
/// `keep`, the marker, the instance lock (held by this process; deleting it
/// would let a second process lock a new file of the same name) and the
/// trash itself. A missing root is already clear. Renaming moves a symlink
/// or junction itself, never its target.
fn trash(root: &Path, keep: &[&str], batch: &str) -> std::io::Result<()> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let dest = root.join(TRASH).join(batch);
    for entry in entries {
        let name = entry?.file_name();
        if [MARKER, boot::INSTANCE_LOCK, TRASH]
            .iter()
            .chain(keep)
            .any(|skip| name == **skip)
        {
            continue;
        }
        fs::create_dir_all(&dest)?;
        fs::rename(root.join(&name), dest.join(&name))?;
    }
    Ok(())
}

/// `MigratingData` step: delete every root's trash. Not fatal; what can't
/// be deleted is reported and retried on the next launch.
pub(crate) fn sweep_trash() -> Result<(), String> {
    let roots = roots()?;
    sweep_at(&roots)
}

fn sweep_at(roots: &Roots) -> Result<(), String> {
    for (root, _) in roots.with_keep(&[]) {
        let trash = root.join(TRASH);
        match fs::remove_dir_all(&trash) {
            Ok(()) => log::info!("reset: deleted {}", trash.display()),
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{} not deleted: {e}", trash.display())),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{MARKER, Roots, TRASH, apply_at, sweep_at};

    fn scratch(name: &str) -> Result<PathBuf, String> {
        let root = std::env::temp_dir().join(format!("ccm-reset-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        Ok(root)
    }

    fn write(path: &Path, body: &str) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(path, body).map_err(|e| e.to_string())
    }

    fn names(dir: &Path) -> Result<Vec<String>, String> {
        let mut out: Vec<String> = fs::read_dir(dir)
            .map_err(|e| e.to_string())?
            .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        out.sort();
        Ok(out)
    }

    /// The Linux layout: separate config and data, plus a cache leaf.
    fn fixture(name: &str, mode: &str) -> Result<(PathBuf, Roots), String> {
        let root = scratch(name)?;
        let (config, data, cache) = (root.join("config"), root.join("data"), root.join("coreml"));
        write(&config.join("settings.json"), "{}")?;
        write(&config.join("themes/mine.json"), "{}")?;
        write(&data.join("app.duckdb"), "")?;
        write(&data.join("models/2/model.onnx"), "")?;
        write(&data.join("session.lock"), "")?;
        write(&data.join(MARKER), mode)?;
        write(&cache.join("model.mlmodelc"), "")?;
        let roots = Roots {
            config,
            others: vec![data.clone(), data, cache],
        };
        Ok((root, roots))
    }

    #[test]
    fn keep_settings_trashes_everything_else() -> Result<(), String> {
        let (root, roots) = fixture("keep", "keep-settings")?;
        let marker = root.join("data").join(MARKER);
        assert_eq!(apply_at(&marker, &roots), Ok(true));
        assert_eq!(names(&root.join("config"))?, ["settings.json", "themes"]);
        assert_eq!(names(&root.join("data"))?, [TRASH, "session.lock"]);
        assert_eq!(names(&root.join("coreml"))?, [TRASH]);
        sweep_at(&roots)?;
        assert_eq!(names(&root.join("data"))?, ["session.lock"]);
        assert!(names(&root.join("coreml"))?.is_empty());
        fs::remove_dir_all(&root).map_err(|e| e.to_string())
    }

    #[test]
    fn all_trashes_settings_too() -> Result<(), String> {
        let (root, roots) = fixture("all", "all")?;
        assert_eq!(apply_at(&root.join("data").join(MARKER), &roots), Ok(true));
        assert_eq!(names(&root.join("config"))?, [TRASH]);
        fs::remove_dir_all(&root).map_err(|e| e.to_string())
    }

    /// macOS: config and data are one dir; settings stay, data goes.
    #[test]
    fn shared_config_and_data_dir() -> Result<(), String> {
        let root = scratch("shared")?;
        let dir = root.join("support");
        write(&dir.join("settings.json"), "{}")?;
        write(&dir.join("app.duckdb"), "")?;
        write(&dir.join(MARKER), "keep-settings")?;
        let roots = Roots {
            config: dir.clone(),
            others: vec![dir.clone()],
        };
        assert_eq!(apply_at(&dir.join(MARKER), &roots), Ok(true));
        assert_eq!(names(&dir)?, [TRASH, "settings.json"]);
        fs::remove_dir_all(&root).map_err(|e| e.to_string())
    }

    /// An unrecognized marker deletes nothing and is consumed.
    #[test]
    fn unknown_mode_deletes_nothing() -> Result<(), String> {
        let (root, roots) = fixture("unknown", "keep-setti")?;
        let marker = root.join("data").join(MARKER);
        assert!(apply_at(&marker, &roots).is_err());
        assert!(!marker.exists());
        assert_eq!(
            names(&root.join("data"))?,
            ["app.duckdb", "models", "session.lock"]
        );
        fs::remove_dir_all(&root).map_err(|e| e.to_string())
    }

    /// A pass killed part way leaves the marker; the rerun finishes it in a
    /// new batch beside the first.
    #[test]
    fn interrupted_pass_finishes_on_rerun() -> Result<(), String> {
        let (root, roots) = fixture("rerun", "all")?;
        let data = root.join("data");
        write(&data.join(TRASH).join("1").join("models/2/model.onnx"), "")?;
        fs::remove_dir_all(data.join("models")).map_err(|e| e.to_string())?;
        assert_eq!(apply_at(&data.join(MARKER), &roots), Ok(true));
        assert_eq!(names(&data)?, [TRASH, "session.lock"]);
        assert_eq!(names(&data.join(TRASH))?.len(), 2);
        sweep_at(&roots)?;
        assert_eq!(names(&data)?, ["session.lock"]);
        fs::remove_dir_all(&root).map_err(|e| e.to_string())
    }

    #[test]
    fn no_marker_is_a_no_op() -> Result<(), String> {
        let (root, roots) = fixture("none", "all")?;
        let data = root.join("data");
        fs::remove_file(data.join(MARKER)).map_err(|e| e.to_string())?;
        assert_eq!(apply_at(&data.join(MARKER), &roots), Ok(false));
        assert_eq!(names(&data)?, ["app.duckdb", "models", "session.lock"]);
        fs::remove_dir_all(&root).map_err(|e| e.to_string())
    }

    /// A linked folder is moved and swept as a link; its target survives.
    #[cfg(unix)]
    #[test]
    fn symlink_target_survives() -> Result<(), String> {
        let (root, roots) = fixture("link", "all")?;
        let outside = root.join("elsewhere");
        write(&outside.join("big.onnx"), "")?;
        std::os::unix::fs::symlink(&outside, root.join("data/linked"))
            .map_err(|e| e.to_string())?;
        assert_eq!(apply_at(&root.join("data").join(MARKER), &roots), Ok(true));
        sweep_at(&roots)?;
        assert_eq!(names(&root.join("data"))?, ["session.lock"]);
        assert_eq!(names(&outside)?, ["big.onnx"]);
        fs::remove_dir_all(&root).map_err(|e| e.to_string())
    }
}
