# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Repository state

The core vertical slice works end-to-end: import a CSV → classify via local ONNX → browse paginated results, with content-hash-keyed caching, live progress, and a migration-backed DuckDB schema. Rust↔Python inference parity is verified at 100% on fixtures. Dev tooling (Biome, clippy/rustfmt with edition-2024 safe-Rust lints, `Taskfile.yaml`) is wired and green via `task check`. The live backlog of what remains lives in the external issue tracker — see "Workflow" below; don't infer project status from this paragraph.

Sources of truth (in this order):

- **The issue tracker** — work-tracking primitive (the working backlog). See "Workflow" below; the maintainer's tracker setup lives in `CLAUDE.local.md` (untracked).
- **The maintainer's knowledge base** — durable cross-cutting knowledge: ADRs, design decisions, context (setup in `CLAUDE.local.md`).
- **This file (`CLAUDE.md`)** — durable repo-scoped conventions, ground rules, and decisions that survive across sessions.
- **`docs/keybinds.md`** — the three-layer keyboard-shortcut model (OS global / Tauri menu accelerator / WebView), per-shortcut decision rule, the concrete shortcut table for this app, and the `useNativeMenu` bridging composable.

## Workflow

Work is tracked in an external issue tracker; the maintainer-specific setup (which tracker, CLI, team, projects) lives in `CLAUDE.local.md`, which is untracked — read it before filing or querying issues. The tracker is the working backlog; `CLAUDE.md` and `docs/keybinds.md` are the durable design layer. The GitHub issue tracker is **not** the backlog — it was zeroed on 2026-06-10 when work moved to the external tracker.

Cross-cutting knowledge and decisions (ADRs, design context) live in the maintainer's knowledge base, also described in `CLAUDE.local.md`.

**Rules:**

