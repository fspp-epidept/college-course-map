# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Repository state

The Tauri 2 + Vue 3 + Vite + TS scaffold is in place; dev tooling (Biome, clippy/rustfmt with edition-2024 safe-Rust lints, `Taskfile.yaml`) is wired and green via `task check`. The only application code is still the default `greet` Hello-World command — treat it as a placeholder to replace, not a pattern to extend.

Sources of truth (in this order):

- **GitHub Project + Issues** — work-tracking primitive. See "Workflow" below.
- **This file (`CLAUDE.md`)** — durable conventions, ground rules, decisions that survive across sessions.
- **`docs/keybinds.md`** — the three-layer keyboard-shortcut model (OS global / Tauri menu accelerator / WebView), per-shortcut decision rule, the concrete shortcut table for this app, and the `useNativeMenu` bridging composable.

> **`docs/handoff.md` is being retired.** It's the original architecture spec and the source we used to seed the GitHub Project. Once project items are populated, treat it as historical reference only — never as the working backlog. Anything still load-bearing has been promoted into this file or into Project items.

## Workflow

Work is tracked in a repo-scoped GitHub Project with linked Issues. The Project is the working backlog; `CLAUDE.md` and `docs/keybinds.md` are the durable design layer. No other backlog exists.

**Rules:**

- **No branch without a Project item.** Every working branch in this repo must have at least one Project item attached (an issue, or a draft item being promoted to one). If you're about to start work and there is no appropriate item, **stop and ask the user** whether to create one before continuing. Don't infer that "it's small enough to skip" — even quick fixes need an item so the history of *why* something changed is captured somewhere persistent.
- **Tag every issue per `.github/labels.md`.** Pick one `area:*` and one `phase:*` label always; add a `type:*` (`decision` / `spike`) when applicable. Don't invent new labels ad-hoc — if an existing label doesn't fit, that's a signal to either re-examine the issue scope or add the label deliberately (update `.github/labels.md` and `tmp/seed-issues.sh`'s `mklbl` block in the same change).
- **Project not yet wired.** Permission to create org-owned Projects v2 in `fspp-epidept` is pending (see `tmp/project-seed.md` current-state note). Until then, the "no branch without a Project item" rule is **relaxed to "no branch without a GitHub issue."** Existing issues will be bulk-added to the Project once it exists.
- **New blocker, feature idea, exploration, or spike → file an issue + Project item.** When work surfaces a question that can't be answered in-flight (an unfamiliar library behavior, a missing decision, a performance unknown, an architectural fork worth investigating), capture it as a GitHub issue and add it to the Project with all relevant context: what triggered it, what's already known, what a successful resolution looks like. Don't leave these as TODO comments in code or hope the conversation will be remembered.
- **Issue content carries the context that used to live in `docs/handoff.md`.** When promoting a seed item to a real issue, copy in the relevant schema fragments, IPC contract details, code-path pointers, and ground rules. The issue is the working spec; the Project item is its tracking shell.
- **Closing an issue closes the loop.** When merging a PR that completes a Project item, make sure the issue is referenced via a closing keyword (`Closes #N`) so the Project automation moves the item to Done. If a single PR partially resolves multiple items, link them explicitly in the PR description.

**Where things land:**

- A *durable* convention or architectural decision → this file (`CLAUDE.md`).
- A *unit of work* (something to do) → Project item / GitHub issue.
- A *keyboard/menu* decision → `docs/keybinds.md`.
- A scratch note, draft text, or handoff blurb → `tmp/` (gitignored), not committed.

## What this app is

