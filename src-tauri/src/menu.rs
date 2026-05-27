//! Native application menu (File / Edit / Run / View / Window / Help).
//!
//! Custom items carry stable ids and fire `menu:<id>` Tauri events for the
//! frontend to handle via the `useNativeMenu` composable (see `docs/keybinds.md`).
//! Predefined items (Quit, Copy, Minimize, …) perform their OS-native action and
//! do not reach `handle_event`. Accelerators mirror the table in `docs/keybinds.md`.

use tauri::{
    AppHandle, Emitter, Manager, Runtime,
    menu::{Menu, MenuBuilder, MenuItemBuilder, SubmenuBuilder},
};

/// Build the full application menu.
pub(crate) fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let import = MenuItemBuilder::new("Import CSV…")
        .id("import_csv")
        .accelerator("CmdOrCtrl+O")
        .build(app)?;
    let export = MenuItemBuilder::new("Export Results…")
        .id("export_results")
        .accelerator("CmdOrCtrl+E")
        .build(app)?;
    let open_recent = MenuItemBuilder::new("Open Recent…")
        .id("open_recent")
        .accelerator("CmdOrCtrl+Shift+O")
        .build(app)?;
    let file = SubmenuBuilder::new(app, "File")
        .item(&import)
        .item(&export)
        .item(&open_recent)
        .separator()
        .quit()
        .build()?;

    let preferences = MenuItemBuilder::new("Preferences…")
        .id("preferences")
        .accelerator("CmdOrCtrl+,")
        .build(app)?;
    let edit = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .separator()
        .item(&preferences)
        .build()?;

    let start = MenuItemBuilder::new("Start Classification")
        .id("start_classification")
        .accelerator("CmdOrCtrl+R")
        .build(app)?;
    let pause = MenuItemBuilder::new("Pause Run")
        .id("pause_run")
        .accelerator("CmdOrCtrl+.")
        .build(app)?;
    let run = SubmenuBuilder::new(app, "Run")
        .item(&start)
        .item(&pause)
        .build()?;

    let toggle_sidebar = MenuItemBuilder::new("Toggle Sidebar")
        .id("toggle_sidebar")
        .accelerator("CmdOrCtrl+B")
        .build(app)?;
    let toggle_command_palette = MenuItemBuilder::new("Show Command Palette")
        .id("toggle_command_palette")
        .accelerator("CmdOrCtrl+K")
        .build(app)?;
    let view = {
        let builder = SubmenuBuilder::new(app, "View")
            .item(&toggle_sidebar)
            .item(&toggle_command_palette);
        // Devtools toggle is a development-only affordance; omit it from release builds.
        #[cfg(debug_assertions)]
        let builder = {
            let toggle_devtools = MenuItemBuilder::new("Toggle Devtools")
                .id("toggle_devtools")
                .accelerator("CmdOrCtrl+Shift+I")
                .build(app)?;
            builder.separator().item(&toggle_devtools)
        };
        builder.build()?
    };

    let window = SubmenuBuilder::new(app, "Window")
        .minimize()
        .separator()
        .close_window()
        .build()?;

    let about = MenuItemBuilder::new("About Course Classifier")
        .id("about")
        .build(app)?;
    let help = SubmenuBuilder::new(app, "Help").item(&about).build()?;

    MenuBuilder::new(app)
        .items(&[&file, &edit, &run, &view, &window, &help])
        .build()
}

/// Route a menu click to the frontend (or handle it natively where it belongs in Rust).
pub(crate) fn handle_event<R: Runtime>(app: &AppHandle<R>, event: &tauri::menu::MenuEvent) {
    let id = event.id().0.as_str();

    // Devtools is a webview concern, not a frontend-state concern — handle it here.
    #[cfg(debug_assertions)]
    if id == "toggle_devtools" {
        if let Some(window) = app.get_webview_window("main") {
            window.open_devtools();
        }
        return;
    }

    if let Err(err) = app.emit(&format!("menu:{id}"), ()) {
        eprintln!("failed to emit menu event menu:{id}: {err}");
    }
}
