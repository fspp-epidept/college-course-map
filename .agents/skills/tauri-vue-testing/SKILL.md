---
name: tauri-vue-testing
description: Use when writing or modifying tests for this Vue 3 + NuxtUI app embedded in a Tauri v2 webview. Frontend-isolated testing with Vitest, @tauri-apps/api/mocks, and tauri-specta-generated types. Covers IPC mocking, NuxtUI teleport/overlay testing, async IPC composables, Vitest browser mode for Reka UI, and determinism patterns. Layer this on top of antfu/skills vue-testing-best-practices.
version: 0.2.0
license: MIT
---

# Tauri + Vue + NuxtUI Testing

Project conventions for testing the Vue 3 frontend **in isolation from the Rust backend**. The frontend is fully testable without launching Tauri, building Rust, or running the real IPC bridge — every Tauri API call goes through `mockIPC`, and types come from `tauri-specta`-generated bindings so the contract stays honest as the Rust side evolves.

End-to-end testing (driving a real Tauri build) is a separate layer covered briefly at the end. It's optional and not required for normal development.

## Decision matrix

- **Pure UI logic, computed, formatters, validators** → Vitest unit, jsdom, no Tauri mocks
- **Components that render data from Tauri** → Vitest component, jsdom, `mockIPC` with types from `bindings.ts`
- **NuxtUI overlays — `UModal`, `UPopover`, `USlideover`, `UContextMenu`, `UTooltip`, `USelect`, `UCommandPalette`** → Vitest browser mode (Reka UI needs real layout). Or skip at the unit level entirely.
- **Composables that wrap Tauri IPC** → Vitest unit + `mockIPC` + `mockWindows` + a `withSetup` helper
- **Rust command logic in isolation** → `cargo test` in `src-tauri/` (separate concern; not in scope for this skill)

If a test's purpose is *"does the component behave when given data,"* it belongs in this skill's scope. If it's *"did the Rust↔JS exchange work end-to-end,"* defer to the optional E2E section — or don't write it yet.

## Type-safe IPC contract via tauri-specta

