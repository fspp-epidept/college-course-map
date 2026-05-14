# Course Classifier Native App — Handoff

> **Retired.** This doc was the original architecture spec and the source we used to seed the repo's GitHub Project. Once Project items are populated, treat this file as **historical reference only**. The working backlog lives in the GitHub Project; durable conventions live in `CLAUDE.md`; keyboard/menu design lives in `docs/keybinds.md`. Don't add new content here — promote it into a Project item or `CLAUDE.md` instead.
>
> Specific updates since freeze: **DuckDB is the single store, no SQLite fallback.** The original concurrency-fork discussion below is preserved for the reasoning it contains but is no longer the active plan.

## Context

We're building a native desktop app that wraps the [annamp/classifying-courses-at-scale](https://huggingface.co/collections/annamp/classifying-courses-at-scale) RoBERTa-base models (2-digit, 4-digit, and 6-digit CCM classification). The reference implementation is a Flask web app at [davidjurgens/course-classifier-website](https://github.com/davidjurgens/course-classifier-website).

**Decisions locked in:**
- Native desktop app is the primary deliverable. Hosted web app is a secondary "try before you install" target. Container deployment falls out of the web app.
- Backend: Rust + ONNX Runtime (via the `ort` crate) for inference.
- UI shell: Tauri 2.
- Frontend: **Vue 3** + Vite + TypeScript, with **Nuxt UI** (which bundles Reka UI primitives + Tailwind 4 + TanStack Table integration), Vue Router, and TanStack Query (Vue adapter).
- Embedded database: **DuckDB** (via the `duckdb` crate) as single source of truth, no fallback. Speed-optimal for the mixed OLTP/OLAP workload at our scale; concurrency behavior under sustained mixed reads/writes is validated by a stress-test Project item rather than hedged against architecturally.
- Scale: realistic working datasets are **up to ~2M rows / 200MB CSV**. Architecture must assume long-running, interruptible, resumable jobs.
- Target user: non-technical administrators who currently use Excel for this work.

**Decisions still open:**
- Whether to ship F32 ONNX models, F16, or int8 (deferred — needs collaborator input).
- Whether to bundle models or fetch at runtime (leaning runtime fetch).
- Whether to coordinate with annamp on hosting ONNX-converted models, or publish under a separate account.

---

## Use case

This tool exists to help university curricula and admissions administrators handle a recurring problem: determining whether courses from other institutions qualify for credit at their own. The challenge:

- Course catalogs aren't one-to-one across institutions. "Intro to Programming" at School A might be CSCI 101; at School B it might be CIS 110, "Computational Thinking," or "Foundations of Computing."
- Administrators need to make these comparisons for inbound and outbound transfer students, often in bulk during admissions cycles.
- Authoring blanket transfer credit policies is hard without a systematic way to compare curricula across institutions.

The annamp RoBERTa classifiers solve a piece of this by mapping arbitrary course titles/descriptions to standardized CCM codes. Two courses that classify to the same 6-digit CCM are presumptively equivalent for credit purposes; matching at 4-digit indicates related fields; 2-digit indicates the same general area of study.

The tool's job is to make this classification accessible to administrators who currently rely on Excel and manual review, without requiring them to manage Python environments, GPU drivers, or cloud infrastructure. It's not a research tool — though the underlying models are an academic project — it's a workhorse for office workflows.

---

## Planned evolution

The tool will develop over multiple phases. This document is mostly Phase 1, but Phase 1 architecture decisions are explicitly forward-compatible with later phases.

**Phase 1 (current planning)**: Import CSV → classify → browse results → export CSV. Basic but functional. Replaces the manual workflow with a faster, model-assisted one. Datasets sidebar, paginated results table, simple dashboard, export to CSV. This is what the rest of this document covers.

**Phase 2: Cross-dataset course matching.** "I have this course; what courses across my other imported datasets share its CCM classification?" Naturally supported by the cache-keyed schema — classifications are globally addressable by `(model_id, content_hash)`, joinable across all imported data without restructuring. The UI would surface this as an "equivalent courses" view per course, ranked by match strictness (6-digit exact > 4-digit related > 2-digit general field), with the source dataset/institution shown for each match.

**Phase 3+**: Additional metadata (institutional accreditation, credit hour normalization, transfer agreement registries), course augmentation (description enrichment, embedding-based similarity for non-classification matching), workflow tooling (saved searches, batch decisions, audit trails for transfer credit decisions).

Phase 1 schema decisions already accommodate Phase 2: cache-keyed inference results, separated source files and datasets, normalized models table. No architectural rework needed to add cross-dataset matching — it's just queries over the existing schema. Phase 3 features will likely add tables (transfer_decisions, embeddings, etc.) but won't disrupt Phase 1 data.

---

## ⚠️ Code signing — solve before first release

This is the single most important non-technical thing to nail down early. Non-technical users will not click through Gatekeeper or SmartScreen warnings, and an unsigned binary triggers exactly that on macOS and Windows.

**Action item:** Check whether UMich has an institutional code-signing certificate available through central IT or LSA's research computing group. Many R1 universities do — they typically have:
- Apple Developer Enterprise or institutional Developer ID accounts
- Windows EV code-signing certificates (often through DigiCert or Sectigo)

If UMich doesn't have one, or won't make it available for an academic open-source project, the fallback costs are:
- **Apple Developer Program**: $99/year, gives you Developer ID for distributing outside the App Store. Requires Apple ID + verification.
- **Windows EV code signing certificate**: $200–400/year from DigiCert, Sectigo, or SSL.com. EV certs build SmartScreen reputation instantly; standard certs require accumulating reputation over many downloads, which is painful.
- **Linux**: no signing requirement, but you'll want to publish to Flathub or a `.deb`/`.rpm` repo for distribution legitimacy.

Tauri 2 has good built-in support for signing/notarization in its bundler config. The annoying part is acquiring the certificates, not using them.

**This affects timeline.** Apple notarization in particular adds 15–30 min to release builds and occasionally fails for opaque reasons. Plan to have a working signed build of a "Hello World" Tauri app well before you have anything to actually ship.

---

## Inference runtime: ONNX Runtime vs Candle

Quick answer: **ONNX Runtime via the `ort` crate** is the right call here. The "no additional runtime" framing is mostly accurate, with one caveat noted below.

### What each is

**ONNX Runtime** is Microsoft's cross-platform inference engine, written in C++. The `ort` crate is a Rust wrapper around it. You convert your PyTorch model to ONNX format once (via `optimum-cli export onnx`), then ONNX Runtime executes it. It's the most battle-tested inference runtime in the industry — same code powers Microsoft 365, Azure ML, Bing, etc.

**Candle** is a pure-Rust ML framework from Hugging Face. No C++ dependency — it's all Rust, and `candle-transformers` has direct support for RoBERTa. You load `.safetensors` weights directly with no format conversion step.

### The "no additional runtime" question

Both options ship as part of the app. Neither requires the user to install Python, Node, or a JVM. The distinction:

- **ONNX Runtime** ships a C++ shared library (~10–30 MB depending on platform and execution providers compiled in). The `ort` crate can either dynamically link against a system-provided `libonnxruntime` or statically bundle it. We'll bundle it. From the user's perspective: invisible, no install step.
- **Candle** is pure Rust, statically compiled into the main binary. Genuinely zero external dependencies.

So Candle is *more* "no runtime" than ONNX, but ONNX is still firmly in the "user doesn't have to install anything separately" category. The Python sidecar approach was the one we ruled out because it actually requires shipping a 200+ MB Python interpreter and dependency tree.

### Why ONNX over Candle here

Candle is appealing in principle but has practical risks:
- The tokenizer behavior must exactly match what the models were trained with (the HF Python `tokenizers` library). Both ONNX and Candle paths use the same `tokenizers` crate from HF, so this is actually neutral — but worth verifying with an end-to-end test against the Python reference.
- Edge cases in model config or unusual layer types occasionally aren't supported in `candle-transformers` and you find out at runtime. RoBERTa-base is well-supported, but "well-supported" isn't "guaranteed."
- ONNX Runtime has dramatically more polished GPU acceleration paths (CUDA, CoreML, DirectML, ROCm) if anyone ever wants to run on a beefier machine. Candle's CUDA support exists but is younger.
- ONNX as a format is portable. If somebody later wants to use the same models from C++, Python, or JavaScript (ONNX.js), they get that for free.

