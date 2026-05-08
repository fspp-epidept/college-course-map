# scripts/models — ONNX conversion pipeline

Build-time tooling. Converts annamp's PyTorch CCM classifiers to ONNX, verifies parity, validates accuracy on labeled data, and (eventually) uploads converted models to Hugging Face. The Tauri app never runs Python — it only consumes the ONNX artifacts this pipeline produces.

> Note: the panel CSV columns are named `inventory_cip_two/four/six` for historical reasons, but the values are CCM codes, not federal CIP codes. The code uses `ccm_*` naming throughout; the column names are preserved as-is because they reference the actual data.

## Layout

```
scripts/models/
├── pyproject.toml         uv-managed Python project
├── uv.lock                committed lockfile (deterministic conversion)
├── _lib/                  shared module (model specs, format, inference, reporting)
├── convert.py             optimum-cli export, idempotent
├── verify.py              ONNX vs PyTorch parity on a synthetic corpus
├── validate.py            real-world accuracy on labeled panel data
├── upload.py              push to HF (TODO — placeholder until env-check is added)
├── data/
│   └── parity_inputs.csv  20-row synthetic corpus, committed
├── output/                generated ONNX, parity results, validation runs (gitignored)
└── reports/               committed markdown summaries (parity-latest, validation-latest)
```

## Run it

All commands are wrapped in Task. From the repo root:

```
task models:setup       # uv sync — install Python deps (~5 min first time)
task models:convert     # 6–15 min: download + export 3 models
task models:verify      # 1–3 min: parity check
task models:validate    # ~30s on GPU: 10k sample of validation.csv
task models:validate:full  # full panel (~30–60 min on a 4070-class GPU)
task models:all         # convert + verify
task models:clean       # wipe output/
```

`method: checksum` is set globally in `Taskfile.yaml`, so each step is skipped when its inputs and outputs haven't changed.

## CUDA setup

ONNX Runtime's CUDA EP needs the `libcublasLt.so.13`, `libcudnn.so.9`, etc. that ship inside the nvidia-* PyPI wheels. PyTorch finds them on its own; ORT does a lazy dlopen at session creation that doesn't search venv paths. The Taskfile sets `LD_LIBRARY_PATH` to point at `.venv/.../nvidia/cu13/lib` and `.../cudnn/lib` so the dynamic linker can find them — no other config needed.

If you invoke the scripts directly with `uv run python …` (bypassing Task), you'll need to set `LD_LIBRARY_PATH` yourself, or accept the CPU fallback. The Taskfile is the supported path.

The `onnxruntime-gpu` package itself comes from a non-PyPI nightly index because the PyPI build is still CUDA 12 only — see `[tool.uv.sources]` in `pyproject.toml`. CUDA 13 stable on PyPI will eventually replace this.

## Validation data

`validate.py` looks for the panel CSV in this order:

1. `--csv <path>` flag
2. `COURSE_CLASSIFIER_VALIDATION_CSV` env var
3. Default: `<repo_root>/data/validation.csv`

The file is gitignored. Source-of-truth location and provenance to be documented when known.

Filter: rows missing any of `sub_pref`, `course`, `inventory_course_title`, or any `inventory_cip_*` ground-truth column are dropped. Rows with `Multiple Course?` set are dropped. All other rows pass through.

## Format

The model input format is locked in `_lib/format.py`:

```
{subject_code} {catalog_number} --- {course_title}
```

This matches annamp's model card. The Tauri Rust app must produce byte-identical strings — `convert.py` writes `output/format_spec.json` as the cross-language contract.

## Open questions

- Train/test split status of `data/validation.csv` — unconfirmed. Until annamp confirms, treat validation accuracy as preliminary (may be inflated by training-set memorization).
- HF namespace for converted-ONNX repos — TBD. `upload.py` will read `HF_USERNAME` from `.env` (loaded automatically by uv).
- Quantization (F16 / int8) — deferred. F32 only for now.
