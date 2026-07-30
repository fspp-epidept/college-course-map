# college-course-map

A native desktop app that bulk-classifies college courses against College Course Map (CCM) codes on a researcher's laptop, with no cloud round-trips.

[![CI](https://github.com/fspp-epidept/college-course-map/actions/workflows/ci.yml/badge.svg)](https://github.com/fspp-epidept/college-course-map/actions/workflows/ci.yml)

- Classifies courses at the 2-, 4-, and 6-digit CCM levels using [annamp's open-weight classifiers](https://huggingface.co/collections/annamp/classifying-courses-at-scale), exported to ONNX (Open Neural Network Exchange); ModernBERT is the app-active model family
- Runs inference locally through swappable ONNX Runtime packs: CPU in every build, CUDA and TensorRT packs downloadable in-app on Windows and Linux, CoreML included on macOS
- Handles ~2M-row datasets with resumable runs and a results cache keyed by `(model, content hash)`, so nothing re-pays for inference already done
- Stores everything in DuckDB: streaming CSV ingest, paginated result queries, exports written straight to disk

Pre-1.0. The core loop is complete: import a CSV, map columns, classify locally, browse and export results.

## Install

Download the installer for your platform from the [latest release](https://github.com/fspp-epidept/college-course-map/releases/latest):

- **Windows**: the `*-setup.exe` installer. It installs per-user, so no administrator rights are needed; the app appears in your per-user Start Menu
- **macOS**: the `.dmg`; drag the app to Applications
- **Linux**: the `.AppImage` (mark it executable, then run it), or the `.deb` / `.rpm` for your distribution

> [!NOTE]
> Builds are not yet code-signed. Windows SmartScreen will warn on first launch: click **More info**, then **Run anyway**. On macOS, Control-click the app and choose **Open**; if it is still blocked, approve it under **System Settings → Privacy & Security → Open Anyway**.

## Quick start

1. **Download the models.** Open the **Models** activity in the left activity bar and click **Download models**. The three classifiers (about 2 GB total) download once from Hugging Face, are hash-verified, and load automatically. Everything after this step is fully offline.
2. **Import a CSV.** Open the **Datasets** activity and click **Import CSV**. Pick your file, then map which columns hold the subject code, catalog number, and course title; recognized headers map automatically. No file handy? Use [`samples/sample_courses.csv`](samples/sample_courses.csv) from this repo, a 49,537-row real-shaped input whose headers auto-map.
3. **Classify.** Select the dataset and click **Classify**. One run classifies at all three digit levels, with live progress. Long runs are interruptible and resumable, and results are cached by course content, so nothing is ever classified twice.
4. **Export.** In the dataset view, click **Export CSV** and choose a destination. Options: include all digit levels in one file, include the top-5 candidate codes with probabilities per level, or collapse to one row per unique course. Exports include your original input columns, so the file drops back into your existing workflow.

## GPU acceleration

CPU inference works out of the box on every platform and needs no configuration. On macOS, the default pack already includes CoreML; there is nothing to set up.

On Windows or Linux with an NVIDIA GPU, open **Settings → Compute**:

1. Click **Download** on the CUDA (or TensorRT) backend. Each backend is a single download that bundles everything it needs
2. Click **Make active**, then **Relaunch**

The active provider is shown at the top of the Compute page, and every run records which provider it used.

If a GPU backend fails on your machine (old driver, provider fails to load), the Compute page shows a warning and the app falls back safely. To disable GPU inference, make the **CPU** backend active again and relaunch. The **Provider priority** list under Advanced reorders execution providers within the active backend; changing it only requires a model reload, not a relaunch.

## Development

Everything below is for working on the app itself. If you installed a release build, you're done; none of this applies.

### Prerequisites

- [Rust](https://rustup.rs/): toolchain pinned by `rust-toolchain.toml`
- [Node.js](https://nodejs.org/) 20+ and [pnpm](https://pnpm.io/) 10+
- [Task](https://taskfile.dev/) (`go-task`): the top-level command runner
- Tauri 2 platform dependencies: <https://v2.tauri.app/start/prerequisites/>
- [uv](https://docs.astral.sh/uv/), only if you run the Python model pipeline

### Build and run

Clone and install JS dependencies:

```sh
git clone git@github.com:fspp-epidept/college-course-map.git
cd college-course-map
pnpm install
```

Run the desktop app in dev mode. The first run downloads the CPU ONNX Runtime pack into `src-tauri/runtimes/` before launching:

```sh
task dev
```

The app fetches its classifier models on first launch from the Models panel.

Run the full check pipeline (Biome, clippy, rustfmt, `vue-tsc`):

```sh
task check
```

List every available task:

```sh
task
```

The ones you'll reach for most:

| Task | What it does |
| --- | --- |
| `task gen:bindings` | Regenerate the typed IPC bindings (`src/bindings.ts`) after changing a Rust command |
| `task check:parity` | Assert Rust ONNX inference matches the Python reference on the parity fixture |
| `task check:runtime` | Report which runtime pack and execution provider this machine resolves to |
| `task check:throughput` | Benchmark batched inference over a CSV |
| `task runtimes:fetch -- cuda` | Fetch the CUDA runtime pack for GPU development |
| `task db:reset` / `task seed:demo` | Reset the dev database / seed it with fixture data |
| `task build` | Build the installer bundle for this platform |

### Model-conversion pipeline

Standalone Python tooling converts the annamp classifiers from PyTorch to ONNX, verifies parity, and uploads the converted models to Hugging Face. The Tauri app never runs Python; it consumes the ONNX artifacts this pipeline publishes. See [`scripts/models/README.md`](scripts/models/README.md).

### More documentation

- [`CLAUDE.md`](CLAUDE.md): repository conventions, architectural ground rules, schema, IPC (inter-process communication) contracts, and threat model
- [`docs/keybinds.md`](docs/keybinds.md): the three-layer keyboard-shortcut model (OS global / menu accelerator / WebView)
- [`docs/model-confidence.md`](docs/model-confidence.md): how confidence values are computed, and how to reproduce one
- [`samples/README.md`](samples/README.md): tracked sample input files

### Layout

```text
src/            Vue 3 + TypeScript frontend
src-tauri/      Rust backend (inference, DuckDB, IPC)
scripts/models/ Python model-conversion pipeline
samples/        Tracked sample input files
docs/           Design docs
```