A native desktop app for university administrators to bulk-classify courses against CCM codes using the [annamp/classifying-courses-at-scale](https://huggingface.co/collections/annamp/classifying-courses-at-scale) RoBERTa models (2/4/6-digit). Replaces an existing Flask reference app (`davidjurgens/course-classifier-website`). Realistic working datasets are ~2M rows / 200 MB CSV — architecture must assume long-running, interruptible, resumable jobs.

> Naming note: the panel CSV (`data/validation.csv`) `inventory_cip_*` columns contain federal **CIP codes** (Classification of Instructional Programs). The annamp models output **CCM codes** — a distinct hierarchical 2/4/6-digit taxonomy. CIP and CCM overlap heavily at the broad 2-digit level but diverge at 4/6-digit. `validate.py`'s reported overlap rate is *not* model accuracy: it's a CIP/CCM agreement measure, and the descending rate at deeper levels reflects expected taxonomy divergence. The meaningful correctness check is parity (Rust ONNX == Python ONNX == annamp PyTorch). In code, `ccm_*` names refer to model-output identifiers; panel column names are preserved as-is.

A sample input file lives at `data/panel.csv` (~165 MB). Headers: `sub_pref,course,inventory_approval,inventory_course_title,inventory_credit_hours,inventory_level,Multiple Course?,year,school,academic_year,inventory_cip_six,inventory_cip_four,inventory_cip_two`.

## Locked-in stack

- **UI shell:** Tauri 2 (already scaffolded)
- **Frontend:** Vue 3 + Vite + TypeScript, Vue Router, **Nuxt UI v4** (bundles Reka UI primitives + Tailwind 4 + TanStack Table integration + admin Dashboard scaffold; works in plain Vue 3 via its Vite plugin), TanStack Query (Vue adapter) for IPC fetching, `@vueuse/core` for composables. PrimeVue is fallback-only; shadcn-vue and Naive UI were considered and rejected — don't relitigate.
- **Inference:** Rust + ONNX Runtime via `ort` crate + `tokenizers` + `hf-hub` + `ndarray`
- **IPC types:** [tauri-specta](https://github.com/specta-rs/tauri-specta) + `specta-typescript` — Rust `#[tauri::command]` handlers generate a typed `bindings.ts` consumed by the frontend instead of stringly-typed `invoke()` calls. **Wired (#58).** Commands are collected in `lib.rs::specta_builder`; `src/bindings.ts` is generated headlessly by the `export_bindings` test and committed. Regenerate after changing any command signature with `task gen:bindings` (runs the test, then Biome-formats the output). The file carries `// @ts-nocheck` and Biome lint is disabled for it. Frontend code imports `commands` / types from `src/bindings.ts`; don't call `invoke()` directly.
- **Storage:** DuckDB via the `duckdb` crate, **single store, no fallback.** Mixed write/read concurrency is validated by stress test (a Project item), not hedged against architecturally.
- **Models:** **bundled into the installer** (airgap delivery — no runtime network). Loaded at runtime from Tauri's `resource_dir`. A build-time model manifest (TOML, location TBD per #99) is the single source of truth for `{digit_level, hf_repo, revision, sha256, local_path}` — read by `scripts/models/` (writes), the build pipeline (#52, fetches + verifies + embeds), and the runtime app (#25 `models` table populated from the manifest at startup).

The Rust dependencies in `src-tauri/Cargo.toml` currently only contain Tauri scaffolding — `ort`, `tokenizers`, `duckdb`, `hf-hub`, `blake3`, `dirs`, `tokio`, `ndarray` will need to be added as features land.

## External references

- **Tauri 2 docs (LLM-friendly index):** https://v2.tauri.app/llms.txt — authoritative, up-to-date Tauri reference. Prefer this over training-data recall when answering Tauri questions, writing Tauri-related code, or writing Tauri config/commands.
- **NuxtUI docs (LLM-friendly index):** https://ui.nuxt.com/llms.txt - authoritative, up-to-date NuxtUI reference. Prefer this over training-data recall when answering NuxtUI questions or writing NuxtUI-related code.

## Commands

Primary command runner is **[Task](https://taskfile.dev)** (`go-task`). All build/dev/test/lint commands should be wrapped as Task tasks; invoke them as `task <name>` and add new ones to `Taskfile.yaml` rather than proliferating raw `pnpm` / `cargo` lines in docs and READMEs.

**Use `method: checksum` as the global default in `Taskfile.yaml`.** Default mtime-based caching produces spurious cache misses (and stale-cache hits) after `git checkout`, fresh clones, or any operation that bumps mtime without changing content. Checksum hashes the source files instead — slower per check, but correct.

```yaml
# Taskfile.yaml — set at the top level so every task inherits it
version: '3'
method: checksum
tasks:
  # ...
```

There is no `Taskfile.yaml` yet. Write one as soon as the first non-trivial command lands; don't let a "we'll wrap it later" period accumulate raw invocations across docs.

**Underlying tools** (referenced by Task tasks; useful to know for direct invocation when bypassing Task or debugging a task definition):

- **pnpm** — JS package manager; referenced by `tauri.conf.json` `beforeDevCommand` / `beforeBuildCommand`. Common raw forms: `pnpm install`, `pnpm dev` (Vite only, port 1420 strict), `pnpm build` (`vue-tsc --noEmit && vite build`), `pnpm tauri dev` (full app), `pnpm tauri build` (signed/notarized bundle).
- **cargo** — run from inside `src-tauri/`. `cargo check` (fast typecheck), `cargo build`, `cargo test` (no tests yet).

**Formatting / linting / typechecking** (all wrapped as Task tasks; see `Taskfile.yaml`):

- **Biome 2.x** handles JS/TS/JSON/CSS and the `<script>`+`<style>` blocks of Vue SFCs. Config in `biome.json`. Scope is intentionally narrow — `src/**` plus root web configs (`package.json`, `tsconfig*.json`, `vite.config.ts`). Biome does **not** lint Vue `<template>` blocks, so `noUnusedVariables`/`noUnusedImports` are disabled for `.vue` files; rely on `vue-tsc` (which understands templates) for unused-binding detection. Run via `task fmt:js` / `task lint:js` / `task check:js`.
- **rustfmt + clippy** in `src-tauri/`. Edition 2024, lints configured in `[lints]` table of `src-tauri/Cargo.toml`: `unsafe_code = deny`, `clippy::pedantic` as warn, plus selected `restriction` lints (`unwrap_used`, `expect_used`, `panic`, `indexing_slicing`, `dbg_macro`, `todo`, `unimplemented`) to enforce Result-based error handling. Toolchain pinned via `rust-toolchain.toml`. Run via `task fmt:rust` / `task lint:rust` / `task check:rust`.
- **`task check`** runs the whole pipeline (fmt:check + lint + typecheck both sides) and is what CI should call.
- When suppression of a configured clippy lint is genuinely needed, prefer `#[expect(lint, reason = "...")]` over `#[allow]` — `expect` errors back out when the suppression is no longer needed, which keeps allows from accumulating. Example: top-level `tauri::Builder::run()` panics on startup failure (canonical Tauri pattern), so it carries `#[expect(clippy::expect_used, reason = "...")]`.

No test runner is wired up yet (`cargo test` runs but there are no tests). When one is added (Vitest, etc.), wrap it as a Task task and reference it here.

## Architectural ground rules

These are decisions to respect, not re-litigate:

- **Keep Rust's IPC surface narrow.** Rust handles ONNX inference, tokenization, DuckDB I/O, HF Hub downloads, CSV streaming, file I/O, hashing. Everything else (UI state, small-set filtering, chart config, form validation) lives in TypeScript. Every `#[tauri::command]` is a contract you must maintain.
- **CSV import split:** the column-mapping configurator UI lives in Vue; all parsing/hashing/validation/ingestion happens in Rust. Frontend never reads the file directly. Three commands: `preview_csv` (headers + ~5 rows), `validate_import` (full-file dry run, no writes), `import_csv` (full ingestion, persists mapping on `source_files`).
- **Never ship 2M rows across the IPC boundary.** Tauri commands return slices. TanStack Table runs in server-side mode. Dashboard aggregations execute in DuckDB and return summary rows. Exports stream from DuckDB to disk; frontend gets a path.
- **Cache by `(model_id, content_hash)`, not by run.** `inference_results` is a global cache keyed by inference configuration; it is intentionally *not* foreign-keyed to courses, datasets, or runs. Classifications survive dataset changes/deletions and reuse across runs by construction. This is what makes the planned Phase 2 cross-dataset matching trivial.
- **Model input format matters.** The annamp models expect `"{SUBJECT CODE} {CATALOG NUMBER} --- {COURSE TITLE}"`. The reference Flask app gets this wrong. Don't make the frontend responsible for assembling this — Rust assembles the model input from structured fields, and that assembled string is what gets hashed into `content_hash`.
- **Write-batching during inference:** flush ~1000 results or 30 seconds, whichever comes first. Run progress fields are updated in the same transaction as the cache insert, so crash recovery is consistent by construction.
- **Separate read-write and read-only DuckDB connections.** Inference pipeline holds the read-write conn; dashboard queries open their own read-only conns. Periodic `CHECKPOINT` during long runs.
- **Schema:** `source_files`, `datasets` (with `source_kind`, `parent_dataset_id`, `filter_spec`, `supersedes_id`), `courses` (no uniqueness on `(dataset_id, content_hash)` — legitimate duplicates are preserved), `models` (normalized, surrogate key referenced by cache), `runs` (lifecycle states: `pending|running|paused|completed|failed|interrupted|cancelled`), `inference_results` (PK `(model_id, content_hash)`). Until the migration runner lands, the working DDL reference is `docs/handoff.md` "Schema" section (retired doc; promote into a migration file when implementing Phase 3).
- **App chrome is hybrid: custom on Windows/Linux, native on macOS** (decision #102, reversing the earlier "native chrome everywhere" rule). On **Windows/Linux** the window is frameless (`decorations: false`) with a custom in-WebView titlebar (Vue + Nuxt UI) supplying the title, application menu, and window controls — the native GTK/Win32 menu can't be themed to match the app, so it's replaced. On **macOS** the window keeps native decorations + the native global menu bar built via `tauri::menu::MenuBuilder` (`menu.rs`, gated `#[cfg(target_os = "macos")]`), since the global bar is the platform convention. Native menu clicks fire `menu:<id>` Tauri events; the custom menu converges on the same handlers (`useNativeMenu`, see `docs/keybinds.md`). The Nuxt UI `DashboardSidebar` is in-app navigation; the menu (native or custom) is *additional*, not a replacement.
- **Keyboard shortcuts: see `docs/keybinds.md`.** Three layers (OS global / Tauri menu accelerator / WebView composables); never duplicate a binding across layers. Layer split is now platform-dependent per the hybrid-chrome decision (#102): on **macOS**, accelerators live on the native menu items (Layer 2); on **Windows/Linux**, there is no native menu, so those same shortcuts are bound at the **WebView layer (Layer 3)** alongside the custom menu. Reserve Layer 3 everywhere for component-scoped behavior (`Esc`, `↑/↓` in dropdowns, `/` to focus search).
- **Config/data/cache live under a `college-course-map` product dir** (decision #106), resolved via the platform path crate (`dirs::config_dir()` etc.), **not** Tauri's identifier-based `app_config_dir`. This is the universal convention for config, working data, and cache — `<config>/college-course-map/`, `<data>/college-course-map/`, etc. (The bundled models are the exception: they load from Tauri's `resource_dir`.)
- **Theming is runtime, CSS-var-token driven** (decision #106). A theme is a token map of Nuxt UI `--ui-*` custom properties + a font + `colorScheme` (`light|dark`), applied to `<html>` via inert `element.style.setProperty` (never `<style>` injection). Rust owns all theme/settings file I/O (`config.rs`): `settings.json` references the active theme by id; user themes are `<config>/college-course-map/themes/*.json`. Built-in themes ship in `src/theme/builtins/` (one per file) and are the always-safe fallback — `default-light` loads if a setting is missing/corrupt or a theme can't be resolved. Theme files are untrusted: the typed structs use `#[serde(deny_unknown_fields)]` (the type is the `--ui-*` allowlist) plus per-value checks. Frontend state is the `useTheme` composable singleton (no Pinia); `colorScheme` toggles the `.dark` class directly (no `@vueuse/core` — Nuxt UI's `useColorMode` is an inert stub in plain-Vue mode). `bootstrapTheme()` applies the active theme before `app.mount()` to avoid FOUC. Deferred follow-ups: theme picker UI (#108), config file-watching (#109), a11y/system auto-mode (#110), schemars JSON Schema, font bundling, `<UTheme :props>`.
- **UI shell is VS Code-shaped: workbench** (#112). Three columns, top to bottom: `AppTitleBar` (Win/Linux), then a row of `ActivityBar` (fixed 48px, never collapses) + `PrimarySidebar` (resizable, content swaps per active activity) + `MainPanel`. Activities are the top-level sections (currently Overview / Datasets / Runs / Models / Settings) defined declaratively in `src/config/activities.ts` — add a new activity by adding a row. Each activity has a `kind: "tabbed" | "fixed"`. Tabbed activities (Datasets, Runs) render a tab strip + the active tab's body via the `tabKindPanels` registry; fixed activities render a single `panel` component. **Switching activities preserves the inactive activity's tabs in the workspace store, not in DOM** — Pinia state survives unmount. Workspace state (active activity, tabs by activity, active tab by activity) lives in the `workspace` Pinia store (`src/stores/workspace.ts`) and is persisted to `localStorage` via `pinia-plugin-persistedstate`; sidebar width/collapse is persisted separately by `UDashboardGroup` under its own `dashboard` key. Cmd/Ctrl-K opens a `UDashboardSearch` palette over activities + open tabs (`workbench/CommandPalette.vue`). vue-router is installed but **routes are intentionally empty** — the workbench is store-driven; deep-linking can be re-engaged later without re-plumbing.

## Security baseline

This is a local-only desktop app, so most web threats (auth, CSRF, network hardening) don't apply. The relevant attack surface is **untrusted CSVs** and the **model supply chain**. Keep this list short by handling each item once, in the right layer.

- **Treat every CSV as hostile input.** Bound field size and column count during parsing. Never use a CSV value as a filesystem path. Validate column indexes and literal values from the mapping spec against the file's actual structure.
- **CSV export must escape formula injection.** Prefix-escape any cell starting with `=`, `+`, `-`, `@`, tab, or CR (OWASP "CSV injection") so admins opening exports in Excel don't get formulas executed.
- **Never use `v-html` with model output, course data, or anything else from the DB.** Vue's `{{ }}` / `:attr` auto-escape — rely on that. Validate URL schemes (`http`, `https` only — block `javascript:` and `data:`) before binding to `href`/`src`.
- **Parameterized SQL only.** Use `?` placeholders via the `duckdb` crate. Identifiers (column names, table names) must come from a hardcoded allowlist, never from user input or the mapping spec.
- **Pin model revisions on HF Hub.** Always download by commit hash, not `main`. Verify file hashes when the API returns them. A tampered ONNX graph is effectively RCE through ONNX Runtime — don't accept "latest."
- **Keep the Tauri capabilities file minimal.** `src-tauri/capabilities/default.json` currently allows only `core:default` + `opener:default` — keep it that tight. Use scoped FS permissions (read on user-selected paths via the dialog plugin, read-write only on the app data dir). Don't enable `shell:allow-execute`.
- **Set a real CSP before first release.** `tauri.conf.json` currently has `"csp": null`. Reasonable starting policy: `default-src 'self'; img-src 'self' data: https://huggingface.co; connect-src 'self' https://huggingface.co; style-src 'self' 'unsafe-inline'` (Tailwind needs inline styles).
- **Code signing & notarization** is a hard requirement for distribution to non-technical users — tracked as a Project item; don't ship release binaries unsigned.

Skip preemptively: auth, rate limiting, secrets management, IPC fuzzing (serde-typed `invoke_handler` already enforces shape).

## Style preferences for this repo

User's global rules (from `~/.claude/CLAUDE.md`) apply: simple over complex, no fallbacks, surgical changes, fix root causes. Specifically relevant here:

- No emojis in code, READMEs, or commit messages unless asked. Functional indicators (✓, ✗, ⚠) acceptable in UI when needed.
- No `Co-Authored-By: Claude …` trailer on commits in this repo (org policy; history was rewritten on 2026-05-14 to enforce). Plain commit messages, author identity only.
- Scratch / handoff / draft files go in `tmp/` at the repo root (already in `.gitignore`). Don't commit them.

Open decisions tracked as Project items (do not solve preemptively): quantization, model-bundling-vs-runtime-fetch, HF namespace for converted ONNX repos, code-signing certs, versioning/release tooling. See the Project for status.