The downside of ONNX is the conversion step — it's an extra build artifact to manage. Worth it.

### Crate choices

```
ort = "2.0"           # ONNX Runtime bindings
tokenizers = "0.20"   # HF tokenizers, matches Python behavior bit-for-bit
hf-hub = "0.3"        # Download models from HF Hub (see model integration section)
ndarray = "0.16"      # Tensor manipulation, ort uses this
```

---

## UI shell: Tauri

### How Tauri actually works (and how it differs from Electron)

Tauri is **not** like Electron. The key architectural difference: **Tauri uses the OS-native webview, not a bundled Chromium**.

| Platform | Webview engine |
|----------|---------------|
| Windows  | WebView2 (Chromium-based, maintained by Microsoft, auto-installed on Win10+) |
| macOS    | WKWebView (Safari/WebKit) |
| Linux    | WebKitGTK (WebKit, but a different distribution than macOS) |

This is why Tauri apps are 5–15 MB instead of Electron's 80–150 MB minimum: you're not shipping Chromium. The tradeoff is cross-engine compat — you'll occasionally hit a WebKit-specific bug that doesn't repro in Chrome. In practice for an app of this complexity (forms, tables, charts), modern WebKit handles everything you'll need. But test on macOS regularly during development; don't only test in Chrome.

### How frontend assets are served

Your frontend builds to static HTML/JS/CSS via Vite (or whatever bundler your framework uses). Tauri embeds these into the binary at compile time. At runtime, Tauri serves them via a custom protocol (`tauri://localhost` on macOS/Linux, `https://tauri.localhost` on Windows) that the webview hits directly. No actual HTTP server — it's a synchronous in-process resolver. This is fast and means your dev experience uses Vite's dev server in development, but production loads from the bundle.

### Frontend ↔ Rust communication

Two main mechanisms:

**Commands** are Rust functions you mark with `#[tauri::command]` and call from the frontend via `invoke()`:

```rust
#[tauri::command]
async fn classify_courses(input: Vec<CourseInput>) -> Result<Vec<Classification>, String> {
    // run inference
}
```

```typescript
import { invoke } from '@tauri-apps/api/core';
const results = await invoke('classify_courses', { input: courses });
```

This is your main API surface. Type-safe-ish (you can use `tauri-specta` to auto-generate TS types from Rust signatures, which is highly recommended).

**Events** are for streaming data Rust → frontend, e.g., progress updates during a long classification job:

```rust
window.emit("classify-progress", ProgressPayload { current: 42, total: 100 })?;
```

```typescript
import { listen } from '@tauri-apps/api/event';
listen<ProgressPayload>('classify-progress', (event) => { /* ... */ });
```

This replaces the SSE pattern from the Flask reference and is much cleaner.

### Routing

Tauri itself doesn't provide routing — it just serves your static bundle to a webview. **You absolutely need a client-side router.** Options:
- **React**: TanStack Router (recommended given your TanStack familiarity), or React Router.
- **Vue**: Vue Router (the official one, very mature).
- **Svelte**: SvelteKit's built-in router with the static adapter.

For an app this size, the routing needs are simple — likely 4–6 routes (upload, processing, results dashboard, search, single-course classify, settings). Don't over-architect.

### Comparison to Electron, summarized

| | Tauri 2 | Electron |
|---|---------|----------|
| Bundle size | 5–15 MB | 80–150 MB |
| Memory footprint | Lower (uses OS webview) | Higher (full Chromium) |
| Backend language | Rust | Node.js |
| Webview consistency | Per-OS (WebKit/Chromium) | Bundled Chromium (consistent) |
| IPC ergonomics | Excellent (commands/events) | Good (ipcRenderer/ipcMain) |
| Security defaults | Stricter (allowlist, CSP) | More permissive |
| Mobile target | Yes (iOS/Android, Tauri 2) | No |

For a non-technical admin's laptop, the bundle size and memory difference matter — Electron apps are notorious for feeling heavy.

### Other Rust GUI options (and why not them, for this use case)

Briefly, since you asked earlier:
- **egui / eframe**: pure Rust, immediate-mode. Great for dev tools. Wrong aesthetic for Excel users — looks like an engineering app, weak table widgets, copy-paste behavior doesn't match user expectations.
- **Iced**: Elm-style architecture, nicer visuals than egui, but the data-grid and charting ecosystems are thin.
- **Slint**: declarative DSL, polished output. Licensing is the catch — free under GPL or via a royalty-free desktop license that you should read carefully. For an academic open-source project under MIT/Apache, friction.
- **Dioxus**: React-like in Rust, can target webview or native. Younger ecosystem than Tauri's, fewer mature components. Worth watching, not betting on yet.

Tauri wins for this project because the frontend-component ecosystem (tables, charts, file pickers) is where you'll spend most of your UI effort, and that ecosystem lives on the web platform.

### Native OS integration: menu bar, window chrome, accelerators

Tauri 2 lets you opt into platform-native UI elements that sit *outside* the WebView. Three pieces matter for this app:

**Native window chrome (title bar)**: every Tauri window has OS-rendered chrome by default — minimize/maximize/close buttons, draggable title bar, system menu. Behavior matches the host OS automatically (traffic-light buttons on macOS, minimize/maximize/X on Windows, GTK/Qt-style on Linux). You can disable it with `decorations: false` and build a custom HTML title bar (the Linear/Slack/Notion pattern), but for an admin tool the native chrome is what registrars expect, and you avoid the drag-region edge cases — resize handles, fullscreen on multi-monitor setups, hover states on Linux. Keep `decorations: true`.

**Native application menu**: this is the File / Edit / View / Window / Help bar. On macOS it appears as the global menu bar at the top of the screen. On Windows and Linux it attaches to the top of the window. Same Rust code, platform-correct rendering. Worth having for this audience — Excel itself has a menu bar, and standard items like Cut/Copy/Paste/SelectAll come pre-built with the platform-correct labels and accelerators wired up.

```rust
use tauri::menu::{MenuBuilder, SubmenuBuilder, MenuItemBuilder};

let import = MenuItemBuilder::new("Import CSV…")
    .id("import_csv")
    .accelerator("CmdOrCtrl+O")
    .build(app)?;

let file_menu = SubmenuBuilder::new(app, "File")
    .item(&import)
    .separator()
    .quit()
    .build()?;

let menu = MenuBuilder::new(app)
    .items(&[&file_menu /*, &edit_menu, &help_menu */])
    .build()?;

app.set_menu(menu)?;
app.on_menu_event(move |app, event| {
    if event.id() == import.id() {
        app.emit("menu:import_csv", ()).unwrap();
    }
});
```

Menu clicks fire as Rust events, which then emit Tauri events to the frontend. The frontend listens via `@tauri-apps/api/event`'s `listen()`. Keyboard accelerators are handled by the OS — they work even when the WebView doesn't have focus.

A useful starting menu structure for this app:

- **File**: Import CSV…, Export Results…, Open Recent… (submenu of recently imported files), Quit
- **Edit**: Cut / Copy / Paste / Select All (predefined items), Preferences (`Cmd+,` / `Ctrl+,`)
- **Run**: Start Classification, Pause Run, Resume Run, View Run History
- **View**: Toggle Sidebar, Toggle Devtools (dev-only)
- **Window**: Minimize, Zoom, Bring All to Front (mostly predefined)
- **Help**: About, Documentation, Report Issue

The Nuxt UI dashboard scaffold (`DashboardSidebar` etc.) lives inside the WebView — that's the in-app navigation. The OS menu bar is *additional*, not a replacement. Together they give Excel-familiar discoverability (menu bar) plus modern admin-tool affordances (sidebar, command palette).

**Don't duplicate keyboard shortcuts** between the OS menu (via `accelerator(...)`) and frontend libraries (`useMagicKeys`, Nuxt UI's `defineShortcuts`). The OS intercepts the accelerator before it reaches the WebView, so a frontend handler for `CmdOrCtrl+O` won't fire if the menu has that accelerator. Pick one per shortcut: menu accelerator for actions that have a menu entry, frontend composable for things like "focus the search box" that don't.