This project uses [`tauri-specta`](https://github.com/specta-rs/tauri-specta) v2 to auto-generate TypeScript bindings from `#[tauri::command]` functions and event types. The generated `src/bindings.ts` is the source of truth for IPC types. **Never hand-mirror command signatures** — import them.

### Generation hook

Wire binding export into a Rust test so it regenerates on every `cargo test`:

```rust
// src-tauri/src/lib.rs
#[cfg(test)]
mod export_bindings {
    use super::*;
    use tauri_specta::{Builder, collect_commands};

    #[test]
    fn export_typescript_bindings() {
        Builder::<tauri::Wry>::new()
            .commands(collect_commands![get_users, save_settings /* keep in sync */])
            .export(
                specta_typescript::Typescript::default(),
                "../src/bindings.ts",
            )
            .expect("Failed to export bindings");
    }
}
```

CI must fail if the committed `bindings.ts` differs from the regenerated version. Add this step after `cargo test`:

```sh
git diff --exit-code src/bindings.ts
```

If it's dirty, the contract drifted and someone forgot to regenerate. This catches it at PR review, not at runtime.

### Using generated types in app code and tests

App code calls the typed `commands` namespace, not raw `invoke`:

```ts
import { commands } from '@/bindings'
import type { User, Settings } from '@/bindings'

const users = await commands.getUsers()        // typed: User[]
await commands.saveSettings({ settings })      // typed args
```

Tests still mock at the `mockIPC` layer because `commands.*` calls dispatch through `invoke` under the hood — but the handler types come from `bindings.ts`.

### Typed mock helper

```ts
// tests/helpers/mockCommands.ts
import { mockIPC } from '@tauri-apps/api/mocks'
import type { User, Settings } from '@/bindings'

// Map snake_case Rust command names to typed handlers.
// Adding a #[tauri::command] on the Rust side regenerates bindings.ts;
// if a test mocks a command not in this map, TypeScript fails the build.
type CommandHandlers = {
  get_users:     () => User[] | Promise<User[]>
  save_settings: (args: { settings: Settings }) => void | Promise<void>
}

export function mockCommands(handlers: Partial<CommandHandlers>) {
  mockIPC((cmd, args) => {
    const handler = (handlers as Record<string, Function>)[cmd]
    if (!handler) throw new Error(`Unmocked Tauri command: ${cmd}`)
    return handler(args as never)
  })
}
```

When you add tauri-specta events, follow the same shape: import generated event types and wrap subscription in a composable that tests can mock at the composable layer.

## Tauri IPC mocking — global setup

Use `@tauri-apps/api/mocks`. **Never** `vi.mock('@tauri-apps/api/core')` directly — the official mocks intercept at the right layer and survive Tauri minor-version bumps.

```ts
// tests/setup.ts
import { afterEach, beforeEach } from 'vitest'
import { clearMocks, mockWindows } from '@tauri-apps/api/mocks'

beforeEach(() => {
  mockWindows('main') // must match window labels in tauri.conf.json
})

afterEach(() => {
  clearMocks()
})
```

```ts
// vitest.config.ts
export default defineConfig({
  test: {
    setupFiles: ['./tests/setup.ts'],
    environment: 'jsdom',
  },
})
```

For multi-window apps: `mockWindows('main', 'settings', 'about')`. The labels must match `windows[].label` in your Tauri config — window-aware code branches on these, and a mismatch makes tests pass while production fails.

### Per-test command mocking

```ts
import { mockCommands } from '../helpers/mockCommands'

it('renders users from get_users', async () => {
  mockCommands({
    get_users: () => [{ id: 1, name: 'Ada' }, { id: 2, name: 'Linus' }],
  })

  const wrapper = mount(UserList)
  await flushPromises()
  expect(wrapper.findAll('[data-testid="user"]')).toHaveLength(2)
})
```

### Tauri events — wrap, don't raw-mock

Subscribing via `listen()` goes through the IPC channel, but mocking the raw protocol is brittle. Wrap subscription in a composable, mock the composable:

```ts
// composables/useAppEvent.ts
export function useAppEvent<T>(event: string, cb: (payload: T) => void) {
  let unlisten: UnlistenFn | undefined
  onMounted(async () => { unlisten = await listen<T>(event, e => cb(e.payload)) })
  onBeforeUnmount(() => unlisten?.())
}
```

In tests, `vi.mock('@/composables/useAppEvent')` and trigger the registered callback synchronously. This keeps event-driven code testable without touching `__TAURI_INTERNALS__`.

## NuxtUI gotchas

### Teleport — overlays escape the wrapper

`UModal`, `USlideover`, `UPopover`, `UContextMenu`, `UTooltip`, `USelect`, and `UCommandPalette` all teleport to `document.body`. `wrapper.find()` will never see them.

```ts
const wrapper = mount(MyDialog, { attachTo: document.body })
await wrapper.find('[data-testid="open-modal"]').trigger('click')
await flushPromises()

// Query document.body, not wrapper:
const modal = document.body.querySelector('[role="dialog"]')
expect(modal).toBeTruthy()
expect(modal!.textContent).toContain('Confirm delete')
```

Always pass `attachTo: document.body` so cleanup happens, and query the body directly for teleported content.

### Reka UI needs real layout — use browser mode

Floating UI positioning, focus traps, and the `Presence` primitive depend on real `getBoundingClientRect`, focus events, and CSS animations. jsdom returns zeros, breaks focus management, and doesn't run transitions — menu/popover open and close hang.

```ts
// vitest.config.ts (separate Vitest workspace project)
test: {
  browser: {
    enabled: true,
    provider: 'playwright',
    instances: [{ browser: 'chromium' }],
  },
}
```

Browser mode uses Chromium, not WebKit/WebView2 — that's fine for catching Reka UI behavior. It's not trying to be a Tauri E2E substitute.

Run browser-mode tests as a separate Vitest workspace project so the fast jsdom suite stays fast on every save.

### Form validation

`UForm` uses Standard Schema. Test the schema in isolation — that's where the logic lives. One form-level integration test per failure mode (required, async, server error) is enough. Don't re-test schema rules through the form.

### Don't snapshot NuxtUI output

Reka UI generates IDs like `reka-popover-content-0` that increment per render. Snapshot diffs become noise. Test behavior and accessible roles, not HTML.

## Async IPC composables

Tauri commands are always async via the IPC channel. Composables need `flushPromises` and at least one `nextTick` for reactive effects to settle.

```ts
import { flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { withSetup } from '../helpers/withSetup'

it('useUsers loads on mount', async () => {
  mockCommands({ get_users: () => [{ id: 1, name: 'Ada' }] })

  const [{ users, loading }, app] = withSetup(() => useUsers())
  expect(loading.value).toBe(true)

  await flushPromises()
  await nextTick()

  expect(loading.value).toBe(false)
  expect(users.value).toHaveLength(1)
  app.unmount()
})
```

`withSetup` is the lifecycle-hook-aware wrapper from antfu's `vue-testing-best-practices`, reference `testing-composables-helper-wrapper`. Without it, `onMounted` doesn't fire and composables that load on mount never resolve.

## Determinism

Frontend tests should produce identical output across machines and runs.

**Time**: don't reach for `vi.useFakeTimers()` if any code path crosses to Rust — webview and Rust clocks behave differently and that gap will surface eventually. Inject time as a parameter or wrap it in a `useNow()` composable that tests can override.

**Randomness**: same pattern. Wrap `Math.random` and `crypto.randomUUID` in a `useRandom` / `useId` composable so tests can pin values. Vue's built-in `useId()` is already render-stable; prefer it over hand-rolled IDs.

**Viewport**: jsdom defaults to 1024×768. If responsive logic matters, set `window.innerWidth`/`innerHeight` explicitly in test setup so a CI machine with different defaults can't shift behavior.

## What NOT to do

- Don't `vi.mock('@tauri-apps/api/core')` — use `mockIPC`
- Don't hand-mirror Rust command signatures in TypeScript — let `tauri-specta` generate them
- Don't run NuxtUI overlay tests in jsdom — use browser mode or skip them at the unit level
- Don't stub `invoke` at component prop level — `mockIPC` intercepts at the right layer and covers `convertFileSrc`, event channels, and other surface area
- Don't snapshot-test NuxtUI/Reka UI rendered HTML — auto-generated IDs make diffs meaningless
- Don't put business logic in `#[tauri::command]` bodies — extract to pure Rust functions so `cargo test` covers them

## When you're ready for end-to-end testing (optional)

Frontend-isolated tests cover everything *except* "does the real Rust↔JS bridge work?" If/when that becomes important, two paths:

- [`tauri-pilot`](https://github.com/mpiton/tauri-pilot) — Linux/macOS, Tauri v2 only, designed for AI agents. Lightest-weight option; ships as a debug-only plugin with zero production overhead.
- [`tauri-driver`](https://v2.tauri.app/develop/tests/webdriver/) + WebdriverIO — official, all platforms including Windows, but heavier setup.

Three things to watch for whenever you do add E2E — none of these affect the Vitest tests above:

- **Capabilities**: Tauri v2 requires explicit permission grants per window in `src-tauri/capabilities/*.json`. A command can work in `tauri dev` (loose default capability) and fail in production (strict capability). Run E2E against the production capability at least sometimes.
- **Content Security Policy (CSP)**: set in `tauri.conf.json` at `app.security.csp`, restricts which origins the webview loads scripts/styles/images from (XSS mitigation). Dev usually has a relaxed CSP for Vite HMR; production should be strict (e.g., `default-src 'self'`). E2E against `tauri build --debug` catches CSP violations the dev server hides.
- **App data persistence**: Tauri's app data directory survives across runs. For deterministic E2E, gate a `reset_app_state` command behind `#[cfg(debug_assertions)]` and call it before each scenario.

## References

- antfu/skills `vue-testing-best-practices` — base Vue testing patterns this skill builds on
- [Tauri mocking docs](https://tauri.app/develop/tests/mocking/) — `mockIPC`, `mockWindows`, `clearMocks`
- [tauri-specta](https://github.com/specta-rs/tauri-specta) — type-safe IPC bindings
- [Vitest browser mode](https://vitest.dev/guide/browser/) — for Reka UI / NuxtUI overlays
- [tauri-pilot](https://github.com/mpiton/tauri-pilot) — optional E2E CLI