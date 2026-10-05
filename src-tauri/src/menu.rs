//! Native application menu (App / File / Edit / Classify / View / Window).
//!
//! Every custom item is a [`MenuAction`]. A click emits one typed
//! [`MenuActionTriggered`] event, and the frontend's `useNativeMenu` composable
//! dispatches it through a `Record<MenuAction, handler>` (see `docs/keybinds.md`).
//! Coverage is checked at both ends:
//! - `every_action_is_placed_exactly_once` (below): every variant is in the menu.
//! - vue-tsc (`task check`): every variant has exactly one frontend handler.
//!
//! Predefined items (Quit, Copy, Minimize, …) perform their OS-native action
//! and never reach `handle_event`. Accelerators mirror `docs/keybinds.md`.
//!
//! The enum, event and layout compile on every platform so `src/bindings.ts`
//! and the coverage test are platform-independent; only building the native
//! menu is macOS-only.

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event;

/// A frontend-handled menu command. The serde name is the native menu item id.
#[derive(Type, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MenuAction {
    About,
    Preferences,
    ImportCsv,
    ExportResults,
    StartClassification,
    StopClassification,
    ToggleSidebar,
    ToggleCommandPalette,
}

/// Emitted when a native menu item (or its accelerator) fires.
#[derive(Type, Serialize, Deserialize, Debug, Clone, Event)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MenuActionTriggered {
    pub action: MenuAction,
}

/// The custom items of each submenu, in order. `build` appends the predefined
/// items; the coverage test reads this table.
#[cfg_attr(
    not(any(target_os = "macos", test)),
    expect(dead_code, reason = "native menu is macOS-only")
)]
const APP_ACTIONS: &[MenuAction] = &[MenuAction::About, MenuAction::Preferences];
#[cfg_attr(
    not(any(target_os = "macos", test)),
    expect(dead_code, reason = "native menu is macOS-only")
)]
const FILE_ACTIONS: &[MenuAction] = &[MenuAction::ImportCsv, MenuAction::ExportResults];
#[cfg_attr(
    not(any(target_os = "macos", test)),
    expect(dead_code, reason = "native menu is macOS-only")
)]
const CLASSIFY_ACTIONS: &[MenuAction] = &[
    MenuAction::StartClassification,
    MenuAction::StopClassification,
];
#[cfg_attr(
    not(any(target_os = "macos", test)),
    expect(dead_code, reason = "native menu is macOS-only")
)]
const VIEW_ACTIONS: &[MenuAction] = &[MenuAction::ToggleSidebar, MenuAction::ToggleCommandPalette];

impl MenuAction {
    /// The native menu item id. Must equal the serde name, which is what the
    /// frontend receives; `every_action_is_placed_exactly_once` checks it.
    #[cfg_attr(
        not(any(target_os = "macos", test)),
        expect(dead_code, reason = "native menu is macOS-only")
    )]
    const fn id(self) -> &'static str {
        match self {
            Self::About => "about",
            Self::Preferences => "preferences",
            Self::ImportCsv => "import_csv",
            Self::ExportResults => "export_results",
            Self::StartClassification => "start_classification",
            Self::StopClassification => "stop_classification",
            Self::ToggleSidebar => "toggle_sidebar",
            Self::ToggleCommandPalette => "toggle_command_palette",
        }
    }
}

#[cfg(target_os = "macos")]
impl MenuAction {
    fn label(self) -> &'static str {
        match self {
            Self::About => "About Course Classifier",
            Self::Preferences => "Settings…",
            Self::ImportCsv => "Import CSV…",
            Self::ExportResults => "Export Results…",
            Self::StartClassification => "Start Classification",
            Self::StopClassification => "Stop Classification",
            Self::ToggleSidebar => "Toggle Sidebar",
            Self::ToggleCommandPalette => "Show Command Palette",
        }
    }

    fn accelerator(self) -> Option<&'static str> {
        match self {
            Self::About => None,
            Self::Preferences => Some("CmdOrCtrl+,"),
            Self::ImportCsv => Some("CmdOrCtrl+O"),
            Self::ExportResults => Some("CmdOrCtrl+E"),
            Self::StartClassification => Some("CmdOrCtrl+R"),
            Self::StopClassification => Some("CmdOrCtrl+."),
            Self::ToggleSidebar => Some("CmdOrCtrl+B"),
            Self::ToggleCommandPalette => Some("CmdOrCtrl+K"),
        }
    }

    /// Whether the action needs the database, so its item stays disabled
    /// until startup is ready (#224).
    const fn needs_boot(self) -> bool {
        matches!(
            self,
            Self::ImportCsv
                | Self::ExportResults
                | Self::StartClassification
                | Self::StopClassification
                | Self::ToggleCommandPalette
        )
    }

    fn from_id(id: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(id.to_owned())).ok()
    }
}

#[cfg(target_os = "macos")]
use tauri::{
    AppHandle, Manager, Runtime,
    menu::{Menu, MenuBuilder, MenuItemBuilder, SubmenuBuilder},
};

/// Toggle Devtools is handled in Rust (a webview concern, debug builds only),
/// so it is a plain id rather than a [`MenuAction`].
#[cfg(all(target_os = "macos", debug_assertions))]
const TOGGLE_DEVTOOLS_ID: &str = "toggle_devtools";

