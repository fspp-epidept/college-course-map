//! CCM taxonomy reference (EPI-115): the read command behind the CCM
//! Reference activity, and the bundled NCES report it points users to.

use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, Manager, State};

use crate::db::AppDb;

/// Bundled copy of the NCES report (NCES 2012-162rev), relative to the
/// bundle's resource dir. Mapped in `tauri.conf.json` `bundle.resources`.
const REFERENCE_PDF: &str = "docs/2010-college-course-map.pdf";

/// One `ccm_taxonomy` row. 2-digit rows carry `title_short`, 6-digit rows
/// carry `description`; there are no 4-digit rows.
#[derive(Type, Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CcmEntry {
    pub(crate) digit_level: u8,
    pub(crate) code: String,
    pub(crate) title: String,
    pub(crate) title_short: Option<String>,
    pub(crate) description: Option<String>,
}

/// The whole taxonomy (2,167 static rows), ordered by code. Search and
/// filtering happen in the frontend — a small fixed set, not a course table.
#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri injects State by value; cannot be taken by reference at the macro layer"
)]
pub(crate) fn list_ccm_taxonomy(db: State<'_, AppDb>) -> Result<Vec<CcmEntry>, String> {
    let conn = db.ro()?;
    let mut stmt = conn
        .prepare(
            "SELECT digit_level, code, title, title_short, description
             FROM ccm_taxonomy
             ORDER BY code",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(CcmEntry {
                digit_level: row.get(0)?,
                code: row.get(1)?,
                title: row.get(2)?,
                title_short: row.get(3)?,
                description: row.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// Open the bundled NCES report in the platform PDF viewer. Rust-side opener
/// call, like `open_logs_dir`: no capability widening for the `WebView`.
#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri injects AppHandle by value; cannot be taken by reference at the macro layer"
)]
pub(crate) fn open_ccm_reference(app: AppHandle) -> Result<(), String> {
    let path = app
        .path()
        .resource_dir()
        .map_err(|e| format!("resolve bundle resource dir: {e}"))?
        .join(REFERENCE_PDF);
    tauri_plugin_opener::open_path(&path, None::<&str>)
        .map_err(|e| format!("open {}: {e}", path.display()))
}
