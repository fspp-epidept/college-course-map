# scripts/models — ONNX conversion pipeline

Build-time tooling. Converts annamp's PyTorch CCM classifiers to ONNX, verifies ONNX↔PyTorch parity, measures CIP/CCM taxonomy overlap on the labeled panel, and uploads converted models to Hugging Face. The Tauri app never runs Python — it only consumes the ONNX artifacts this pipeline produces.

> Note: the panel CSV (`data/validation.csv`) `inventory_cip_*` columns contain federal **CIP codes** (Classification of Instructional Programs). The annamp models output **CCM codes** — a distinct hierarchical 2/4/6-digit taxonomy. CIP and CCM overlap heavily at the broad 2-digit level but diverge at 4/6-digit. `validate.py`'s overlap rate is *not* model accuracy; it's a CIP/CCM agreement measure. The meaningful correctness check is parity (Rust ONNX == Python ONNX == annamp PyTorch). In code, `ccm_*` names refer to model-output identifiers; panel column names are preserved as-is.

## Layout

```
scripts/models/
├── pyproject.toml         uv-managed Python project
├── uv.lock                committed lockfile (deterministic conversion)
├── _lib/                  shared module (model specs, format, inference, reporting, neg_rewrite)
├── convert.py             optimum-cli export + fp32 Neg→Mul pass, idempotent
├── verify.py              ONNX vs PyTorch parity on a synthetic corpus
├── validate.py            CIP/CCM overlap rate on labeled panel data
├── upload.py              push to HF, one repo per spec + a collection; idempotent
├── manifest.py            regenerate src-tauri/models.toml from the published repos
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
task models:upload      # push converted models to the HF namespace
task models:manifest    # regenerate src-tauri/models.toml from the published repos
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

## Model families

Both annamp families are converted and published: RoBERTa and ModernBERT. The app-active family is **ModernBERT** (decision 2026-07-03); the RoBERTa repos stay published on HF but are not what the app downloads. `src-tauri/models.toml` pins the app-active repos by commit SHA and per-file sha256; regenerate it with `task models:manifest` after any upload.

## Export pass: fp32 Neg → Mul(-1)

ONNX Runtime's CoreML execution provider has no `Neg` builder, and ModernBERT's export carries two `Neg` nodes per layer (rotary `rotate_half`), which splits every layer into CoreML/CPU partitions. After export, `convert.py` runs `_lib/neg_rewrite.py`: every fp32 `Neg` becomes `Mul(x, -1.0)` (bit-identical in IEEE float), names are preserved, and the pass proves zero output difference against the pre-rewrite graph on the parity corpus before saving. Non-fp32 `Neg` nodes are left alone and printed. Rewritten graphs carry `metadata_props` `coreml_neg_rewrite=1`; graphs with nothing to rewrite (RoBERTa) are untouched. Decision 2026-08-26.

`upload.py` reads `HF_USERNAME` from `.env` (loaded automatically by uv) and pushes to `<HF_USERNAME>/courses-{two,four,six}-digit-{roberta,modernbert}-base-onnx`.

## Open questions

- Train/test split status of `data/validation.csv` — unconfirmed. Mostly moot now that we know the panel is CIP and the models are CCM (so memorization can't inflate the *overlap* rate the way it would inflate accuracy on same-taxonomy labels), but worth confirming if annamp ever publishes a CCM-labeled holdout.

## Closed decisions

- Quantization (F16 / int8) — **never, without revisiting parity** (decision 2026-07-29). Research users need unmodified model outputs; correctness is exact-match parity against the Python reference, which quantization breaks. F32 only.
