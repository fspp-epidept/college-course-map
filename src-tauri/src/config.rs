//! Theme + general-settings persistence (theming decision #106).
//!
//! All config file I/O lives here; the frontend never touches disk. Files live
//! under the product-named config dir `college-course-map` (via `dirs::config_dir`,
//! not Tauri's identifier-based `app_config_dir`):
//!
//! - `settings.json` — general settings; `activeTheme` references a theme by id.
//! - `themes/<id>.json` — one CSS-custom-property token map per file. The id is the
//!   filename stem, never read from the file body.
//!
//! Theme files are untrusted input. The typed structs use `deny_unknown_fields`, so
//! the type itself is the allowlist of `--ui-*` tokens; every string value is then
//! checked by [`validate_value`] (length-bounded, no `url(...)`/CSS-escape sequences)
//! before it can reach the frontend's inert `setProperty` applier.

use std::{fs, path::PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use specta::Type;

/// `light` / `dark` — drives `VueUse` `useColorMode().preference` on the frontend.
#[derive(Type, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ColorScheme {
    Light,
    Dark,
}

/// A `--ui-color-{role}-{shade}` ramp. Each shade is optional so a theme can
/// override a subset. Field names render to the numeric shade keys.
#[derive(Type, Serialize, Deserialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct ColorRamp {
    #[serde(rename = "50", skip_serializing_if = "Option::is_none")]
    pub(crate) s50: Option<String>,
    #[serde(rename = "100", skip_serializing_if = "Option::is_none")]
    pub(crate) s100: Option<String>,
    #[serde(rename = "200", skip_serializing_if = "Option::is_none")]
    pub(crate) s200: Option<String>,
    #[serde(rename = "300", skip_serializing_if = "Option::is_none")]
    pub(crate) s300: Option<String>,
    #[serde(rename = "400", skip_serializing_if = "Option::is_none")]
    pub(crate) s400: Option<String>,
    #[serde(rename = "500", skip_serializing_if = "Option::is_none")]
    pub(crate) s500: Option<String>,
    #[serde(rename = "600", skip_serializing_if = "Option::is_none")]
    pub(crate) s600: Option<String>,
    #[serde(rename = "700", skip_serializing_if = "Option::is_none")]
    pub(crate) s700: Option<String>,
    #[serde(rename = "800", skip_serializing_if = "Option::is_none")]
    pub(crate) s800: Option<String>,
    #[serde(rename = "900", skip_serializing_if = "Option::is_none")]
    pub(crate) s900: Option<String>,
    #[serde(rename = "950", skip_serializing_if = "Option::is_none")]
    pub(crate) s950: Option<String>,
}

/// The role color ramps (`--ui-color-<role>-<shade>`).
#[derive(Type, Serialize, Deserialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct ColorRamps {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) primary: Option<ColorRamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) secondary: Option<ColorRamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) success: Option<ColorRamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) info: Option<ColorRamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) warning: Option<ColorRamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<ColorRamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) neutral: Option<ColorRamp>,
}

/// The semantic `--ui-*` tokens. Field names render to the token suffix; the
/// applier prepends `--ui-` (e.g. `bg_muted` -> `--ui-bg-muted`).
#[derive(Type, Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct SemanticTokens {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) bg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) bg_muted: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) bg_elevated: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) bg_accented: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) bg_inverted: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) text_muted: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) text_dimmed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) text_toned: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) text_highlighted: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) text_inverted: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) border: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) border_muted: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) border_accented: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) border_inverted: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) primary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) secondary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) success: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) info: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) warning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) radius: Option<String>,
}

/// A full theme. `id` is the filename stem (set after load), never read from the
/// file body — `deny_unknown_fields` rejects an `id` key in the JSON.
#[derive(Type, Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Theme {
    #[serde(skip_deserializing)]
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) color_scheme: ColorScheme,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) font: Option<String>,
    #[serde(default)]
    pub(crate) tokens: SemanticTokens,
    #[serde(default)]
    pub(crate) colors: ColorRamps,
}

/// Lightweight listing entry (no token payload) for the theme registry.
#[derive(Type, Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThemeSummary {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) color_scheme: ColorScheme,
}

/// General application settings. `activeTheme` references a theme by id; design
/// tokens themselves live in `themes/*.json`, not here.
#[derive(Type, Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Settings {
    pub(crate) active_theme: String,
    /// Execution-provider priority for inference (EPI-73), most preferred
    /// first. `cpu` is a list entry meaning "allowed as fallback" — the one
    /// mechanism, no separate GPU toggle. Reordering takes effect on the
    /// next model load; switching *packs* (cpu↔cuda dylib) needs a relaunch.
    /// `serde(default)` keeps pre-EPI-73 settings.json files parsing under
    /// `deny_unknown_fields`.
    #[serde(default = "crate::runtime::default_priority")]
    pub(crate) execution_providers: Vec<crate::runtime::EpKind>,
    /// Cap on ORT's intra-op CPU threads during inference (EPI-83).
    /// `0` = auto (ORT default: all physical cores); any value below 1 or
    /// above the machine's cores also behaves as auto — clamp semantics,
    /// no error states. Applied at session build; rides `reload_models`.
    #[serde(default)]
    pub(crate) max_cpu_threads: u32,
    /// Directory holding CUDA/cuDNN libraries to preload at startup (EPI-84)
    /// — for users whose CUDA lives in a conda env or pip venv
    /// (`site-packages/nvidia`) instead of a system install. Takes precedence
    /// over the downloadable support-libs pack; changing it needs a relaunch
    /// (preload is process-lifetime).
    #[serde(default)]
    pub(crate) cuda_library_dir: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            active_theme: "default-light".to_owned(),
            execution_providers: crate::runtime::default_priority(),
            max_cpu_threads: 0,
            cuda_library_dir: None,
        }
    }
}

