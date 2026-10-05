# Course Classifier — Keybind management

Companion doc to `CLAUDE.md`. Covers how to split keyboard-shortcut handling across the three layers available in a Tauri + Vue app, with the goal of avoiding the "why doesn't my shortcut fire" debugging session that comes from accidental layering conflicts.

> **Update — hybrid chrome (2026-05-26).** The app now uses custom chrome on Windows/Linux and native chrome on macOS. **Layer 2 (native menu accelerators) only exists on macOS.** On Windows/Linux there is no native menu, so every shortcut the macOS menu would carry is bound instead at **Layer 3 (WebView)** alongside the custom in-WebView menu. The "menu home" column in the tables below describes the macOS native menu and the custom menu's structure equally; the *binding layer* differs by platform. Read "menu accelerator" as "Layer 2 on macOS, Layer 3 on Windows/Linux" throughout this doc.

## The three layers, in order of priority

Three layers can intercept a keypress, each running before the next can see it. Designing this deliberately upfront avoids surprises later.

**Layer 1: OS-level shortcuts (`tauri-plugin-global-shortcut`).** These work even when your app isn't focused. Triggered before any application sees the keypress. Used for things like "press Cmd+Shift+Space anywhere on the system to summon the app." Almost certainly not relevant for an admin tool — registrars don't need to summon a course classifier with a hotkey while they're in Excel. Skip this layer entirely.

**Layer 2: Tauri menu accelerators (set on `MenuItemBuilder` via `.accelerator(...)`).** Run before the WebView sees the keypress, but only when the app has focus. Handled by the OS at the windowing-system level. Useful properties: they show up next to the menu item in the menu bar (so users discover them), they work even when focus is on a non-text element, and on macOS they get the platform-correct rendering (the ⌘ symbol etc.).

**Layer 3: WebView shortcuts (Nuxt UI's `defineShortcuts`, `@vueuse/core`'s `useMagicKeys`, manual `keydown` listeners).** Run inside the WebView, only when the WebView has focus, and only after the OS and menu layers have decided not to intercept. Useful for shortcuts that are component-scoped or that don't have a corresponding menu entry.

Once the priority order is understood, the splitting principle falls out: **pick the highest layer that makes sense for each shortcut, never duplicate across layers.**

## A decision rule per shortcut

For each keyboard shortcut you want, walk this checklist:

**Is there a menu item for this action?** If yes → put the accelerator on the menu item. Don't also bind it in the frontend. The menu accelerator gives users a discoverable shortcut (visible next to the menu label), works across focus contexts, and runs at the right priority level.

**Is the action global to the app, but doesn't fit any menu category?** Rare in practice. If you find yourself wanting this, ask whether the action belongs in a menu — usually it does, you just hadn't thought of where. "Open command palette" → View menu. "Toggle sidebar" → View menu. Acting as if every global action *should* have a menu home reveals the structure of your app's affordances.

**Is the action component-scoped or contextual?** Frontend layer. Examples: `Esc` to close a dialog, `↑/↓` to navigate a dropdown, `/` to focus the search input within a page. These don't belong in a menu — they're behavior, not commands.

**Does the action only make sense when an input is focused?** Frontend layer, and use `keydown` on the input directly rather than a global shortcut. Example: pressing Tab in the column-mapping configurator to advance to the next field.

## The split for this app, concretely

### Menu accelerators (Layer 2)

| Action | Shortcut | Menu home |
|--------|----------|-----------|
| About | — | App |
| Settings | `CmdOrCtrl+,` | App |
| Quit | (predefined) | App |
| Import CSV | `CmdOrCtrl+O` | File |
| Export Results | `CmdOrCtrl+E` | File |
| Cut/Copy/Paste/SelectAll | (predefined) | Edit |
| Start Classification | `CmdOrCtrl+R` | Classify |
| Stop Classification | `CmdOrCtrl+.` | Classify |
| Toggle Sidebar | `CmdOrCtrl+B` | View |
| Toggle Command Palette | `CmdOrCtrl+K` | View |
| Toggle Devtools | `CmdOrCtrl+Shift+I` | View (dev only) |
| Minimize | (predefined) | Window |
| Bring All to Front | (predefined) | Window (macOS) |

