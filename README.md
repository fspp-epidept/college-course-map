# college-course-map

A native desktop app that bulk-classifies college courses against College Course Map (CCM) codes on a researcher's laptop, with no cloud round-trips.

[![CI](https://github.com/fspp-epidept/college-course-map/actions/workflows/ci.yml/badge.svg)](https://github.com/fspp-epidept/college-course-map/actions/workflows/ci.yml)

- Classifies courses at the 2-, 4-, and 6-digit CCM levels using [annamp's open-weight classifiers](https://huggingface.co/collections/annamp/classifying-courses-at-scale), exported to ONNX (Open Neural Network Exchange); ModernBERT is the app-active model family
- Runs inference locally through swappable ONNX Runtime packs: CPU in every build, CUDA and TensorRT packs downloadable in-app on Windows and Linux, CoreML included on macOS
- Handles ~2M-row datasets with resumable runs and a results cache keyed by `(model, content hash)`, so nothing re-pays for inference already done
- Stores everything in DuckDB: streaming CSV ingest, paginated result queries, exports written straight to disk

Pre-1.0. The core loop is complete: import a CSV, map columns, classify locally, browse and export results. Remaining v1 work is tracked in Linear (team EPI).

## Prerequisites

- [Rust](https://rustup.rs/): toolchain pinned by `rust-toolchain.toml`
- [Node.js](https://nodejs.org/) 20+ and [pnpm](https://pnpm.io/) 10+
- [Task](https://taskfile.dev/) (`go-task`): the top-level command runner
- Tauri 2 platform dependencies: <https://v2.tauri.app/start/prerequisites/>
- [uv](https://docs.astral.sh/uv/), only if you run the Python model pipeline

## Develop

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

The app fetches its classifier models on first launch from the Models panel. `samples/sample_courses.csv` is a 49,537-row input for trying an import end to end.

Run the full check pipeline (Biome, clippy, rustfmt, `vue-tsc`):

```sh
task check
```

List every available task:

```sh
task
```

## Model-conversion pipeline

Standalone Python tooling converts the annamp classifiers from PyTorch to ONNX, verifies parity, and uploads the converted models to Hugging Face. The Tauri app never runs Python; it consumes the ONNX artifacts this pipeline publishes. See [`scripts/models/README.md`](scripts/models/README.md).

## More documentation

- [`CLAUDE.md`](CLAUDE.md): repository conventions, architectural ground rules, schema, IPC (inter-process communication) contracts, and threat model
- [`docs/keybinds.md`](docs/keybinds.md): the three-layer keyboard-shortcut model (OS global / menu accelerator / WebView)
- [`docs/model-confidence.md`](docs/model-confidence.md): how confidence values are computed, and how to reproduce one
- [`PRODUCT.md`](PRODUCT.md): personas, brand personality, and design principles
- [`samples/README.md`](samples/README.md): tracked sample input files

## Layout

```text
src/            Vue 3 + TypeScript frontend
src-tauri/      Rust backend (inference, DuckDB, IPC)
scripts/models/ Python model-conversion pipeline
samples/        Tracked sample input files
docs/           Design docs
```