const PRODUCT_DIR: &str = "college-course-map";

fn config_root() -> Result<PathBuf, String> {
    dirs::config_dir()
        .map(|dir| dir.join(PRODUCT_DIR))
        .ok_or_else(|| "no platform config directory available".to_owned())
}

fn themes_dir() -> Result<PathBuf, String> {
    Ok(config_root()?.join("themes"))
}

fn settings_path() -> Result<PathBuf, String> {
    Ok(config_root()?.join("settings.json"))
}

/// Create the config root and `themes/` dir if absent. Idempotent.
fn ensure_dirs() -> Result<(), String> {
    fs::create_dir_all(themes_dir()?).map_err(|e| e.to_string())
}

/// Theme ids index the filesystem, so they are constrained to a safe charset and
/// can never contain path separators or `..`.
fn validate_id(id: &str) -> Result<(), String> {
    let ok = !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(format!("invalid theme id: {id:?}"))
    }
}

/// Bound a single token value: length-capped and free of CSS-escape / resource-load
/// sequences. Values are applied via `element.style.setProperty` (inert), so this is
/// defense-in-depth against a hand-edited theme file smuggling `url(...)` fetches or
/// breaking out of the property context.
fn validate_value(value: &str) -> Result<(), String> {
    const MAX_LEN: usize = 128;
    const FORBIDDEN: [&str; 6] = ["url(", "expression", "javascript:", "@import", "/*", "*/"];

    if value.len() > MAX_LEN {
        return Err(format!("token value too long (>{MAX_LEN} chars)"));
    }
    let lower = value.to_ascii_lowercase();
    if let Some(bad) = FORBIDDEN.iter().find(|needle| lower.contains(**needle)) {
        return Err(format!("token value contains forbidden sequence {bad:?}"));
    }
    if value
        .chars()
        .any(|c| matches!(c, '<' | '>' | ';' | '{' | '}' | '\\'))
    {
        return Err("token value contains a forbidden character".to_owned());
    }
    Ok(())
}

/// Recursively validate every string leaf of a serialized token tree.
fn validate_json_strings(value: &Value) -> Result<(), String> {
    match value {
        Value::String(s) => validate_value(s),
        Value::Object(map) => map.values().try_for_each(validate_json_strings),
        Value::Array(items) => items.iter().try_for_each(validate_json_strings),
        _ => Ok(()),
    }
}

fn validate_theme(theme: &Theme) -> Result<(), String> {
    if let Some(font) = &theme.font {
        validate_value(font)?;
    }
    let tokens = serde_json::to_value(&theme.tokens).map_err(|e| e.to_string())?;
    let colors = serde_json::to_value(&theme.colors).map_err(|e| e.to_string())?;
    validate_json_strings(&tokens)?;
    validate_json_strings(&colors)
}

/// Read, parse, validate, and id-stamp a single theme file.
fn load_theme(id: &str) -> Result<Theme, String> {
    validate_id(id)?;
    let path = themes_dir()?.join(format!("{id}.json"));
    let raw = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut theme: Theme = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    validate_theme(&theme)?;
    id.clone_into(&mut theme.id);
    Ok(theme)
}

/// List user-supplied themes (built-in themes are registered on the frontend).
/// Unparseable or invalid files are skipped, not fatal, so one bad file can't hide
/// the rest.
#[tauri::command]
#[specta::specta]
pub(crate) fn list_themes() -> Result<Vec<ThemeSummary>, String> {
    ensure_dirs()?;
    let mut out = Vec::new();
    for entry in fs::read_dir(themes_dir()?).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        match load_theme(stem) {
            Ok(theme) => out.push(ThemeSummary {
                id: theme.id,
                name: theme.name,
                color_scheme: theme.color_scheme,
            }),
            Err(err) => eprintln!("skipping theme file {}: {err}", path.display()),
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// Read one user theme by id.
#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn read_theme(id: String) -> Result<Theme, String> {
    load_theme(&id)
}

/// Read settings, creating a default file on first run.
#[tauri::command]
#[specta::specta]
pub(crate) fn read_settings() -> Result<Settings, String> {
    ensure_dirs()?;
    let path = settings_path()?;
    if !path.exists() {
        let defaults = Settings::default();
        write_settings_to_disk(&defaults)?;
        return Ok(defaults);
    }
    let raw = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|e| e.to_string())
}

/// Persist settings. Validates `activeTheme` is a well-formed id.
#[tauri::command]
#[specta::specta]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command arguments are deserialized by value"
)]
pub(crate) fn write_settings(settings: Settings) -> Result<(), String> {
    validate_id(&settings.active_theme)?;
    ensure_dirs()?;
    write_settings_to_disk(&settings)
}

fn write_settings_to_disk(settings: &Settings) -> Result<(), String> {
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    fs::write(settings_path()?, json).map_err(|e| e.to_string())
}