On macOS the first submenu is always the application menu (titled with the app name), so About, Settings and Quit live there rather than under Help/Edit/File. There is no Open Recent: it was a placeholder with no feature behind it (#195); add it back with the feature.

### Frontend shortcuts (Layer 3, via Nuxt UI's `defineShortcuts` or `@vueuse/core`)

| Action | Shortcut | Why frontend |
|--------|----------|--------------|
| Close dialog | `Esc` | Component-scoped behavior |
| Focus search input | `/` | Page-level affordance, no menu home |
| Navigate dropdown | `↑/↓/Enter` | Component behavior |
| Submit form | `CmdOrCtrl+Enter` | Form-scoped, conventional |
| Multi-select rows | `Shift+Click`, `Cmd+Click` | TanStack Table built-in |

The asymmetry is real: menu accelerators do most of the heavy lifting, the frontend layer handles the small stuff inside components. This matches how desktop apps actually work.

## Why `Cmd+K` for the command palette goes on the menu

Worth dwelling on this one because instinct says "command palette is a frontend concern, bind it with `useMagicKeys`." But:

The command palette is a global app affordance — it's not scoped to a particular page or component. It deserves a menu entry under View ("Show Command Palette") so users who don't know the shortcut can find it. Once it has a menu entry, the accelerator goes on the menu entry. The frontend then listens for the menu event and opens the palette. Same end-user experience, but discoverable in two ways instead of one, with no shortcut duplication.

This pattern generalizes. Anything that opens a global UI (search, settings, run history, preferences) belongs on the menu, with the menu accelerator as the keyboard binding.

## Implementation pattern

Every custom menu item is a variant of the Rust `MenuAction` enum (`src-tauri/src/menu.rs`). Its serde name is the native item id, and tauri-specta exports it to `src/bindings.ts` as a string union. A click (or accelerator) emits one typed event, `MenuActionTriggered { action }`; there are no per-item `menu:<id>` events.

```rust
// menu.rs — placement is data, so a test can check it on any platform
const FILE_ACTIONS: &[MenuAction] = &[MenuAction::ImportCsv, MenuAction::ExportResults];

// handle_event: parse the item id back into a MenuAction and emit it
MenuActionTriggered { action }.emit(app)
```

The frontend side is `src/composables/useNativeMenu.ts`:

- `useMenuActions()` returns a `Record<MenuAction, () => void>`. Each handler calls the action the in-app UI already uses (workspace store actions, the shared `useStopClassification` mutation, the import dialog mounted in `Workbench.vue`). Classify and Export are component-local to `DatasetDetail`, so their handlers set `workspace.pendingDatasetAction`; `DatasetDetail` consumes it and runs exactly what its button would, or toasts the same blocker that disables the button.
- `useNativeMenu()` listens for `MenuActionTriggered` and dispatches through that table. `Workbench.vue` calls it once. The workbench mounts only once startup is ready (#224), so until then no menu action runs; the macOS items that need the database are also built disabled and enabled on ready, so they show greyed out.
- The Windows/Linux custom titlebar menu (#104) should dispatch through `useMenuActions()` too.

Adding an item can't skip a handler. Coverage is checked at both ends:

- `menu::tests::every_action_is_placed_exactly_once` (`cargo test`) checks that every `MenuAction` variant sits in the menu layout exactly once, under the id the frontend receives.
- vue-tsc (`task check`) fails if the handler table is missing a variant or has a key that is no longer a variant.

Toggle Devtools is the one custom item handled in Rust (a webview concern, debug builds only), so it is a plain id, not a `MenuAction`.

### Never bind a menu shortcut at two layers

On macOS the menu accelerator owns the keypress (Layer 2), so the matching WebView bindings are registered only on Windows/Linux: `Workbench.vue`'s `meta_b` `defineShortcuts`, and `CommandPalette`'s `UDashboardSearch` `shortcut` prop (empty on macOS, which never matches). If both layers saw the key, a toggle would fire twice and cancel itself out.

This rests on an assumption not yet verified on a Mac: that a native accelerator such as `Cmd-K` reaches the menu even while the WebView has focus. If testing shows WKWebView swallows it instead, the fix is to drop the accelerator from that menu item, not to bind the key at both layers.

## The discoverability dividend

A subtler benefit of doing this split correctly: users learn shortcuts from the menu bar. They're looking for "Export" in the File menu, see `⌘E` next to it, and now they know the shortcut. Frontend-bound shortcuts are invisible — users only learn them from documentation or from hitting them by accident. Over time, the menu-anchored shortcuts get used; the frontend-anchored ones get forgotten. This is why putting global shortcuts on the menu (rather than just in `useMagicKeys`) increases adoption of those shortcuts by users.

For an admin tool used by Excel-trained registrars, this matters more than for a developer tool used by power users. Your users will look at the menu. Make sure what they need to find is there.

## Edge cases worth flagging

**Cross-platform accelerator strings.** Use `CmdOrCtrl` (Tauri's portable token) rather than `Cmd` or `Ctrl` directly. Resolves to `Cmd` on macOS and `Ctrl` on Windows/Linux. The menu rendering shows the right symbol per platform.

**Conflicts with system shortcuts.** Avoid `CmdOrCtrl+H` (Hide on macOS), `CmdOrCtrl+M` (Minimize on macOS, conflicts with some Linux WMs), `CmdOrCtrl+W` (Close Window — usually you want this to work as expected, don't override). Standard menu items like `quit()`, `hide()`, `minimize()` get the right accelerators automatically; lean on them.

**Modal/dialog state.** When a modal is open, you usually want global shortcuts to be suppressed. Menu accelerators *don't* care about modal state — they fire regardless. Two options: disable menu items programmatically when a modal opens (Tauri 2 supports this via `MenuItemBuilder::enabled(false)` and runtime updates), or have menu event handlers check current app state and bail if a modal is open. The second is simpler, and would go in `useMenuActions`. Not done yet: today every handler is safe to run over an open modal (they switch activity, toggle chrome, or open a dialog).

**Actions that need a selection.** Export and Start Classification act on the selected dataset. With none selected, the handler switches to Datasets and toasts "Select a dataset first". The items are not disabled natively, which would mean syncing selection state into Rust.

**Devtools shortcut in production builds.** Tauri ships with `Cmd+Option+I` / `F12` enabled in dev builds and disabled in release builds by default. If you add Toggle Devtools as a menu item, gate it on a build-time flag so it doesn't appear in shipped builds.