/// A submenu builder pre-filled with `actions`' items.
#[cfg(target_os = "macos")]
fn submenu<'a, R: Runtime>(
    app: &'a AppHandle<R>,
    title: &str,
    actions: &[MenuAction],
) -> tauri::Result<SubmenuBuilder<'a, R, AppHandle<R>>> {
    let mut builder = SubmenuBuilder::new(app, title);
    for &action in actions {
        let item =
            MenuItemBuilder::with_id(action.id(), action.label()).enabled(!action.needs_boot());
        let item = match action.accelerator() {
            Some(accelerator) => item.accelerator(accelerator),
            None => item,
        };
        builder = builder.item(&item.build(app)?);
    }
    Ok(builder)
}

/// Build the full application menu. On macOS the first submenu is the
/// application menu (titled with the app name whatever its label), so it
/// carries About, Settings and Quit by platform convention.
#[cfg(target_os = "macos")]
pub(crate) fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let app_menu = submenu(app, "Course Classifier", APP_ACTIONS)?
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;

    let file = submenu(app, "File", FILE_ACTIONS)?.build()?;

    let edit = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;

    let classify = submenu(app, "Classify", CLASSIFY_ACTIONS)?.build()?;

    let view = submenu(app, "View", VIEW_ACTIONS)?;
    // Devtools toggle is a development-only affordance; omit it from release builds.
    #[cfg(debug_assertions)]
    let view = {
        let toggle_devtools = MenuItemBuilder::with_id(TOGGLE_DEVTOOLS_ID, "Toggle Devtools")
            .accelerator("CmdOrCtrl+Shift+I")
            .build(app)?;
        view.separator().item(&toggle_devtools)
    };
    let view = view.build()?;

    let window = SubmenuBuilder::new(app, "Window")
        .minimize()
        .separator()
        .close_window()
        .build()?;

    MenuBuilder::new(app)
        .items(&[&app_menu, &file, &edit, &classify, &view, &window])
        .build()
}

/// Enable the items [`MenuAction::needs_boot`] built disabled. Called from
/// the boot thread: menu mutations block on the main thread, so the work is
/// posted there and not waited on.
#[cfg(target_os = "macos")]
pub(crate) fn enable_boot_items<R: Runtime>(app: &AppHandle<R>) {
    let handle = app.clone();
    let posted = app.run_on_main_thread(move || {
        let Some(menu) = handle.menu() else {
            return;
        };
        let submenus = menu.items().unwrap_or_default();
        for action in [APP_ACTIONS, FILE_ACTIONS, CLASSIFY_ACTIONS, VIEW_ACTIONS].concat() {
            if !action.needs_boot() {
                continue;
            }
            let item = submenus.iter().find_map(|submenu| {
                submenu
                    .as_submenu()
                    .and_then(|submenu| submenu.get(action.id()))
                    .and_then(|item| item.as_menuitem().cloned())
            });
            match item.map(|item| item.set_enabled(true)) {
                Some(Ok(())) => {}
                Some(Err(e)) => log::warn!("enable menu item {}: {e}", action.id()),
                None => log::warn!("menu item {} not found", action.id()),
            }
        }
    });
    if let Err(e) = posted {
        log::warn!("enable menu items: {e}");
    }
}

/// Route a menu click to the frontend (or handle it natively where it belongs in Rust).
#[cfg(target_os = "macos")]
pub(crate) fn handle_event<R: Runtime>(app: &AppHandle<R>, event: &tauri::menu::MenuEvent) {
    let id = event.id().0.as_str();

    // Devtools is a webview concern, not a frontend-state concern — handle it here.
    #[cfg(debug_assertions)]
    if id == TOGGLE_DEVTOOLS_ID {
        if let Some(window) = app.get_webview_window("main") {
            window.open_devtools();
        }
        return;
    }

    let Some(action) = MenuAction::from_id(id) else {
        log::warn!("unhandled menu item id {id}");
        return;
    };
    if let Err(err) = (MenuActionTriggered { action }).emit(app) {
        log::warn!("failed to emit menu action {id}: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::{APP_ACTIONS, CLASSIFY_ACTIONS, FILE_ACTIONS, MenuAction, VIEW_ACTIONS};
    use specta::{Generics, Type, TypeCollection, datatype::DataType};

    /// Every `MenuAction` variant (as specta reports it, i.e. exactly the
    /// union in `bindings.ts`) is placed in the native menu exactly once, under
    /// an item id equal to the name the frontend receives.
    #[test]
    fn every_action_is_placed_exactly_once() -> Result<(), serde_json::Error> {
        let DataType::Enum(definition) =
            MenuAction::inline(&mut TypeCollection::default(), Generics::Definition)
        else {
            return Err(serde::de::Error::custom("MenuAction is not an enum"));
        };
        let mut variants: Vec<String> = definition
            .variants()
            .iter()
            .map(|(name, _)| name.to_string())
            .collect();
        variants.sort();

        let mut placed = Vec::new();
        for action in [APP_ACTIONS, FILE_ACTIONS, CLASSIFY_ACTIONS, VIEW_ACTIONS].concat() {
            let name = serde_json::from_value::<String>(serde_json::to_value(action)?)?;
            assert_eq!(action.id(), name);
            placed.push(name);
        }
        placed.sort();

        assert_eq!(placed, variants);
        Ok(())
    }
}
