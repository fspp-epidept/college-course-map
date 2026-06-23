# college-course-map

A native desktop app for bulk-classifying college courses against CCM codes using fine-tuned RoBERTa models, designed to run on a researcher's laptop with no cloud round-trips.

- Classifies courses at 2-, 4-, and 6-digit CCM levels using [annamp's open-weight classifiers](https://huggingface.co/collections/annamp/classifying-courses-at-scale)
- Runs ONNX inference locally on GPU (CUDA / DirectML / CoreML) with CPU fallback
- Built for ~2M-row datasets with resumable jobs and content-hash-keyed result caching
- DuckDB-backed storage for streaming ingest and interactive dashboard queries

Pre-alpha. The Python model-conversion pipeline under `scripts/models/` is working end-to-end. The Tauri desktop app's core loop works — import a CSV, classify it locally via ONNX, and browse cached results — with the remaining work toward a shippable v1 tracked in Linear (team EPI).

## Prerequisites

- [Rust](https://rustup.rs/) — toolchain pinned via `rust-toolchain.toml`
- [Node.js](https://nodejs.org/) 20+ and [pnpm](https://pnpm.io/) 10+
- [Task](https://taskfile.dev/) (`go-task`) — top-level command runner
- [uv](https://docs.astral.sh/uv/) — only if you intend to run the Python model pipeline
- Tauri 2 platform dependencies: <https://v2.tauri.app/start/prerequisites/>

## Develop

Clone and install JS deps:

```sh
git clone git@github.com:fspp-epidept/college-course-map.git
cd college-course-map
pnpm install
```

Run the desktop app in dev mode:

```sh
pnpm tauri dev
```

Run the full check pipeline (Biome + clippy + rustfmt + `vue-tsc`):

```sh
task check
```

List every available task:

```sh
task
```

## Model-conversion pipeline

Standalone Python tooling that converts the annamp CCM classifiers from PyTorch to ONNX, verifies bit-for-bit parity, and measures CIP/CCM overlap on labeled panel data. The Tauri app never runs Python — it only consumes ONNX artifacts this pipeline produces. See [`scripts/models/README.md`](scripts/models/README.md).

## Architecture

- [`CLAUDE.md`](CLAUDE.md) — repository conventions, architectural ground rules, schema, IPC contracts, and threat model. The durable source of truth for design decisions.
- [`docs/keybinds.md`](docs/keybinds.md) — the three-layer keyboard-shortcut model (OS global / Tauri menu accelerator / WebView).
- [`docs/models-pipeline.md`](docs/models-pipeline.md) — the Python PyTorch→ONNX conversion pipeline.

## Layout

```
src/            Vue 3 + TS frontend
src-tauri/      Rust backend (inference, DuckDB, IPC)
scripts/models/ Python model-conversion pipeline
docs/           Architecture and design docs
```