**System tray icon** (separate `TrayIcon` API) is for apps that live in the menu bar / system tray instead of as a regular window — clipboard managers, uptime monitors. Not relevant here.

---

## Frontend framework: Vue 3 (locked in)

Stack:
```
Vue 3 + Vite + TypeScript
Vue Router (more battle-tested than TanStack Router's Vue adapter)
Nuxt UI v4 (bundles Reka UI primitives + Tailwind 4 + TanStack Table integration + theming)
TanStack Query (Vue adapter) for async data fetching from Tauri commands
@vueuse/core for composables (debounce, throttle, useMagicKeys for hotkeys, etc.)
Pinia only if reactive()/ref() composition isn't enough (likely not needed)
Chart.js (via vue-chartjs) or ECharts for dashboard visualizations
```

Notes for the implementer:

**Nuxt UI is the primary component library.** Despite the name, it works in plain Vue 3 + Vite (not just Nuxt) via its Vite plugin since v4. Built on Reka UI primitives + Tailwind 4 + TanStack Table — so we get those underneath without separately installing them. ~125 components covering everything we need, including a purpose-built admin dashboard scaffold (`DashboardGroup`, `DashboardSidebar`, `DashboardNavbar`, `DashboardPanel`, `DashboardSearch`, `DashboardToolbar`, `DashboardResizeHandle`) which directly maps to what we're building.

The Table component is a TanStack Table wrapper with built-in support for: sorting, filtering, pagination, row selection, expansion, pinning (rows and columns), grouping with aggregations, column visibility, drag-and-drop, virtual scrolling, server-side pagination patterns, and infinite scroll. We don't need a separate AG Grid for the data-heavy view — Nuxt UI's Table covers Excel-familiar behavior without an extra dependency.

Theming is Tailwind-native with CSS variables. No "styled vs unstyled vs Tailwind passthrough" mode confusion (which has been a recurring complaint about PrimeVue).

Maintenance signal: NuxtLabs joined Vercel; v4 is current; LLMs.txt + MCP server + Skills available for AI-assisted development. Active development.

**`@vueuse/core`** fills the gaps where TanStack lacks Vue adapters. Most relevant: `useDebounceFn` / `useThrottleFn` (substitutes for TanStack Pacer, which has no Vue adapter), and `useMagicKeys` for keyboard shortcuts (substitutes for TanStack Hotkeys, also no Vue adapter). `useMagicKeys` exposes pressed keys as reactive refs and handles Cmd/Meta platform differences automatically — fine for the modest shortcut surface this app needs (command palette, escape-to-close-dialog, save/cancel). Sequences and scoped shortcuts aren't supported by useMagicKeys; if those become needed later, dedicated libraries like `hotkeys-js` exist. Note: Nuxt UI also ships its own `defineShortcuts` composable, so prefer that for app-level shortcuts and reach for `useMagicKeys` only for cases Nuxt UI's composable doesn't cover.

**TanStack Query** for fetching data from Tauri commands. Handles caching, retry, deduplication, background refetch, mutation lifecycle. The Vue adapter (`@tanstack/vue-query`) has full feature parity with the React version. Query keys can be reactive refs that auto-trigger refetches when changed. Pairs naturally with our IPC-bound architecture: every Tauri command is a query or a mutation.

**PrimeVue is the fallback** if Nuxt UI is missing something specific. Most plausibly: niche enterprise components like `TreeTable` or `OrganizationChart` (none of which we currently need), or PrimeVue's `useConfirm()` programmatic dialog API if Nuxt UI's `useOverlay` composable falls short. Don't reach for PrimeVue preemptively; community signal on PrimeVue's maintenance bandwidth and theming churn is mixed.

**Naive UI considered and rejected.** Worth recording because it's a common Tauri+Vue choice and someone may suggest it. The maintainer has prior experience with it and prefers not to use it. Don't relitigate.

**shadcn-vue considered and rejected** for this project. It would have meant copying components into our repo and owning them — appropriate for projects with bespoke design needs, but overkill for an internal admin tool where Nuxt UI's batteries-included approach matches the requirements better.

**Reactivity gotcha** (already known to the implementer): `ref()` for primitives, `reactive()` for objects, `toRefs()` when destructuring while preserving reactivity. Failure mode is silent — UI just stops updating, no error.

---

## Model integration: HuggingFace + runtime fetch

### Recommended approach: fetch at runtime, cache locally

Don't bundle models in the installer. Reasons:
- Three RoBERTa-base models in F32 is ~1.5 GB. Bundled, that's the installer size, which is rough for a "first-time user clicks download" experience.
- Decoupling models from app version means you can publish improved/retrained models without an app release.
- Lets you implement the "selectable models" workflow you described — pull from the HF collection at runtime, let the user choose.