- **No branch without a tracked issue.** Every working branch must map to at least one issue. If you're about to start work and there's no appropriate issue, **stop and ask the user** whether to create one before continuing. Don't infer that "it's small enough to skip" — even quick fixes need an issue so the history of *why* something changed is captured somewhere persistent.
- **File issues into the right project.** New work attaches to one of the existing tracker projects; pick the one that fits (don't spawn ad-hoc projects). Set priority/state in the tracker — it owns task state.
- **New blocker, feature idea, exploration, or spike → file an issue** with full context: what triggered it, what's already known, what a successful resolution looks like. Don't leave these as TODO comments in code or hope the conversation will be remembered.
- **Issue content is the working spec.** When fleshing out an issue, copy in the relevant schema fragments, IPC contract details, code-path pointers, and ground rules so it stands alone.
- **Closing the loop.** Reference the issue id in the branch name and PR body — not the PR title, since titles become public changelog entries. The tracker's GitHub integration links from those references, but it does **not** reliably auto-transition issue state on merge; flip the state manually after merging.

**Releases & commit convention:**

- `main` is protected: PR-only, **squash-merge only**. The squash commit's subject is the PR title, so **PR titles must be Conventional Commits** (`feat:`, `fix:`, `chore:`, `refactor:`, `docs:`, `test:`, `ci:`, `perf:`) — enforced by the `pr-title.yml` workflow. Local commit messages on branches are squashed away; no commit-msg hook exists on purpose.
- **release-please** (`release.yml`, on push to main) maintains a rolling release PR from those titles. Merging it tags `vX.Y.Z`, publishes the GitHub Release, and bumps `package.json` + `src-tauri/tauri.conf.json`. **`src-tauri/Cargo.toml`'s version is intentionally not release-managed** — the crate is never published and `tauri.conf.json` is what the bundler stamps on installers; leaving Cargo.toml static avoids Cargo.lock churn in release PRs.
- Pre-1.0 versioning: `feat` bumps minor, `fix`/others bump patch, breaking changes bump minor (`bump-minor-pre-major`).
- `src/bindings.ts` is committed **exactly as tauri-specta emits it** — Biome neither lints nor formats it — so `cargo test` regenerating it never dirties the tree.

**Where things land:**

- A *durable* repo-scoped convention or implementation ground rule → this file (`CLAUDE.md`).
- A *cross-cutting decision / ADR / context* (the "why", spanning sessions or repos) → the maintainer's knowledge base (see `CLAUDE.local.md`).
- A *unit of work* (something to do) → an issue in the tracker, in the right project.
- A *keyboard/menu* decision → `docs/keybinds.md`.
- A scratch note, draft text, or handoff blurb → `tmp/` (gitignored), not committed.

## What this app is

A native desktop app for university administrators to bulk-classify courses against CCM codes using the [annamp/classifying-courses-at-scale](https://huggingface.co/collections/annamp/classifying-courses-at-scale) models (2/4/6-digit). Replaces an earlier Flask-based reference implementation. Realistic working datasets are ~2M rows / 200 MB CSV — architecture must assume long-running, interruptible, resumable jobs.

> Naming note: the panel CSV (`data/validation.csv`) `inventory_cip_*` columns contain federal **CIP codes** (Classification of Instructional Programs). The annamp models output **CCM codes** — a distinct hierarchical 2/4/6-digit taxonomy. CIP and CCM overlap heavily at the broad 2-digit level but diverge at 4/6-digit. `validate.py`'s reported overlap rate is *not* model accuracy: it's a CIP/CCM agreement measure, and the descending rate at deeper levels reflects expected taxonomy divergence. The meaningful correctness check is parity (Rust ONNX == Python ONNX == annamp PyTorch). In code, `ccm_*` names refer to model-output identifiers; panel column names are preserved as-is.

Tracked sample inputs live in `samples/`. Large dev datasets — including the labeled panel CSV (`data/validation.csv`) that `scripts/models/validate.py` reads — are gitignored under `data/`. Panel headers: `sub_pref,course,inventory_approval,inventory_course_title,inventory_credit_hours,inventory_level,Multiple Course?,year,school,academic_year,inventory_cip_six,inventory_cip_four,inventory_cip_two`.

## Locked-in stack

- **UI shell:** Tauri 2 (already scaffolded)
- **Frontend:** Vue 3 + Vite + TypeScript, Vue Router, **Nuxt UI v4** (bundles Reka UI primitives + Tailwind 4 + TanStack Table integration + admin Dashboard scaffold; works in plain Vue 3 via its Vite plugin), TanStack Query (Vue adapter) for IPC fetching, `@vueuse/core` for composables. PrimeVue is fallback-only; shadcn-vue and Naive UI were considered and rejected — don't relitigate.
- **Inference:** Rust + ONNX Runtime via `ort` crate + `tokenizers` + `hf-hub` + `ndarray`. **ONNX Runtime is `load-dynamic` (decision 2026-07-29): nothing links at build time.** Every build ships the CPU *runtime pack* as a bundle resource; GPU packs (CUDA 12 + TensorRT on Win/Linux) are downloaded in-app. Packs are repackaged **official microsoft/onnxruntime release archives** pinned in `src-tauri/runtimes.toml` (version + sha256; the version must match what the `ort` crate targets — an `ort` bump re-pins every pack in the same PR, never compile ONNX Runtime from source). `runtime.rs` resolves a pack from the settings EP priority list and `ort::init_from`s it **once per process** — pack switches need a relaunch; EP-priority reorders only need `reload_models`. Per-session EP registration lives in `inference.rs::register_eps`; the winner is recorded on `runs.execution_provider` and shown in Settings→Inference. **Never attempt an EP the loaded pack doesn't claim** (decision 2026-08-25): the settings priority is filtered through `RuntimeState::registrable` before every load, and an EP whose registration fails once is never retried in that process (`inference::FailedEps`) — `ort` rc.12 turns a failed-then-retried DirectML registration into a null-pointer crash, so pack `eps` metadata is a registration precondition, not display data. `task runtimes:fetch` pulls the dev/CI CPU pack (gitignored `src-tauri/runtimes/`); `task check:runtime` reports what a machine resolves to. Parity stays CPU-only by decision; the shared results cache is deliberately EP-agnostic (key `(model_id, content_hash)`).
- **IPC types:** [tauri-specta](https://github.com/specta-rs/tauri-specta) + `specta-typescript` — Rust `#[tauri::command]` handlers generate a typed `bindings.ts` consumed by the frontend instead of stringly-typed `invoke()` calls. **Wired.** Commands are collected in `lib.rs::specta_builder`; `src/bindings.ts` is generated headlessly by the `export_bindings` test and committed. Regenerate after changing any command signature with `task gen:bindings` (runs the test, then Biome-formats the output). The file carries `// @ts-nocheck` and Biome lint is disabled for it. Frontend code imports `commands` / types from `src/bindings.ts`; don't call `invoke()` directly.
- **Storage:** DuckDB via the `duckdb` crate, **single store, no fallback.** Mixed write/read concurrency is validated by stress test (tracked in the backlog), not hedged against architecturally.
- **Models:** the app-active family is **ModernBERT** (decision 2026-07-03; parity-gated by `task check:parity`). **`src-tauri/models.toml` is the build-time manifest** — the SSOT for `{digit_level, app_subdir, hf_repo, revision (full SHA), per-file sha256+size}`, generated by `task models:manifest` from the published `robotastronaut/*-onnx` HF repos and embedded via `include_str!` (`manifest.rs`). At startup the manifest upserts `models` table rows and every digit-level → model-id lookup resolves through the `ModelCatalog` state — never by SQL guessing. Two flavors: **connected** (default; first-run in-app download via `download_models` — Rust-side reqwest with streaming sha256 verify, so no CSP change — into `<data>/college-course-map/models/`) and **airgap** (cargo feature; models bundled via `tauri.airgap.conf.json` into `resource_dir`). **Model loading is async**: the app boots model-less, `ModelStore` fills from a background autoload when files are present, and commands error cleanly until then. The loader reads `pad_token_id` from each model's `config.json` — never hardcode family-specific tokenizer params. **The published ModernBERT graphs carry an export pass (decision 2026-08-26): every fp32 `Neg` is rewritten to `Mul(x, -1.0)`** (`scripts/models/_lib/neg_rewrite.py`, stamped `metadata_props` `coreml_neg_rewrite=1`) because ORT's CoreML EP has no `Neg` builder and the rotary `Neg`s split every layer into CoreML/CPU partitions; the pass is bit-exact by construction and proves it per run against the pre-rewrite graph, so parity is unaffected.

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

**Underlying tools** (referenced by Task tasks; useful to know for direct invocation when bypassing Task or debugging a task definition):

- **pnpm** — JS package manager; referenced by `tauri.conf.json` `beforeDevCommand` / `beforeBuildCommand`. Common raw forms: `pnpm install`, `pnpm dev` (Vite only, port 1420 strict), `pnpm build` (`vue-tsc --noEmit && vite build`), `pnpm tauri dev` (full app), `pnpm tauri build` (signed/notarized bundle).
- **cargo** — run from inside `src-tauri/`. `cargo check` (fast typecheck), `cargo build`, `cargo test` (bindings export, parity, and crash-recovery harnesses).

**Formatting / linting / typechecking** (all wrapped as Task tasks; see `Taskfile.yaml`):

- **Biome 2.x** handles JS/TS/JSON/CSS and the `<script>`+`<style>` blocks of Vue SFCs. Config in `biome.json`. Scope is intentionally narrow — `src/**` plus root web configs (`package.json`, `tsconfig*.json`, `vite.config.ts`). Biome does **not** lint Vue `<template>` blocks, so `noUnusedVariables`/`noUnusedImports` are disabled for `.vue` files; rely on `vue-tsc` (which understands templates) for unused-binding detection. Run via `task fmt:js` / `task lint:js` / `task check:js`.
- **rustfmt + clippy** in `src-tauri/`. Edition 2024, lints configured in `[lints]` table of `src-tauri/Cargo.toml`: `unsafe_code = deny`, `clippy::pedantic` as warn, plus selected `restriction` lints (`unwrap_used`, `expect_used`, `panic`, `indexing_slicing`, `dbg_macro`, `todo`, `unimplemented`) to enforce Result-based error handling. Toolchain pinned via `rust-toolchain.toml`. Run via `task fmt:rust` / `task lint:rust` / `task check:rust`.
- **`task check`** runs the whole pipeline (fmt:check + lint + typecheck both sides) and is what CI should call.
- When suppression of a configured clippy lint is genuinely needed, prefer `#[expect(lint, reason = "...")]` over `#[allow]` — `expect` errors back out when the suppression is no longer needed, which keeps allows from accumulating. Example: top-level `tauri::Builder::run()` panics on startup failure (canonical Tauri pattern), so it carries `#[expect(clippy::expect_used, reason = "...")]`.

Rust tests run through `cargo test` (wrapped by the `check:*` tasks). No JS test runner is wired up yet; when one is added (Vitest, etc.), wrap it as a Task task and reference it here.

## Architectural ground rules

These are decisions to respect, not re-litigate:

- **Keep Rust's IPC surface narrow.** Rust handles ONNX inference, tokenization, DuckDB I/O, HF Hub downloads, CSV streaming, file I/O, hashing. Everything else (UI state, small-set filtering, chart config, form validation) lives in TypeScript. Every `#[tauri::command]` is a contract you must maintain.
- **CSV import split:** the column-mapping configurator UI lives in Vue; all parsing/hashing/validation/ingestion happens in Rust. Frontend never reads the file directly. Three commands: `preview_csv` (headers + ~5 rows), `validate_import` (full-file dry run, no writes), `import_csv` (full ingestion, persists mapping on `source_files`).
- **Never ship 2M rows across the IPC boundary.** Tauri commands return slices. TanStack Table runs in server-side mode. Dashboard aggregations execute in DuckDB and return summary rows. Exports stream from DuckDB to disk; frontend gets a path.
- **Cache by `(model_id, content_hash)`, not by run.** `inference_results` is a global cache keyed by inference configuration; it is intentionally *not* foreign-keyed to courses, datasets, or runs. Classifications survive dataset changes/deletions and reuse across runs by construction. This is what makes the planned Phase 2 cross-dataset matching trivial.
- **Model input format matters.** The annamp models expect `"{SUBJECT CODE} {CATALOG NUMBER} --- {COURSE TITLE}"`. The reference Flask app gets this wrong. Don't make the frontend responsible for assembling this — Rust assembles the model input from structured fields, and that assembled string is what gets hashed into `content_hash`.
- **Write-batching during inference:** one DB round-trip (cache check, Appender insert, progress UPDATE) per `FLUSH_SIZE = 1024`-row super-chunk (`runs.rs`) — deliberately decoupled from the ONNX batch size (`inference::batch_size`), which is a *throughput* constant tuned via `task check:throughput` and must not be re-coupled to flush cadence. Run progress fields are updated in the same transaction as the cache insert, so crash recovery is consistent by construction. Batch-size guideline (measured 2026-07-29): inputs are length-bucketed per super-chunk; bucketed batch 128 is the optimum on both CUDA and CPU — padding waste, not launch overhead, is what punishes larger batches. Re-measure before changing either constant.
- **One shared DuckDB instance, separate read-write and read connections.** The inference/import pipeline holds the read-write `Connection`; reads use a second `Connection` cloned from it (`try_clone`), guarded by its own `Mutex`. Both must come from the *same* instance: a separately-opened read-only instance (`open_with_flags`) is a point-in-time snapshot that never sees the RW instance's later commits, so polling reads (`list_datasets`, `get_run`) would freeze an in-progress import/run at zero. Cloned connections share MVCC and see committed writes immediately. The read handle is read-only by convention (only read commands take `AppDb::ro()`), not by access mode. Periodic `CHECKPOINT` during long runs.
- **Schema:** `source_files`, `datasets` (with `source_kind`, `parent_dataset_id`, `filter_spec`, `supersedes_id`), `courses` (no uniqueness on `(dataset_id, content_hash)` — legitimate duplicates are preserved), `models` (normalized, surrogate key referenced by cache), `runs` (lifecycle states: `pending|running|paused|completed|failed|interrupted|cancelled`), `inference_results` (PK `(model_id, content_hash)`). The migration runner has landed; the authoritative DDL is `src-tauri/migrations/*.sql`.
- **App chrome is hybrid: custom on Windows/Linux, native on macOS** (decision 2026-05-26, reversing the earlier "native chrome everywhere" rule). On **Windows/Linux** the window is frameless (`decorations: false`) with a custom in-WebView titlebar (Vue + Nuxt UI) supplying the title, application menu, and window controls — the native GTK/Win32 menu can't be themed to match the app, so it's replaced. On **macOS** the window keeps native decorations + the native global menu bar built via `tauri::menu::MenuBuilder` (`menu.rs`, gated `#[cfg(target_os = "macos")]`), since the global bar is the platform convention. Native menu clicks fire `menu:<id>` Tauri events; the custom menu converges on the same handlers (`useNativeMenu`, see `docs/keybinds.md`). The Nuxt UI `DashboardSidebar` is in-app navigation; the menu (native or custom) is *additional*, not a replacement.
- **Keyboard shortcuts: see `docs/keybinds.md`.** Three layers (OS global / Tauri menu accelerator / WebView composables); never duplicate a binding across layers. Layer split is now platform-dependent per the hybrid-chrome decision: on **macOS**, accelerators live on the native menu items (Layer 2); on **Windows/Linux**, there is no native menu, so those same shortcuts are bound at the **WebView layer (Layer 3)** alongside the custom menu. Reserve Layer 3 everywhere for component-scoped behavior (`Esc`, `↑/↓` in dropdowns, `/` to focus search).
- **Config/data/cache live under a `college-course-map` product dir** (decision 2026-05-26), resolved via the platform path crate (`dirs::config_dir()` etc.), **not** Tauri's identifier-based `app_config_dir`. This is the universal convention for config, working data, and cache — `<config>/college-course-map/`, `<data>/college-course-map/`, etc. (The bundled models are the exception: they load from Tauri's `resource_dir`.)
- **Theming is runtime, CSS-var-token driven** (decision 2026-05-26). A theme is a token map of Nuxt UI `--ui-*` custom properties + a font + `colorScheme` (`light|dark`), applied to `<html>` via inert `element.style.setProperty` (never `<style>` injection). Rust owns all theme/settings file I/O (`config.rs`): `settings.json` references the active theme by id; user themes are `<config>/college-course-map/themes/*.json`. Built-in themes ship in `src/theme/builtins/` (one per file) and are the always-safe fallback — `default-light` loads if a setting is missing/corrupt or a theme can't be resolved. Theme files are untrusted: the typed structs use `#[serde(deny_unknown_fields)]` (the type is the `--ui-*` allowlist) plus per-value checks. Frontend state is the `useTheme` composable singleton (no Pinia); `colorScheme` toggles the `.dark` class directly (no `@vueuse/core` — Nuxt UI's `useColorMode` is an inert stub in plain-Vue mode). `bootstrapTheme()` applies the active theme before `app.mount()` to avoid FOUC. Deferred follow-ups: theme picker UI, config file-watching, a11y/system auto-mode, schemars JSON Schema, font bundling, `<UTheme :props>`.
- **UI shell is a workbench with sidebar-driven master/detail** (tabs removed 2026-07-30; don't reintroduce them). Top to bottom: `AppTitleBar` (Win/Linux), then a flex row of `ActivityBar` (fixed 48px, never collapses) + `PrimarySidebar` (fixed width 16rem, hideable; resize handle is a polish follow-up) + `MainPanel`. Activities are the top-level sections (currently Overview / Datasets / Runs / Models / Settings) defined declaratively in `src/config/activities.ts` — add a new activity by adding a row with a `sidebar` + `panel` component. Every activity renders exactly one panel; Datasets and Runs are master/detail — the sidebar owns selection (`selectedDatasetId` / `selectedRunId` in the workspace store) and their panels render the selected resource's detail (`DatasetDetail` / `RunDetail`, keyed by resource id so per-resource local state can't bleed). **Selection survives activity switches in the store, not in DOM.** Workspace state lives in the `workspace` Pinia store (`src/stores/workspace.ts`), persisted to `localStorage` under the versioned key `workspace-v2` (the tabbed-era `workspace` key is deliberately orphaned). Sidebars prune a persisted selection whose backing row disappeared. Cmd/Ctrl-K opens a `UDashboardSearch` palette over activities + all datasets + all runs (`workbench/CommandPalette.vue`) — its `defineShortcuts` binding works standalone. vue-router is installed but **routes are intentionally empty** — the workbench is store-driven; deep-linking can be re-engaged later without re-plumbing. **Don't reintroduce `UDashboardGroup` or `UDashboardSidebar`**: they're built for full-viewport web dashboards — `UDashboardGroup` is `fixed inset-0` (covers our titlebar + activity bar), `UDashboardSidebar` is `hidden lg:flex` (collapses under 1024px) and always mounts a mobile slideover. Use plain `<aside>`s; `UDashboardSearch` is the only Dashboard primitive we keep.

## Security baseline

This is a local-only desktop app, so most web threats (auth, CSRF, network hardening) don't apply. The relevant attack surface is **untrusted CSVs** and the **model supply chain**. Keep this list short by handling each item once, in the right layer.

- **Treat every CSV as hostile input.** Bound field size and column count during parsing. Never use a CSV value as a filesystem path. Validate column indexes and literal values from the mapping spec against the file's actual structure.
- **CSV export must escape formula injection.** Prefix-escape any cell starting with `=`, `+`, `-`, `@`, tab, or CR (OWASP "CSV injection") so admins opening exports in Excel don't get formulas executed.
- **Never use `v-html` with model output, course data, or anything else from the DB.** Vue's `{{ }}` / `:attr` auto-escape — rely on that. Validate URL schemes (`http`, `https` only — block `javascript:` and `data:`) before binding to `href`/`src`.
- **Parameterized SQL only.** Use `?` placeholders via the `duckdb` crate. Identifiers (column names, table names) must come from a hardcoded allowlist, never from user input or the mapping spec.
- **Pin model revisions on HF Hub.** Always download by commit hash, not `main`. Verify file hashes when the API returns them. A tampered ONNX graph is effectively RCE through ONNX Runtime — don't accept "latest."
- **Keep the Tauri capabilities file minimal.** `src-tauri/capabilities/default.json` currently allows only `core:default` + `opener:default` — keep it that tight. Use scoped FS permissions (read on user-selected paths via the dialog plugin, read-write only on the app data dir). Don't enable `shell:allow-execute`.
- **Set a real CSP before first release.** `tauri.conf.json` currently has `"csp": null`. Reasonable starting policy: `default-src 'self'; img-src 'self' data: https://huggingface.co; connect-src 'self' https://huggingface.co; style-src 'self' 'unsafe-inline'` (Tailwind needs inline styles).
- **Code signing & notarization** is a hard requirement for distribution to non-technical users — tracked in the backlog; don't ship release binaries unsigned.

Skip preemptively: auth, rate limiting, secrets management, IPC fuzzing (serde-typed `invoke_handler` already enforces shape).

## Style preferences for this repo

User's global rules (from `~/.claude/CLAUDE.md`) apply: simple over complex, no fallbacks, surgical changes, fix root causes. Specifically relevant here:

- No emojis in code, READMEs, or commit messages unless asked. Functional indicators (✓, ✗, ⚠) acceptable in UI when needed.
- No `Co-Authored-By: Claude …` trailer on commits in this repo (org policy; history was rewritten on 2026-05-14 to enforce). Plain commit messages, author identity only.
- Scratch / handoff / draft files go in `tmp/` at the repo root (already in `.gitignore`). Don't commit them.

Open decisions are tracked in the issue tracker (do not solve them preemptively); see `CLAUDE.local.md` for how to query them.

Closed decision (2026-07-29): **no quantization, ever, without revisiting parity.** The app optimizes around the raw published models — research users need unmodified model outputs, and correctness is exact-match parity against the Python reference, which quantization breaks. Optimize batching, padding, and pipeline shape instead.
