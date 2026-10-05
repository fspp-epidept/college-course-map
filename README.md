# college-course-map

A native desktop app that bulk-classifies college courses against College Course Map (CCM) codes on a researcher's laptop, with no cloud round-trips.

[![CI](https://github.com/fspp-epidept/college-course-map/actions/workflows/ci.yml/badge.svg)](https://github.com/fspp-epidept/college-course-map/actions/workflows/ci.yml)

- Classifies courses at the 2-, 4-, and 6-digit CCM levels using [annamp's open-weight classifiers](https://huggingface.co/collections/annamp/classifying-courses-at-scale), exported to ONNX (Open Neural Network Exchange); ModernBERT is the app-active model family
- Runs inference locally through swappable ONNX Runtime packs: CPU in every build, CUDA and TensorRT packs downloadable in-app on Windows and Linux, CoreML included on macOS
- Handles ~2M-row datasets with stoppable, resumable classification and a results cache keyed by `(model, content hash)`, so nothing re-pays for inference already done
- Stores everything in DuckDB: streaming CSV ingest, paginated result queries, exports written straight to disk

Pre-1.0. The core loop is complete: import a CSV, map columns, classify locally, browse and export results.

## Install

Download the installer for your platform from the [latest release](https://github.com/fspp-epidept/college-course-map/releases/latest):

- **Windows**: the `*-setup.exe` installer. It installs per-user, so no administrator rights are needed; the app appears in your per-user Start Menu
- **macOS**: the `.dmg`; drag the app to Applications
- **Linux**: the `.AppImage` (mark it executable, then run it), or the `.deb` / `.rpm` for your distribution

> [!NOTE]
> The macOS build is signed and notarized by Apple, so it opens without a Gatekeeper prompt. Windows builds aren't code-signed yet, so SmartScreen warns on first launch: click **More info**, then **Run anyway**.

## Quick start

1. **Download the models.** Open the **Models** activity in the left activity bar and click **Download Models**. The three classifiers (about 2 GB total) download once from Hugging Face, are hash-verified, and load automatically. Everything after this step is fully offline.
2. **Import a CSV.** Open the **Datasets** activity and click **Import CSV**. Pick your file, then map which columns hold the subject code, catalog number, and course title; recognized headers map automatically. No file handy? Use [`samples/sample_courses.csv`](samples/sample_courses.csv) from this repo, a 49,537-row real-shaped input whose headers auto-map.
3. **Classify.** Select the dataset and click **Classify**. It classifies at all three digit levels, with live progress. You can stop it and click **Classify** again later to pick up where it left off; results are cached by course content, so nothing is ever classified twice.
4. **Export.** In the dataset view, click **Export CSV** and choose a destination. Options: include all digit levels in one file, include the top-5 candidate codes with probabilities per level, or collapse to one row per unique course. Exports include your original input columns, so the file drops back into your existing workflow.

## GPU acceleration

CPU inference works out of the box on every platform and needs no configuration. On macOS the app runs on CPU. CoreML stays off until it passes the parity check against the Python reference.

On Windows or Linux with an NVIDIA GPU, open **Settings → Compute**:

1. Click **Download** on the CUDA (or TensorRT) backend. Each backend is a single download that bundles everything it needs
2. Click **Make Active**, then **Relaunch**

The active provider is shown at the top of the Compute page, and each dataset's page shows which provider classified it.

If a GPU backend fails on your machine (old driver, provider fails to load), the Compute page shows a warning and the app falls back safely. To disable GPU inference, make the **CPU** backend active again and relaunch. The **Provider priority** list under Advanced reorders execution providers within the active backend; changing it only requires a model reload, not a relaunch.

## Troubleshooting

The app writes a diagnostic log to `logs/app.log` in its data folder. It records startup steps, provider resolution, model load results, and errors; never course data. To attach it to a bug report, open **Settings → About** and click **Open Logs Folder**. The folder lives at:

- Windows: `%LOCALAPPDATA%\college-course-map\logs`
- macOS: `~/Library/Application Support/college-course-map/logs`
- Linux: `~/.local/share/college-course-map/logs`

### Restoring your data after an update

Before an update changes the database, the app copies it to `app.duckdb.pre-<version>.bak` next to `app.duckdb` (one level above `logs`). `<version>` is the app version that made the change. To go back:

1. Quit the app.
2. Install a version older than `<version>`.
3. Rename `app.duckdb` out of the way, then rename the `.bak` file to `app.duckdb`.

A version of the app that is older than its data refuses to open it and names the version to install. Versions 0.5.0 and earlier do not make that check.

Once you're happy with the update, delete the backup under **Settings → Storage**. **Reset App Data…** deletes it along with everything else.

## Freeing disk space

To delete a dataset, open it and click **Delete Dataset…**. That removes its courses. Cached classifications stay, so importing the same courses again needs no new inference.

**Settings → Storage** lists what the app keeps on disk and gives space back:

- **Cached classifications**: remove results from earlier model versions, or results for courses no dataset contains any more
- **Compact Database…**: the database file never shrinks by itself, so deleted data leaves free space inside it. Compacting writes a fresh copy without that space, then relaunches the app
- **Database backup**: delete the pre-update backup

To delete a GPU backend you no longer use, click **Remove** on its row in **Settings → Compute**.

## Resetting and uninstalling

To start over without uninstalling, open **Settings → Storage** and click **Reset App Data…**. The app relaunches and deletes every dataset, run, cached result, downloaded model, and runtime pack before it starts. Settings and custom themes are kept unless you tick **Also reset settings and themes**.

Uninstalling removes the app but not its data, which can reach several GB with models and GPU runtime packs:

- **Windows**: in the uninstaller, tick **Delete the application data**. That removes the `college-course-map` folders below. Updates never delete data
- **macOS and Linux**: no uninstaller runs, so delete the folders by hand after removing the app (dragging it to the Trash, or removing the `.deb` / `.rpm` / `.AppImage`)

| OS | Folders |
| --- | --- |
| Windows | `%APPDATA%\college-course-map`, `%LOCALAPPDATA%\college-course-map` |
| macOS | `~/Library/Application Support/college-course-map`, `~/Library/Caches/college-course-map` |
| Linux | `~/.config/college-course-map`, `~/.local/share/college-course-map` |

The app's window also keeps a small WebView storage folder named after its bundle ID. The Windows uninstaller removes it with the same checkbox; on macOS and Linux, delete it too:

- **macOS**: `~/Library/WebKit/edu.umich.epi.college-course-map`, `~/Library/Caches/edu.umich.epi.college-course-map`
- **Linux**: `~/.local/share/edu.umich.epi.college-course-map`

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
| `task db:fixture` | Write the upgrade-test fixture for the current schema and DuckDB version |
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