The pattern:
1. App ships with no models bundled (installer is ~15 MB Tauri shell + UI).
2. On first launch, present a model-selection screen. Show available models from the HF collection (you can hardcode this list or, better, fetch the collection metadata via HF API).
3. Download selected models to a cache directory using the `hf-hub` Rust crate. Show progress.
4. Cache to platform-appropriate location via the `dirs` crate:
   - macOS: `~/Library/Application Support/com.yourorg.courseclassifier/models/`
   - Windows: `%APPDATA%\com.yourorg.courseclassifier\models\`
   - Linux: `~/.local/share/com.yourorg.courseclassifier/models/`
5. Subsequent launches: load from cache. Optionally check for model updates (HF revisions) on app launch or on demand.

The `hf-hub` crate mirrors the Python `huggingface_hub` API and handles caching, partial downloads, and revision pinning. It's the right tool.

### The conversion problem

The annamp models on HF are PyTorch (`.safetensors` with F32 weights). ONNX Runtime needs ONNX format. You have three options:

**Option A: Convert offline, host converted versions on HF**
- Run `optimum-cli export onnx --model annamp/classifying-courses-at-scale-six-digit-roberta-base ./onnx-output` for each.
- Push converted models to a new HF repo (under your own account, or coordinate with annamp to publish alongside the originals).
- App downloads ONNX directly. Clean.

**Option B: Convert in CI on every release**
- Same as A, but automated. More work upfront, less manual maintenance.

**Option C: Convert at first launch**
- Bad. Requires bundling either Python or a Rust ONNX exporter. Reintroduces the runtime bloat we're trying to avoid.

**Go with Option A initially, automate to B later if you're publishing model variants frequently.** Naming convention suggestion: `yourorg/courses-2digit-onnx`, `yourorg/courses-4digit-onnx`, `yourorg/courses-6digit-onnx`, with `main` always pointing to the F32 variant and tags/branches for `f16`, `int8` if/when you publish quantized versions.

Mention this to your collaborators — you may want annamp to be a co-author on the converted-model repos, or to publish them under their account directly. Worth a conversation.

### On quantization (deferred)

You said you don't want to quantize without confirming with collaborators. That's correct — quantization is a model-quality decision, not a deployment one. The accuracy reported on the model cards (0.65 for 6-digit, 0.75 enrollment-weighted) is for F32. F16 typically loses <0.5 percentage points. Int8 dynamic quantization typically loses 1–3 percentage points. Whether that's acceptable depends on what the data is being used for downstream.

**Plan for F32 initially.** ~500 MB per model, ~1.5 GB total first-time download. Architect the app to support multiple precision variants from the same HF repo (different filenames or different revisions) so you can swap in F16/int8 later without code changes.

### Selectable models — UI implications

If you want the "user picks which models to download" flow:
- First-launch screen shows a list of models with size, accuracy, and a checkbox.
- User can later go to Settings → Models and download additional ones, delete unused ones, check for updates.
- Status indicator in main UI showing which models are currently loaded into memory.

This fits the academic ethos of the underlying collection — users can see exactly what they're running, swap in newer versions, and the project doesn't lock anyone into a specific snapshot.

### One model-format gotcha

The model card notes the input format is `"{SUBJECT CODE} {CATALOG NUMBER} --- {COURSE TITLE}"`. The reference Flask app feeds raw `course_description` or `course_title`, which is probably hurting accuracy. Whatever you build, get this format right. The input schema for `classify_courses` should accept structured fields (subject, catalog number, title, optional description) and assemble the model input internally. Don't make the user (or the frontend) responsible for string formatting.

---

## Data architecture: persistence, IPC, and long-running jobs

**Working datasets are large.** Realistic CSVs are ~2M rows, ~200MB on disk. With 3 models that's 6M inference calls. CPU-only batch inference on RoBERTa-base is roughly 17 hours for a full dataset; consumer GPU brings it to ~1.5–2 hours. The app must assume long-running, interruptible, resumable jobs from day one.

### Storage: DuckDB

**Use DuckDB as the single store, no fallback.** Speed-optimal for our mixed workload, single dependency, single mental model. The user has explicitly prioritized speed over weight, and DuckDB's row-level write penalty is irrelevant in absolute terms at our scale. (The original draft also discussed a SQLite + DuckDB attach pattern as a fallback; that has been **dropped** — see retirement banner at top of file.)

Rough performance comparison at 2M rows. These are order-of-magnitude estimates, not benchmarks — actual numbers depend on hardware, indexes, and query specifics:

| Operation | SQLite only | DuckDB only | SQLite + DuckDB attached |
|---|---|---|---|
| Bulk import 200MB CSV | 20–40s | 2–5s | 20–40s (writes go to SQLite) |
| Insert batch of 100 rows during inference | 1–5ms | 5–20ms | 1–5ms |
| Single row lookup by indexed key | <1ms | 1–10ms | <1ms |
| Paginated table view (sorted, LIMIT 100 OFFSET 5000) | 1–5ms | 50–200ms | 1–5ms |
| `GROUP BY classification` aggregation on 2M rows | 500ms–2s | 20–100ms | 200–800ms |
| `GROUP BY school, classification` (multi-column) | 1–4s | 50–200ms | 300ms–1.2s |
| Complex analytical query (joins + window functions) | seconds | tens of ms | hundreds of ms |
| Disk footprint | ~250MB for 2M rows | ~150MB (columnar compresses well) | ~250MB |

**Why DuckDB-only over SQLite+DuckDB:** The attached-SQLite pattern still pays SQLite's row-based read cost — DuckDB queries through SQLite's API, so it can't apply columnar storage tricks. DuckDB-on-SQLite is faster than SQLite-native for analytics (vectorized engine helps) but slower than DuckDB-native (no columnar storage to scan). For interactive dashboards, the difference between "20–100ms" and "200–800ms" is the difference between "feels instant" and "feels like work happens." Worth optimizing for.

**Why DuckDB-only over SQLite-only:** Dashboard `GROUP BY` queries on 2M rows are 10–50x faster in DuckDB. For an exploratory tool where users click between aggregations, this matters more than the slower point-write cost (which is invisible against a multi-hour inference job).

**Where DuckDB's slower writes might bite:** Sustained mixed workloads (heavy concurrent writes during interactive reads) are less battle-tested in DuckDB than in SQLite WAL mode. Worth verifying with a stress test once the pipeline is working — run a large classification job and click around the dashboard simultaneously to confirm no pathological lag. The stress test is now a Project item; if it surfaces problems, we tune DuckDB (memory limits, threads, CHECKPOINT cadence, transaction shapes) rather than retreat to a different store.

DuckDB tunings (less critical than SQLite's PRAGMA list — defaults are already good):

```rust
let conn = duckdb::Connection::open("library.duckdb")?;
conn.execute_batch("
    SET threads = 4;                    -- match cores; leave headroom for inference
    SET memory_limit = '2GB';            -- prevent runaway memory on big aggregations
    PRAGMA enable_progress_bar = false;  -- disable CLI-style progress in embedded use
")?;
```

Other alternatives considered and rejected:
- **Polars**: dataframe library, not a database. No persistence layer or transaction model. Could be useful for in-memory result-set analytics later; wrong shape as primary store.
- **Parquet + DuckDB**: append-only writes during inference complicate file management. Workable but more moving parts.
- **libSQL/Turso, ClickHouse-local, chDB**: nothing here that DuckDB doesn't already give us, with worse Rust ecosystem support.
- **LMDB, sled, RocksDB**: KV stores. No SQL, no relational structure. Would mean building our own indexes and query layer.

### Schema

Critical design points:

1. **Source files are first-class.** A `source_files` table tracks file metadata (path, hashes, dirty/missing flags) separately from datasets. Files can be rehashed and checked for drift without touching dataset rows.
2. **Datasets are logical units with lineage.** Every dataset is materialized (has rows in `courses`), but records its provenance — derived from a file, derived from another dataset, or manually created. Lineage is metadata, not live; refreshing a derived dataset is an explicit user action.
3. **Models are normalized into their own table.** Every cache entry would otherwise repeat `(model_id, revision, type, precision)` 4M times. A surrogate key on a `models` table cuts cache size 30-50% and gives us a clean hook for model-specific metadata.
4. **Classifications are cached by inference configuration, not by run.** Two runs with identical model selections over overlapping datasets share cache entries. Adding a new run on already-classified data is nearly instantaneous.
5. **`courses` preserves source fidelity 1:1.** No uniqueness constraint on `(dataset_id, content_hash)` — real CSVs have legitimate duplicates (same course offered fall and spring, same applicant with two semesters of the same course). The inference cache deduplicates *compute*, but `courses` deduplicates nothing — what came in is what's stored. `row_index` tracks position in the current source file for export ordering, not for identity. Identity is the surrogate `id`.

What makes a classification deterministic and therefore cacheable:
- Model identity (HF repo)
- Model revision (HF commit hash)
- Model type (2/4/6 digit)
- Precision (F32/F16/int8)
- Input string (hashed as `content_hash`)

Random seeds, batch size, and input order do not affect output. Execution provider (CUDA/CPU/CoreML) can affect last-bit floating-point math but not argmax outcomes for these models — recorded on the run for reproducibility, not used as a cache key.

```
source_files
  id              BIGINT PRIMARY KEY      -- surrogate
  path            TEXT NOT NULL           -- last known FS path
  display_name    TEXT NOT NULL           -- typically filename, user-editable
  imported_at     TIMESTAMP NOT NULL
  imported_hash   VARCHAR NOT NULL        -- blake3 at import (immutable anchor)
  size_bytes      BIGINT
  last_checked_at TIMESTAMP               -- last time we re-verified the file
  current_hash    VARCHAR                 -- hash at last check (NULL if missing)
  is_missing      BOOLEAN DEFAULT FALSE   -- file not found at last check
  is_dirty        BOOLEAN DEFAULT FALSE   -- current_hash != imported_hash
  column_mapping  JSON                    -- frontend-authored mapping spec, applied by Rust at ingestion
  notes           TEXT
  -- index on imported_hash

datasets
  id              TEXT PRIMARY KEY        -- uuid
  title           TEXT NOT NULL           -- user-editable
  source_kind     TEXT NOT NULL           -- 'file' | 'derived' | 'manual'
  source_file_id  BIGINT REFERENCES source_files(id)  -- if source_kind='file'
  parent_dataset_id TEXT REFERENCES datasets(id)      -- if source_kind='derived'
  filter_spec     JSON                    -- declarative filter applied to parent
  is_materialized BOOLEAN NOT NULL DEFAULT TRUE  -- if false, dataset is a live view over parent
  imported_at     TIMESTAMP NOT NULL
  row_count       BIGINT
  supersedes_id   TEXT REFERENCES datasets(id)    -- optional version-chain link
  notes           TEXT

courses
  id                  BIGINT PRIMARY KEY  -- monotonic surrogate
  dataset_id          TEXT NOT NULL REFERENCES datasets(id)
  row_index           BIGINT NOT NULL     -- position in the current source-of-truth file (presentation, not identity)
  subject_code        VARCHAR
  catalog_number      VARCHAR
  course_title        VARCHAR
  course_description  VARCHAR
  school_name         VARCHAR
  school_year_enrolled VARCHAR
  extra_columns       JSON                -- anything else from the source CSV
  content_hash        VARCHAR NOT NULL    -- blake3 of the model input string
  is_classifiable     BOOLEAN NOT NULL DEFAULT TRUE   -- false if missing required fields
  parse_warnings      JSON                -- per-row import issues (encoding, quoting, etc.)
  -- No uniqueness on (dataset_id, content_hash) — legitimate duplicates exist
  -- in source data (same course offered fall and spring, etc.). The cache
  -- handles dedup at inference time; courses preserves source fidelity 1:1.
  -- index on (dataset_id, row_index)        -- export ordering, paginated browse
  -- index on (dataset_id, content_hash)     -- dedup query during inference
  -- index on (content_hash)                 -- Phase 2 cross-dataset matching

models
  id              BIGINT PRIMARY KEY      -- surrogate
  hf_repo         TEXT NOT NULL           -- e.g., "annamp/courses-six-digit-roberta-base"
  hf_revision     VARCHAR NOT NULL        -- HF commit hash
  model_type      TEXT NOT NULL           -- 2|4|6
  precision       TEXT NOT NULL           -- f32|f16|int8
  display_name    TEXT                    -- user-friendly label
  size_bytes      BIGINT                  -- on-disk size
  local_path      TEXT                    -- where the ONNX file lives
  downloaded_at   TIMESTAMP
  last_used_at    TIMESTAMP               -- for LRU eviction if needed
  UNIQUE (hf_repo, hf_revision, model_type, precision)

runs
  id                  TEXT PRIMARY KEY    -- uuid
  dataset_id          TEXT NOT NULL REFERENCES datasets(id)
  description         TEXT                -- user-supplied
  state               TEXT NOT NULL       -- pending|running|paused|completed|failed|interrupted|cancelled
  model_ids           JSON NOT NULL       -- array of model IDs (FKs to models.id) selected for this run
  course_filter       JSON                -- optional scope filter; null means "whole dataset"
  rows_total          BIGINT              -- total rows in run scope
  rows_processed      BIGINT              -- monotonically increasing across resumes
  unique_inputs_total BIGINT              -- post-dedup work units within scope
  unique_inputs_done  BIGINT
  cache_hits          BIGINT              -- skipped due to existing inference_results
  created_at          TIMESTAMP NOT NULL
  started_at          TIMESTAMP
  completed_at        TIMESTAMP
  last_progress_at    TIMESTAMP
  resume_count        INTEGER DEFAULT 0
  error_message       TEXT
  execution_provider  TEXT                -- which ONNX EP: cuda|directml|coreml|cpu|...

inference_results
  model_id        BIGINT NOT NULL REFERENCES models(id)
  content_hash    VARCHAR NOT NULL        -- blake3 of model input string
  classification  VARCHAR NOT NULL
  probability     REAL
  computed_at     TIMESTAMP NOT NULL
  computed_by_run TEXT REFERENCES runs(id)  -- audit: which run originally computed this
  PRIMARY KEY (model_id, content_hash)
  -- index on content_hash
```

A note on what was rejected: a single combined hash column (`blake3(model_id || revision || type || precision || content_hash)` as a PK) was considered and rejected. It doesn't reduce join structure (the join is on `content_hash`, not on model config — model config is a *filter*, expressed equivalently as N column predicates or one hash predicate). It actively makes cache management queries harder ("show me entries for the old revision" needs to scan + reconstruct, not filter by indexed column). The composite PK with normalized `models` table is the cleaner design.

The PK on `inference_results` is `(model_id, content_hash)`. To get classifications for a run:

```sql
-- For each model in the run's model_ids, join on cache key
SELECT c.id, c.school_name, c.course_title, ir.classification, ir.probability
FROM courses c
JOIN inference_results ir
  ON ir.content_hash = c.content_hash AND ir.model_id = ?
WHERE c.dataset_id = (SELECT dataset_id FROM runs WHERE id = ?)
```

A note on `paused` vs `interrupted`: paused is user-initiated (clicked Stop), interrupted is crash-discovered (state was `running` at app startup). Same recovery path, different UX semantics. Paused gets a Resume button; interrupted gets a "Resume?" dialog explaining what happened.

A note on `cache_hits`: tracking this lets the UI show "Run completed: 1.4M unique inputs, 800K computed (600K cache hits from prior runs)." Useful feedback for users who don't realize they're getting free reuse.

### Datasets, source files, and lineage

The split between `source_files` (physical file metadata) and `datasets` (logical units the user works with) supports several down-the-road features without forcing them up front:

**Refresh detection.** A `source_files` row tracks `imported_hash` (at import) and `current_hash` (last verified). A background or on-demand check rehashes the file and updates `current_hash`, `last_checked_at`, and the `is_dirty` / `is_missing` flags. The dataset itself is unaffected unless the user explicitly initiates a refresh. The UI surfaces a "source has changed since import" indicator.

**Derived datasets.** A dataset can declare `source_kind = 'derived'` with a `parent_dataset_id` and a `filter_spec`. By default these are materialized: the filter is applied at creation time, courses are copied (or selected via `INSERT...SELECT`) into the new dataset, and from that point the derived dataset stands on its own. Refreshing a derived dataset against its parent's current state is an explicit user action, not automatic.

**Live views (optional).** For lightweight exploration ("temporary filter, don't materialize"), a derived dataset can be created with `is_materialized = false`. In this mode the courses table doesn't have rows for the derived dataset; instead, queries route through a DuckDB `VIEW` over the parent's courses. Vectorized scans of 2M rows in DuckDB are fast enough that this is fine for browsing, though inference runs against unmaterialized datasets should materialize first to lock in the scope.

**Cascading refresh.** When a `source_files` row's `current_hash` differs from `imported_hash`, the user can ask the system to refresh datasets descended from that file. The system identifies the chain (file → root dataset → derived datasets) and offers a preview of changes ("12,000 new rows, 340 modified, 18 removed") before any modification. Don't auto-refresh; users must opt into changes that affect work they've already done.

Joins across this lineage chain are cheap in DuckDB. `courses → datasets → source_files` for displaying a course's full provenance is sub-millisecond. Don't optimize for join avoidance at the schema level.

### Cache idempotency: classifications survive dataset changes

The cache-keyed design has a property worth being explicit about: **classifications are not tied to datasets**. `inference_results` is keyed by `(model_id, content_hash)` only — no foreign key to courses, datasets, or runs. A row in the cache says "for this model, this exact input string produces this output," and that statement is true regardless of which dataset (or how many datasets) currently contain rows with that content hash.

Practical consequences:

**Updating a dataset doesn't lose classifications.** When a user refreshes a source and chooses to update an existing dataset in place, courses get added, modified, or deleted. Classifications for content that's still present continue to apply automatically, because the cache lookup is by `content_hash`. Classifications for newly-added content are computed on the next run. Classifications for deleted content stay in the cache (harmlessly) and would be reused if that content reappears in any future dataset.

**Deleting a dataset doesn't lose classifications either.** The same logic applies. Cache entries persist across dataset deletions and contribute to future cache hits.

**Cross-dataset queries find consistent results.** Selecting any course in any dataset, the system can find its classification under a given run configuration with the same query — `JOIN inference_results ON content_hash` — and the answer is deterministic. Two courses in different datasets with identical content (same subject, number, title) get the same classification, by construction. This is what makes the Phase 2 cross-matching feature trivial to implement on this schema.

**Reruns across updated data are mostly cache hits.** When a user chooses to update a dataset rather than supersede it, and then re-runs classification, only the rows whose content actually changed need fresh inference. Content that survived the update is recognized by hash and reused.

The `runs` table separately tracks operational state (progress, lifecycle, execution_provider) so that a run-in-progress can be resumed after a crash. Runs reference datasets (so we know what scope they covered), but classifications are not stored under runs — they're in the global cache. The `computed_by_run` field on `inference_results` is purely informational, recording which run *first* computed a given result.



### Re-ingestion: file-content as identity

When a user picks a file to import, hash it with blake3 (~30–60ms for 200MB). The hash, combined with the path, determines what happens next:

**Hash matches an existing source_file**: show "You imported this exact file on $DATE — open the dataset created from it, or create a new dataset from this file?" Default to opening. The "new dataset" option creates a separate `datasets` row pointing at the same `source_files` row, so two datasets can share a source.

**Path matches an existing source_file but hash differs**: show "$DISPLAY_NAME was previously imported from this path with different contents — what would you like to do?"
- *Update source*: rehash the source_file row, update the dataset's courses to reflect the new contents, and offer to refresh dependent derived datasets.
- *Supersede dataset*: create a new dataset (and new source_file row), set `supersedes_id` on the new dataset pointing to the old one. Both visible side by side; user can compare.
- *Independent import*: create a new source_file row and a new dataset, no relation to anything existing.

**Important: when the user chooses "Update source," compute and surface a deletion-detection summary before applying the update.** Compare the new file's content hashes against the existing dataset's courses and explicitly call out:
- New rows being added (courses with content hashes not present in the existing dataset)
- Modified rows (rows where `(subject_code, catalog_number)` matches an existing course but `content_hash` differs)
- **Deleted rows** (existing courses whose content hash is no longer present in the new file)

Deletions are the dangerous case — they're easy to miss visually in a CSV diff, and an admin who accepts an update without realizing courses were dropped from their working dataset could quietly lose track of transfer applicants. The UI should require explicit confirmation when deletions are detected ("This update will remove 47 courses from the dataset — continue?"), with the option to view the affected rows before confirming.

A non-destructive note that's worth surfacing: classifications are not lost when courses are deleted from a dataset. Cache entries in `inference_results` are keyed by content hash, not by course or dataset, so a deleted course's classification persists in the cache and would be reused if that content reappears in any future dataset. Nothing tied to the deletion is irrecoverable from a classification standpoint — but the *dataset's record* of having contained that course is gone, which is what the warning protects against.

**No matches**: standard new-source-file + new-dataset flow.

The system surfaces relational options at the right moment but lets the user make the call. No automatic linking, no automatic deletion. Datasets accumulate; the user prunes via UI.

A `source_files` row's `path` is plain string metadata — survives the source file moving, being deleted, or sitting on a network drive that's currently disconnected. It exists for display and reference, not as a foreign reference.

### CSV import flow: mapping UI in frontend, parsing in Rust

The interactive part of CSV import — the column mapping configurator — lives in the Vue frontend. Everything else (full parsing, hashing, validation, ingestion) lives in Rust. This is a stricter division than the typical "do whatever's convenient" approach and it pays off in three ways: one parser with one set of edge cases (encoding, quoting, line endings, BOM markers, malformed rows), hashing always at native speed, and the frontend never needs to handle large files.

The frontend only ever sees a tiny preview slice — the first row plus 5 or so data rows — which Rust extracts and returns. The mapping UI uses that preview to populate dropdowns with real data, but the preview is just there as a configurator aid; the frontend doesn't re-parse the file or compute anything from it. Mapping decisions are sent back to Rust as a spec, and Rust does both validation (full-file scan, dry run, returns stats) and ingestion (full-file scan, writes to DB) using the same mapping.

**Three Rust commands.** Each does one thing:

```rust
#[tauri::command]
async fn preview_csv(path: String, n_rows: usize) -> Result<CsvPreview, Error>;
// Returns: { headers, rows[5], detected_encoding, detected_has_header }
// Used to populate the mapping UI with real data.

#[tauri::command]
async fn validate_import(path: String, mapping: ColumnMapping) -> Result<ImportStats, Error>;
// Full-file scan, no DB writes.
// Returns: { row_count, unclassifiable_count, parse_warnings, encoding_used,
//            estimated_unique_hashes, deletion_summary (if updating existing) }
// Emits progress events during the scan.

#[tauri::command]
async fn import_csv(path: String, mapping: ColumnMapping, dataset_meta: DatasetMeta) -> Result<DatasetId, Error>;
// Full ingestion: parse, hash, batched-insert into DuckDB.
// Stores the mapping JSON on the source_files row for future re-use.
// Emits progress events throughout.
```

Separating `validate_import` from `import_csv` is deliberate: validation never writes to the DB, ingestion always does. No `dry_run: bool` flag means no "I forgot to set dry_run" footgun. The frontend always validates first and shows the user the stats; if they confirm, ingestion runs against the same file with the same mapping, and the result is deterministic relative to what the validation showed.

**The mapping spec.** Saved as JSON on the `source_files` row in a new column:

```
column_mapping  JSON  -- frontend-authored mapping spec, applied by Rust at ingestion
```

Schema of the JSON itself:

```typescript
{
  has_header: boolean,
  encoding: 'utf-8' | 'windows-1252' | 'auto',  // 'auto' = let Rust detect
  fields: {
    subject_code: FieldSpec,
    catalog_number: FieldSpec,
    course_title: FieldSpec,
    course_description: FieldSpec | null,
    school_name: FieldSpec | null,
    school_year_enrolled: FieldSpec | null,
  }
}

type FieldSpec =
  | { source: 'column'; index: number }
  | { source: 'columns'; indexes: number[]; separator: string }
  | { source: 'literal'; value: string };
```

The `literal` option matters: admins often process per-school files where the school name isn't a column in the data, it's implicit in which file they imported. "All rows in this file are from Big State U" should be expressible as a constant.

**Saving the mapping on source_files** has several payoffs:
- Re-importing the same file path auto-applies the saved mapping (user can override).
- Re-imports for the "Update source" flow re-use the original mapping by default — no re-mapping ceremony each time.
- The mapping persists as part of the import record. Six months later, an admin can still see how columns were interpreted.
- It's part of the data, not transient UI state. Survives crashes, app restarts, machine migrations.

**Frontend mapping UI.** Built around the preview Rust returned:

- **Header detection toggle.** Pre-set from `detected_has_header`; user can override.
- **Field dropdowns.** One per required/optional field. Each lists all available columns by header (or by index if no header). Pre-selected from an alias dictionary (`subject` → subject_code, `dept` → subject_code, etc.) when the header text matches a known variant.
- **Multi-column combination.** Each dropdown can be switched to "combine columns" mode where the user picks 2+ columns and a separator. Useful when subject and number arrive as separate fields and the model wants `{SUBJECT} {NUMBER}` formatted into the input.
- **Literal value option.** Each optional field's dropdown can be switched to "use a fixed value" with a text input. Common for school_name when the file is per-school.
- **Live synthetic preview.** As the user adjusts mappings, show "Here's how the first 5 rows will be interpreted" using only the preview data the frontend already has. No additional Rust calls needed for adjustments.
- **Disambiguation.** When multiple columns plausibly match a field (e.g., both `course_name` and `title` could be `course_title`), pre-select one and surface alternatives in the dropdown rather than hiding them.

**Alias dictionary lives in user data, not the app bundle.** Stored as a JSON file in the platform data directory (alongside the database). When a user manually maps "DEPT" to `subject_code` on a CSV that didn't auto-detect, optionally offer to save that as a new alias. The dictionary grows with use without requiring app updates.

**Trust boundaries.** Frontend treats CSV preview data as display-only — never uses field values to construct queries or paths. Rust treats the column mapping as untrusted input — validates that column indexes are within bounds for the file's actual structure, that combine-column separators are reasonable strings, that literal values fit size limits. Everything talking to DuckDB is parameterized regardless. Tauri's filesystem allowlist handles path concerns.
### Deduplication and caching across runs

The models are stateless per input — every forward pass is independent. Combined with the configuration-keyed cache, this gives us two layers of dedup:

**Within-run dedup**: real administrative data has substantial content duplication (same intro courses across schools, same standard offerings across years). Compute inference once per unique content hash, broadcast results across all matching courses. The dedup ratio depends heavily on the dataset's shape:

- **Datasets aligned to a unified course-numbering system** dedup very heavily. Measured against `data/validation.csv` (1.76M-row Texas higher-ed transfer panel using TCCNS, the Texas Common Course Numbering System): **~88% dedup, 209K unique inputs from 1.76M rows.** The same `MATH 1314 — College Algebra` literally appears 264 times across schools and years. Cache pays off massively: only 12% of nominal work needs fresh inference.
- **Datasets without a unified numbering system** dedup much less. When subject codes and catalog numbers vary across institutions ("CSCI 101" vs "CIS 110" vs "CMP 100" for the same intro course), only exact-string repeats — typically the same school's offerings across years — collapse. **Plan for 30–60% dedup as a baseline expectation** for non-aligned data; treat anything more as upside.

The cache-keyed design works either way; it just delivers more value the more aligned the data is. The UI should display cache-hit counts so users can see the saving in real time without needing to know any of this.

**Across-run dedup**: identical model configurations on overlapping datasets share cache entries. A user who runs 4-digit + 6-digit on a dataset, then later wants to add 2-digit, only pays the inference cost for the 2-digit model. A user who imports a corrected version of a previously-classified file gets cache hits for every course whose content didn't change.

The runner's algorithm:

```
for each model_id in run.model_ids:
    distinct_hashes = SELECT DISTINCT content_hash
                      FROM courses
                      WHERE dataset_id = run.dataset_id
                        AND (course_filter applied)
                        AND content_hash NOT IN (
                            SELECT content_hash FROM inference_results
                            WHERE model_id = ?
                        )
    
    accumulator = []
    for each batch of 32 hashes in distinct_hashes:
        results = run inference on batch
        accumulator.extend(results)
        if accumulator.size >= 1000 OR 30s_elapsed_since_last_flush:
            INSERT INTO inference_results (...) VALUES (...) for accumulator
            commit transaction
            accumulator = []
    
    flush remaining accumulator
```

Idempotent, dedup-aware, resumable. Crash recovery is the same query — anything in the cache is already done; anything missing is what's left to do.

Display progress against `unique_inputs_done / unique_inputs_total` (the actual work being done) and surface cache hits separately. Suggested format: "Processed 800K of 1.4M unique inputs · 600K cache hits from prior runs · covering 1.7M of 2M rows."

### Write-batching during inference

The naive pattern of "compute batch of 32, write batch of 32, repeat" creates ~30-50 small DuckDB transactions per second over a multi-hour job. That's a lot of write traffic competing with whatever interactive reads the dashboard might be doing.

Instead, accumulate inference results in memory and flush in larger chunks. Recommended chunk size: **1000 results or 30 seconds, whichever comes first**. This:

- **Reduces write transaction frequency by ~30-50x** vs per-batch writes
- **Bounds the crash window**: worst-case data loss on crash is 1000 results (~30 seconds of work)
- **Lets DuckDB amortize transaction overhead** across many rows (one fat insert is materially faster than 30 thin ones)
- **Doesn't require a separate in-memory database** — a `Vec<InferenceResult>` in the writer task is sufficient

Each chunk's transaction also updates the run's progress fields, so progress is crash-consistent with the cache:

```sql
BEGIN;
  INSERT INTO inference_results (model_id, content_hash, classification, ...)
  VALUES (...), (...), ...;       -- the 1000-result chunk

  UPDATE runs SET
    rows_processed     = ?,
    unique_inputs_done = ?,
    cache_hits         = ?,
    last_progress_at   = NOW()
  WHERE id = ?;
COMMIT;
```

After any crash, the run's progress fields reflect exactly what's in the cache. The resume query (find content hashes still missing) and the displayed progress agree by construction. If a chunk's transaction fails mid-flight, DuckDB rolls back both halves — no inconsistency, the next attempt just recomputes that chunk. State transitions (`pending → running`, `running → paused`, etc.) are separate small `UPDATE` statements outside the chunk path. A nice side effect: a freshly-opened progress view never starts blank, since it reads current state from the run row directly.

The combination of write-batching + separate read/write connections + read-only connection mode for dashboard queries should give DuckDB enough breathing room that interactive reads stay responsive during long inference jobs. The stress test (a Project item) validates this; the response to problems is DuckDB tuning, not a store swap.


### Reruns and partial work

The schema makes these scenarios cheap:

**Adding new model levels to an existing dataset** — for any `model_id` not already represented in the cache for the dataset's content hashes, query the missing combinations and compute only those. Often a small fraction of total work.

**Reclassifying with an updated model** — register the new model (new HF revision = new row in `models` with a new `id`) and reference it in a new run's `model_ids`. The cache is keyed by `model_id`, so the new run won't accidentally reuse old-model results. Both versions coexist.

**Crash recovery** — on startup, find runs in `running` state, mark as `interrupted`, offer the user a Resume dialog explaining what happened. Resume queries are the same as initial-run queries: anything in the cache is done; anything missing is what's left.

**Cache reuse across runs** — a run that operates on a previously-classified dataset (or a superseded version with overlapping content) gets free cache hits for matching `(model_id, content_hash)` pairs. Track these via the `cache_hits` counter on the run row and surface in the UI.

**Re-importing a corrected dataset** — when a user supersedes dataset A with dataset B, courses in B with content hashes that match anything previously classified hit the cache automatically. Only courses with novel content require fresh inference. This is the main payoff of the cache-keyed design: dataset corrections are nearly free to reclassify.

The job runner should not be a "process all rows" function. Write it as "for each model in the run, process content hashes whose results aren't yet in the cache" and call it idempotently. Crash recovery, dedup, and cross-run reuse all fall out of the same query shape.

### Inference pipeline

For 2M rows you want a real pipeline, not a `for row in rows` loop:

```
missing-cache-key reader → batch builder (32 hashes) → tokenizer → ONNX inference → write accumulator → chunked DB writer
```

`tokio` mpsc channels between stages with bounded queue depth keep memory flat regardless of dataset size. One dedicated database writer task batches inserts as described in "Write-batching during inference" above. Batch size 32 for inference is a reasonable starting point — tune later.

ONNX Runtime supports both intra-op and inter-op parallelism. On CPU, both are useful. On GPU, prefer larger batches over parallel inference calls — the device is already parallel.

**GPU execution providers.** Configure `ort` to try execution providers in priority order at startup: CUDA → DirectML → CoreML → ROCm → CPU. Fall back gracefully and log which one was selected. Surface this in a Settings → About panel and persist on the run record (`execution_provider` column) for reproducibility. The WebKitGTK/Nvidia rendering bug discussed elsewhere does NOT affect ONNX Runtime's CUDA path — `libcuda.so` is accessed directly, bypassing the browser/WebKit stack entirely.

### DuckDB concurrency considerations

DuckDB uses MVCC: writers create new versions of rows; readers see consistent snapshots from when their query started. In principle this is well-suited to our pattern (one write pipeline + many read queries), but the specific access shape — sustained small writes plus interactive analytical reads on the same database file — is less battle-tested than DuckDB's typical analytical-batch workload.

Mitigations baked into the design:

- **Separate connections for read and write paths.** DuckDB explicitly recommends this. Same connection serializes its operations. Inference pipeline opens one read-write connection; dashboard queries open their own (read-only) connections.
- **Read-only mode for dashboard connections.** `OPEN_READ_ONLY` skips MVCC bookkeeping for that connection, reducing overhead.
- **Write-batching** (covered above) reduces write transaction frequency by ~30-50x.
- **Periodic explicit CHECKPOINT during long-running inference**, every few minutes, keeps WAL size bounded so checkpointing pauses don't accumulate.
- **Short transactions on the read side.** Dashboard queries should commit quickly; don't keep transactions open for exploration.

If the stress test surfaces concurrency lag, the response is to tune DuckDB — memory limits, thread count, CHECKPOINT cadence, transaction shape, batch size, separating long-running reads from short interactive ones — rather than swap stores. (Earlier drafts of this doc documented a SQLite + DuckDB-attach fallback; that has been dropped.)

### IPC strategy: never ship 2M rows across the boundary

The frontend never holds the full dataset. Tauri commands return slices.

**Data table view** — Nuxt UI's Table in server-side mode (it wraps TanStack Table, which has built-in support for this pattern). Frontend requests `getCourses({datasetId, runId, offset, limit, sort, filter})` and gets back ~100 rows. DuckDB handles pagination/sort/filter; for paginated reads on indexed columns it's fast enough at our scale even though it's not its strongest pattern. Tiny payload, fast roundtrip.

**Dashboard view** — aggregations run natively in DuckDB. `SELECT ir.classification, COUNT(*) FROM courses c JOIN inference_results ir ON ir.content_hash = c.content_hash AND ir.model_id = ? WHERE c.dataset_id = ? GROUP BY ir.classification` returns at most a few hundred rows and runs in 20–100ms on 2M-row data. Frontend renders charts from summary data; never receives raw rows.

**Export** — stream from DuckDB to disk in Rust, return the file path to the frontend, hand off via Tauri's `shell.open` or filesystem API. Don't materialize results in memory or pass them through IPC. DuckDB's `COPY TO` command is great for this.

**Progress events during inference** — throttle hard. Emit `{rows_processed, rows_total, unique_inputs_done, unique_inputs_total, cache_hits, eta_seconds, current_model, execution_provider}` every 500ms or every N rows, whichever comes first. Per-row events will saturate the IPC channel and bog down the WebView.

### Workflow UX implications

The architecture pushes the UI design toward a tool-like rather than form-like shape:

The "upload" flow is **import-into-database**, not load-into-memory. After import, show "imported 2,000,000 rows from foo.csv" and let the user browse before classifying. Import alone for 200MB will take 30–60 seconds — show progress.

The classification flow is **select dataset → select models → start run**, then a long-running progress view, then results backed by the database with paginated browsing. The user might close the app and come back; results persist.

A **Datasets** sidebar concept becomes natural — users will accumulate multiple imported CSVs, run different model combinations on each, and want to revisit past runs. The mental model is closer to records-management software (think: a Zotero library, or Excel-with-projects-and-history) than the reference Flask app's stateless upload-process-download flow. Administrators expect to see "what files have I imported, what have I done with them, where are the results" at a glance.

### Cache locations

Use the `dirs` crate for platform-appropriate paths:
- **Models**: `data_dir() / com.yourorg.courseclassifier / models/`
- **Database**: `data_dir() / com.yourorg.courseclassifier / library.duckdb`
- **Imported CSVs** (optional copies for reproducibility): `data_dir() / com.yourorg.courseclassifier / sources/`

### Tauri-specific notes from the wild

Two pieces of community wisdom worth absorbing into the design:

**Keep Rust's surface area narrow.** Every Rust function is an IPC boundary you maintain, debug, and serialize across. Use Rust for what it's uniquely good for here: ONNX inference, tokenization, DuckDB I/O, HF Hub downloads, CSV streaming, file system access. Everything else (UI state, search filtering of small result sets, dashboard chart configuration, form validation) stays in TypeScript. This isn't dogma — it reduces the IPC contract you have to keep stable.

**Linux WebKitGTK has a shadow-rendering performance hit.** Heavy use of `box-shadow` (especially in frequently-rendered components like table rows) causes frame lag on Linux. Tailwind defaults use shadows liberally and Nuxt UI's components inherit that. Audit the data table specifically — minimize or remove shadows on row-level components, keep them on dialogs/popovers/cards where they render once. Test on Linux early; don't discover this at release time.

---

## Open questions to confirm with collaborators

1. **Code signing certificate access** — does UMich have an institutional cert? (Highest priority. Start the conversation now, before any code is written.)
2. **Model conversion + hosting** — convert annamp models to ONNX under your own HF account, or coordinate with annamp to publish ONNX versions in their account?
3. **Quantization** — F32 only initially, or do you want to publish F16/int8 variants? Affects download size and accuracy tradeoffs. Quick benchmark on a labeled subset would settle it. Note: precision is part of the cache key, so each variant gets its own cache entries — switching variants doesn't invalidate prior work, just means new computation.
4. **Input format** — confirm the reference app's accuracy issue around `course_title` vs `{SUBJECT} {NUMBER} --- {TITLE}`. Worth a quick benchmark on a labeled subset. Note: this affects what gets hashed into `content_hash`, so changing the format mid-project would invalidate existing cache entries (a bigger deal than the model-precision question).
5. **Hosted web app priority** — is this a "ship after the native app is solid" thing, or do you want it parallel? At 2M-row scale, the hosted web app is mostly a demo target — anyone with real data should run locally, both for speed and for keeping transcript data off shared infrastructure.
6. **Run history retention** — should runs be retained indefinitely, or auto-pruned after N days? Distinct from `inference_results`, which is the cache and should be retained as long as it's useful (small storage cost vs significant compute savings on rerun).
7. **DuckDB write contention under interactive use** — verify with a stress test once the pipeline works. Run a full classification job in the background and click around the dashboard simultaneously. Response to problems is DuckDB tuning, not a store swap (see updated Storage section).

---

## Suggested next steps for Claude Code

1. **Scaffold** the Tauri 2 project with Vue 3 + Vite + TypeScript. `npm create tauri-app@latest`. Verify a "Hello World" build runs on at least one target platform. Add Nuxt UI to the frontend via its Vite plugin (Tailwind 4, Reka UI, and TanStack Table integration come bundled). Wire up a minimal native application menu at this step too (File → Quit, Edit → Cut/Copy/Paste/SelectAll, View → Toggle Devtools) — even just the predefined items. Doing it during scaffolding means the menu bar exists when the rest of the app is built around it; retrofitting later means deciding which existing keyboard shortcuts and in-app buttons need menu equivalents.
2. **Inference spike**: stand up the Rust inference module with `ort` + `tokenizers` + `hf-hub`. Convert one annamp model to ONNX manually, hardcode-load it, and verify end-to-end inference matches the Python reference output bit-for-bit on a small labeled test set. **Verify the input-format question at this step** (raw title vs `{SUBJECT} {NUMBER} --- {TITLE}`) — get accuracy numbers before building UI on top of either.
3. **Database layer**: stand up DuckDB with the full schema (`source_files`, `datasets`, `courses`, `models`, `runs`, `inference_results`). Apply the recommended tunings. Write the dataset-import path: CSV streaming → blake3 file-hash → match-existing-or-create flow → source_file row → dataset row → batched inserts of courses with content_hash computed per row → progress events. Test on a 2M-row file end-to-end.
4. **Pipeline**: implement the cache-aware inference pipeline (missing-cache-key reader → batcher → tokenizer → inference → in-memory accumulator → chunked DB writer). Use separate read-write and read-only DuckDB connections. Implement `tokio` channels with graceful interruption. Persist progress and cache_hits to the runs table. Test resume-from-interruption by deliberately killing the process mid-run and verifying the next run picks up cleanly with no duplicate work and no missing rows. Also verify cache reuse: run the same model configuration twice on overlapping datasets and confirm the second run is mostly cache hits.
5. **Stress-test write/read concurrency**: run a long inference job in the background while issuing many dashboard-style aggregation queries from a separate read-only connection. Confirm the UI stays responsive. If problems appear, the response is DuckDB tuning (memory, threads, CHECKPOINT cadence, batch shape) — no store swap.
6. **Model management UI**: download/cache/select active models, with `models` table tracking each one. This is what the user sees first on a clean install.
7. **Import flow with column mapping**: Three Rust commands — `preview_csv` (returns headers + ~5 data rows), `validate_import` (full-file dry run, returns stats), `import_csv` (full ingestion, writes to DB and persists mapping on source_files). Vue frontend handles only the mapping configurator UI: alias-dictionary pre-selection, multi-column combination, literal-value fields, live synthetic preview from the small slice Rust returned. All parsing, hashing, and validation logic in Rust; frontend never reads the file directly. Test on a 200MB CSV with a non-trivial mapping (combined subject+number, literal school_name).
8. **Import → classify → browse loop**: minimum viable end-to-end UX. Datasets sidebar (use Nuxt UI's `DashboardSidebar` for the scaffold; show `source_file` lineage), import dialog wired to step 7's mapping flow, run configuration (model selections + optional course filter), progress view with cache-hit display, paginated results table using Nuxt UI's Table in server-side mode (it wraps TanStack Table, so server-side pagination follows the standard pattern).
9. **Source-file refresh detection**: on app launch (or on user demand), rehash known source_files, update `is_dirty` / `is_missing` / `current_hash` / `last_checked_at` directly on the source_files row, surface drift in the UI. When the user invokes the "update source" flow, run the deletion-detection summary (new / modified / deleted rows) and require explicit confirmation if any deletions are present.
10. **Dashboard and search**: native DuckDB aggregations. Most visually complex but architecturally low-risk once the database queries are solid.
11. **Signing and distribution**: notarized macOS build, signed Windows build, .deb/.rpm/.AppImage for Linux. Don't leave this to the end — verify the signing pipeline works end-to-end before there's anything important to ship.
