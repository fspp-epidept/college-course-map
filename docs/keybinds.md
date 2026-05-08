# Course Classifier — Keybind management

Companion doc to the main handoff doc. Covers how to split keyboard-shortcut handling across the three layers available in a Tauri + Vue app, with the goal of avoiding the "why doesn't my shortcut fire" debugging session that comes from accidental layering conflicts.

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
| Import CSV | `CmdOrCtrl+O` | File |
| Export Results | `CmdOrCtrl+E` | File |
| Open Recent | `CmdOrCtrl+Shift+O` | File |
| Quit | (predefined) | File / App |
| Cut/Copy/Paste/SelectAll | (predefined) | Edit |
| Preferences | `CmdOrCtrl+,` | Edit / App |
| Start Classification | `CmdOrCtrl+R` | Run |
| Pause Run | `CmdOrCtrl+.` | Run |
| Toggle Sidebar | `CmdOrCtrl+B` | View |
| Toggle Command Palette | `CmdOrCtrl+K` | View |
| Toggle Devtools | `CmdOrCtrl+Shift+I` | View (dev only) |
| Minimize | (predefined) | Window |
| Bring All to Front | (predefined) | Window (macOS) |

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

Rust side defines the menu and emits events when items are clicked:

```rust
let toggle_palette = MenuItemBuilder::new("Show Command Palette")
    .id("toggle_command_palette")
    .accelerator("CmdOrCtrl+K")
    .build(app)?;

// Add to View submenu, then on the global on_menu_event handler:
app.on_menu_event(move |app, event| {
    match event.id().0.as_str() {
        "import_csv" => { app.emit("menu:import_csv", ()).unwrap(); }
        "toggle_command_palette" => { app.emit("menu:toggle_command_palette", ()).unwrap(); }
        "start_classification" => { app.emit("menu:start_classification", ()).unwrap(); }
        // ...
        _ => {}
    }
});
```

Frontend side has a single composable that bridges menu events to whatever app-level state needs to react:

```typescript
// composables/useNativeMenu.ts
import { listen } from '@tauri-apps/api/event'
import { onMounted, onUnmounted } from 'vue'

export function useNativeMenu() {
  const palette = useCommandPalette()
  const sidebar = useSidebar()
  const importDialog = useImportDialog()
  // ...

  let unlisteners: Array<() => void> = []

  onMounted(async () => {
    unlisteners.push(await listen('menu:toggle_command_palette', () => palette.toggle()))
    unlisteners.push(await listen('menu:toggle_sidebar', () => sidebar.toggle()))
    unlisteners.push(await listen('menu:import_csv', () => importDialog.open()))
    // ...
  })

  onUnmounted(() => {
    unlisteners.forEach(fn => fn())
  })
}
```

Call `useNativeMenu()` once in the root component. All menu events route through it. The composable becomes the inventory of "things the native menu can trigger" — a single file you can audit when adding a new menu item.

## The discoverability dividend

A subtler benefit of doing this split correctly: users learn shortcuts from the menu bar. They're looking for "Export" in the File menu, see `⌘E` next to it, and now they know the shortcut. Frontend-bound shortcuts are invisible — users only learn them from documentation or from hitting them by accident. Over time, the menu-anchored shortcuts get used; the frontend-anchored ones get forgotten. This is why putting global shortcuts on the menu (rather than just in `useMagicKeys`) increases adoption of those shortcuts by users.

For an admin tool used by Excel-trained registrars, this matters more than for a developer tool used by power users. Your users will look at the menu. Make sure what they need to find is there.

## Edge cases worth flagging

**Cross-platform accelerator strings.** Use `CmdOrCtrl` (Tauri's portable token) rather than `Cmd` or `Ctrl` directly. Resolves to `Cmd` on macOS and `Ctrl` on Windows/Linux. The menu rendering shows the right symbol per platform.

**Conflicts with system shortcuts.** Avoid `CmdOrCtrl+H` (Hide on macOS), `CmdOrCtrl+M` (Minimize on macOS, conflicts with some Linux WMs), `CmdOrCtrl+W` (Close Window — usually you want this to work as expected, don't override). Standard menu items like `quit()`, `hide()`, `minimize()` get the right accelerators automatically; lean on them.

**Modal/dialog state.** When a modal is open, you usually want global shortcuts to be suppressed. Menu accelerators *don't* care about modal state — they fire regardless. Two options: disable menu items programmatically when a modal opens (Tauri 2 supports this via `MenuItemBuilder::enabled(false)` and runtime updates), or have menu event handlers check current app state and bail if a modal is open. The second is simpler. Add the check inside the `useNativeMenu` composable: if a modal is open, ignore most menu events except Quit and Help.

**Devtools shortcut in production builds.** Tauri ships with `Cmd+Option+I` / `F12` enabled in dev builds and disabled in release builds by default. If you add Toggle Devtools as a menu item, gate it on a build-time flag so it doesn't appear in shipped builds.
