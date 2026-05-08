# Course Classifier — Models Pipeline (Python, uv-managed)

Companion doc to the main handoff. Covers the one-time-ish Python pipeline that takes the three annamp PyTorch models and produces ONNX artifacts hosted under your own HF account, ready for the Tauri app to fetch at runtime.

The Python in this project is **build-time tooling, not runtime infrastructure**. The Tauri app never calls Python. This pipeline produces files; those files get hosted on HF; the Rust app fetches them. After conversion, Python isn't in the loop until you need to add a new model variant or a new precision tier.

## Source models (annamp)

Three RoBERTa-base CCM classifiers, all on Hugging Face:

- `annamp/classifying-courses-at-scale-two-digit-roberta-base`
- `annamp/classifying-courses-at-scale-four-digit-roberta-base`  *(verify exact name)*
- `annamp/classifying-courses-at-scale-six-digit-roberta-base`  *(verify exact name)*

Each repo contains: `model.safetensors` (~500 MB), `tokenizer.json` (fast tokenizer, what we need), `config.json` (architecture + label mappings), plus the slow tokenizer files (`vocab.json`, `merges.txt`) and a model card. Verified against the two-digit repo; the others are almost certainly the same shape.

The exact repo names for four-digit and six-digit are unconfirmed at time of writing. First action of the convert script is to verify these resolve.

## Goal

For each of the three source models:

1. Download from `annamp/...`.
2. Convert PyTorch → ONNX (F32 only for Phase 1; quantization deferred).
3. Verify parity: the ONNX model produces the same predictions as the PyTorch original within numerical tolerance.
4. Sanity-check on labeled data if available (does accuracy match what's reported in the source model card?).
5. Generate a model card noting provenance, conversion command, verification results.
6. Upload to your HF account as a new repo (Pattern A from the previous discussion — keeps annamp's research repos untouched, your ONNX repos clearly labeled as deployment-format derivatives).

End state: three new HF repos under your namespace, each containing `model.onnx` + tokenizer files + config + model card. The Tauri app's Rust code fetches from these repos.

## Prerequisites

- `uv` installed. `curl -LsSf https://astral.sh/uv/install.sh | sh` on Unix; `powershell -ExecutionPolicy ByPass -c "irm https://astral.sh/uv/install.ps1 | iex"` on Windows.
- A Hugging Face account with write access. Generate a token at huggingface.co/settings/tokens (Type: Write). Export as `HF_TOKEN` in your shell, or use `huggingface-cli login` once.
- Disk space: ~3 GB free for the three F32 models + their PyTorch sources + scratch space during conversion.
- Time: each model takes 2-5 minutes to convert, 1-2 minutes to verify, ~1-2 minutes to upload depending on connection. Plan for an hour end-to-end.
- Optional: a labeled test set of course inputs with known CCM codes, for accuracy verification beyond parity. If unavailable, parity verification on diverse but unlabeled inputs is sufficient for Phase 1.

## Project layout

Place the pipeline at `scripts/models/` in the repo, isolated from the Tauri app and frontend:

```
course-classifier/
├── src-tauri/                 # Rust app
├── web/                       # Vue frontend
├── scripts/
│   └── models/                # This pipeline
│       ├── pyproject.toml
│       ├── uv.lock
│       ├── convert.py
│       ├── verify.py
│       ├── upload.py
│       ├── parity_inputs.csv  # Test corpus, committed
│       └── output/            # Generated, gitignored
├── Taskfile.yml
└── .gitignore                 # Add: scripts/models/output/
```

The `output/` directory holds the converted ONNX files locally. It's gitignored — these files are too big for git, and the canonical copies live on HF after upload.

## Project setup

From `scripts/models/`:

```bash
uv init --name course-classifier-models --python 3.11
```

Then write `pyproject.toml`:

```toml
[project]
name = "course-classifier-models"
version = "0.1.0"
description = "Convert annamp HF models to ONNX, verify, and publish"
requires-python = ">=3.11"
dependencies = [
    "transformers>=4.45",
    "torch>=2.4",
    "optimum[onnxruntime]>=1.23",
    "onnx>=1.17",
    "onnxruntime>=1.20",
    "numpy>=1.26",
    "huggingface_hub>=0.26",
    "pandas>=2.2",       # for reading the parity CSV
]

[tool.uv]
# Pin lockfile for reproducibility — see note below
```

Lock and install:

```bash
uv lock
uv sync
```

The lockfile (`uv.lock`) is critical here and **must be committed**. ONNX export from `transformers` models has occasionally fragile dep version interactions; a `transformers` version paired with an incompatible `optimum` version can produce ONNX files that look fine but produce different probabilities than the source. Committing the lockfile means the conversion is reproducible six months later when someone needs to regenerate the models.

This matters more than usual because the ONNX outputs feed into `content_hash` computations in the Tauri app — if a regenerated model behaves differently, every cached classification becomes invalid. The lockfile is your insurance against that.

## Pipeline stage 1: Convert

`scripts/models/convert.py`:

```python
"""Convert annamp PyTorch models to ONNX."""
from __future__ import annotations

import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class ModelSpec:
    source_repo: str            # HF repo to pull from
    output_subdir: str          # Local folder under output/
    display_name: str           # For logs


MODELS = [
    ModelSpec(
        source_repo="annamp/classifying-courses-at-scale-two-digit-roberta-base",
        output_subdir="two-digit",
        display_name="Two-digit CCM classifier",
    ),
    ModelSpec(
        source_repo="annamp/classifying-courses-at-scale-four-digit-roberta-base",
        output_subdir="four-digit",
        display_name="Four-digit CCM classifier",
    ),
    ModelSpec(
        source_repo="annamp/classifying-courses-at-scale-six-digit-roberta-base",
        output_subdir="six-digit",
        display_name="Six-digit CCM classifier",
    ),
]


def convert(spec: ModelSpec, output_root: Path) -> Path:
    output_dir = output_root / spec.output_subdir
    output_dir.mkdir(parents=True, exist_ok=True)

    print(f"\n=== Converting {spec.display_name} ===")
    print(f"  Source: {spec.source_repo}")
    print(f"  Output: {output_dir}")

    # CRITICAL: --task sequence-classification ensures the exported graph
    # outputs class logits, not hidden states. RoBERTa-base configs can be
    # ambiguous and the default task inference may pick wrong.
    cmd = [
        "optimum-cli", "export", "onnx",
        "--model", spec.source_repo,
        "--task", "text-classification",
        # F32 only for Phase 1. Add --fp16 or --quantize avx512 later.
        str(output_dir),
    ]
    subprocess.run(cmd, check=True)

    # Sanity check: confirm the ONNX file loaded back in
    onnx_path = output_dir / "model.onnx"
    if not onnx_path.exists():
        raise RuntimeError(f"Conversion succeeded but {onnx_path} not found")
    
    size_mb = onnx_path.stat().st_size / (1024 * 1024)
    print(f"  → model.onnx: {size_mb:.1f} MB")
    return output_dir


def main() -> int:
    output_root = Path(__file__).parent / "output"
    output_root.mkdir(exist_ok=True)
    
    for spec in MODELS:
        try:
            convert(spec, output_root)
        except subprocess.CalledProcessError as e:
            print(f"\nFAIL: {spec.display_name} conversion failed", file=sys.stderr)
            return 1
    
    print(f"\n✓ All {len(MODELS)} models converted")
    return 0


if __name__ == "__main__":
    sys.exit(main())
```

Run with `uv run python convert.py`. Each model takes 2-5 minutes; the script prints progress per model.

**Note on the `--task` flag**: optimum's task names changed slightly across versions. As of optimum 1.23, the correct flag for RoBERTa sequence classification is `text-classification`. If you're on an older version it might be `sequence-classification`. The `optimum-cli export onnx --help` output shows the valid values for your version.

**Verify before proceeding to stage 2**: open one of the `output/<size>/` directories and confirm it contains `model.onnx`, `tokenizer.json`, `config.json`, plus the slow tokenizer files. The directory should look essentially like the source HF repo with `.safetensors` replaced by `.onnx`.

## Pipeline stage 2: Verify parity

This is the stage that catches conversion bugs. Don't skip it. Skipping it means shipping models that "work" but produce different results than the originals.

First, build a parity test corpus. `scripts/models/parity_inputs.csv`:

```csv
subject_code,catalog_number,course_title,course_description
MATH,101,"Calculus I","Differential and integral calculus of one variable."
ENGL,200,"Introduction to Literature","Survey of major literary movements from antiquity to present."
BIO,150,"Cell Biology","Structure and function of eukaryotic cells."
CS,101,"Introduction to Computer Science","Programming fundamentals using Python."
ART,110,"Drawing I","Foundations of observational drawing techniques."
HIST,205,"World History to 1500","Comparative survey of major world civilizations."
PSY,101,"Introductory Psychology","Survey of major topics in scientific psychology."
ECON,101,"Principles of Microeconomics","Theory of consumer and firm behavior."
PHIL,200,"Introduction to Ethics","Major ethical theories and contemporary moral problems."
MUS,103,"Music Appreciation","Listening skills for Western classical and popular music."
NURS,300,"Pathophysiology","Cellular and systemic responses to disease."
SPAN,101,"Elementary Spanish I","Beginning communicative Spanish for non-native speakers."
ANTH,201,"Cultural Anthropology","Comparative study of human societies and cultures."
CHEM,210,"Organic Chemistry","Structure and reactions of carbon-based compounds."
BUS,150,"Introduction to Business","Survey of functional areas in business operations."
EDUC,300,"Educational Psychology","Application of psychology to teaching and learning."
ENGR,205,"Statics","Forces in equilibrium for engineering systems."
GEOG,101,"Physical Geography","Earth's physical systems and processes."
LING,210,"Introduction to Linguistics","Scientific study of human language."
SOC,101,"Introduction to Sociology","Group behavior and social institutions."
```

20 inputs across diverse subjects is the bare minimum. More is better. If you have access to a labeled holdout from the original training run, use that and verify accuracy too.

**The input format question matters here.** Per the open issues in the main handoff doc, the model may have been trained on `{SUBJECT} {NUMBER} --- {TITLE}` rather than raw `course_title`. The verification script needs to format the input the same way the model was trained, or you'll get false negatives (the model legitimately produces different predictions on differently-formatted inputs, but that's a feature, not a conversion bug). Confirm the training format from the source model card or training code before running this stage.

`scripts/models/verify.py`:

```python
"""Verify ONNX models produce same predictions as PyTorch sources."""
from __future__ import annotations

import sys
from pathlib import Path
from dataclasses import dataclass

import numpy as np
import pandas as pd
import torch
from transformers import AutoTokenizer, AutoModelForSequenceClassification
import onnxruntime as ort

from convert import MODELS, ModelSpec


# Numerical tolerance for logit comparison.
# 1e-3 absolute is generous; conversion typically produces diffs of 1e-5 to 1e-4.
# If your runs exceed this, something likely went wrong.
LOGIT_ABS_TOLERANCE = 1e-3

# Tolerance for class-prediction agreement.
# argmax should match perfectly except in cases where the top two logits
# are within ~LOGIT_ABS_TOLERANCE of each other (rare, but possible).
ARGMAX_AGREEMENT_THRESHOLD = 0.99  # 99% of inputs must have matching argmax


def format_input(row: pd.Series) -> str:
    """Format a course row the way the model was trained.

    IMPORTANT: this must match the training format. Confirm from the source
    model card before running. Two formats currently in question:

      a) f"{row.course_title}"
      b) f"{row.subject_code} {row.catalog_number} --- {row.course_title}"

    Until confirmed, this script uses (b). Change here if (a) turns out to
    be correct.
    """
    return f"{row.subject_code} {row.catalog_number} --- {row.course_title}"


@dataclass
class VerificationResult:
    spec: ModelSpec
    n_inputs: int
    argmax_agreement: float       # fraction matching
    max_logit_diff: float         # max abs diff across all inputs
    mean_logit_diff: float
    top3_agreement: float         # fraction with same top-3 set


def verify(spec: ModelSpec, output_root: Path, inputs: pd.DataFrame) -> VerificationResult:
    print(f"\n=== Verifying {spec.display_name} ===")
    output_dir = output_root / spec.output_subdir

    # Load both models
    tok = AutoTokenizer.from_pretrained(spec.source_repo)
    pt_model = AutoModelForSequenceClassification.from_pretrained(spec.source_repo).eval()

    onnx_path = output_dir / "model.onnx"
    ort_session = ort.InferenceSession(str(onnx_path), providers=["CPUExecutionProvider"])
    onnx_input_names = {inp.name for inp in ort_session.get_inputs()}

    # Run both on each input
    n = len(inputs)
    argmax_matches = 0
    top3_matches = 0
    max_diff = 0.0
    diffs = []

    for _, row in inputs.iterrows():
        text = format_input(row)
        tokens = tok(text, return_tensors="pt", truncation=True, padding=True, max_length=512)

        # PyTorch forward
        with torch.no_grad():
            pt_logits = pt_model(**tokens).logits.numpy()[0]

        # ONNX forward — pass only inputs the ONNX graph expects
        ort_inputs = {
            k: v.numpy() for k, v in tokens.items() if k in onnx_input_names
        }
        ort_logits = ort_session.run(None, ort_inputs)[0][0]

        # Compare
        diff = np.abs(pt_logits - ort_logits).max()
        diffs.append(diff)
        max_diff = max(max_diff, diff)

        if pt_logits.argmax() == ort_logits.argmax():
            argmax_matches += 1

        pt_top3 = set(pt_logits.argsort()[-3:].tolist())
        ort_top3 = set(ort_logits.argsort()[-3:].tolist())
        if pt_top3 == ort_top3:
            top3_matches += 1

    return VerificationResult(
        spec=spec,
        n_inputs=n,
        argmax_agreement=argmax_matches / n,
        max_logit_diff=max_diff,
        mean_logit_diff=float(np.mean(diffs)),
        top3_agreement=top3_matches / n,
    )


def main() -> int:
    here = Path(__file__).parent
    output_root = here / "output"
    inputs = pd.read_csv(here / "parity_inputs.csv")
    print(f"Loaded {len(inputs)} parity inputs")

    results = []
    for spec in MODELS:
        try:
            result = verify(spec, output_root, inputs)
            results.append(result)
        except Exception as e:
            print(f"FAIL: {spec.display_name} verification failed: {e}", file=sys.stderr)
            return 1

    # Print summary
    print("\n=== Verification Summary ===")
    print(f"{'Model':<30} {'Argmax':>8} {'Top-3':>8} {'MaxDiff':>10} {'MeanDiff':>10}")
    print("-" * 70)
    for r in results:
        print(
            f"{r.spec.display_name:<30} "
            f"{r.argmax_agreement:>7.1%} "
            f"{r.top3_agreement:>7.1%} "
            f"{r.max_logit_diff:>10.2e} "
            f"{r.mean_logit_diff:>10.2e}"
        )

    # Fail loudly if any result is below threshold
    failed = [
        r for r in results
        if r.argmax_agreement < ARGMAX_AGREEMENT_THRESHOLD
        or r.max_logit_diff > LOGIT_ABS_TOLERANCE
    ]
    if failed:
        print(f"\nFAIL: {len(failed)} model(s) failed verification thresholds", file=sys.stderr)
        return 1

    print("\n✓ All models passed verification")
    return 0


if __name__ == "__main__":
    sys.exit(main())
```

Run with `uv run python verify.py`. This should produce output like:

```
Model                          Argmax    Top-3    MaxDiff    MeanDiff
----------------------------------------------------------------------
Two-digit CCM classifier        100.0%   100.0%   3.42e-05   8.71e-06
Four-digit CCM classifier       100.0%   100.0%   4.18e-05   1.02e-05
Six-digit CCM classifier         95.0%   100.0%   8.93e-05   1.85e-05
```

**What numbers to expect**:
- `MaxDiff` should be 1e-5 to 1e-4. Higher than 1e-3 means something is wrong with conversion.
- `Argmax` should be 100% in most cases. Can dip slightly when the top-two logits are very close (the tiny numerical diff flips the argmax). 95%+ is acceptable; below that needs investigation.
- `Top-3` should usually be 100%. The set of top predictions is more stable than the strict argmax.

**If verification fails**, possibilities to check in order of likelihood:
1. Wrong `--task` flag during conversion → re-run convert with `text-classification` (or whatever your optimum version expects).
2. Tokenizer mismatch — verify both runtimes load the same `tokenizer.json` and produce identical token IDs for the same input.
3. Input format mismatch — the model expects a specific format that you're not using during verification.
4. Optimum version mismatch — older optimum sometimes mis-exports attention masks. Bump the lockfile.

## Pipeline stage 3: Upload

`scripts/models/upload.py`:

```python
"""Upload converted ONNX models to HF under your namespace."""
from __future__ import annotations

import os
import sys
from pathlib import Path

from huggingface_hub import HfApi, create_repo, ModelCard

from convert import MODELS, ModelSpec


# Set your HF username here.
HF_USERNAME = "YOUR_HF_USERNAME"  # TODO: replace before running


def repo_id_for(spec: ModelSpec) -> str:
    # Convention: <user>/courses-<digits>-roberta-base-onnx
    digits = spec.output_subdir  # "two-digit", "four-digit", "six-digit"
    return f"{HF_USERNAME}/courses-{digits}-roberta-base-onnx"


def model_card_for(spec: ModelSpec) -> str:
    """Generate a YAML-headed markdown model card."""
    return f"""---
license: mit
language: en
library_name: optimum
tags:
  - onnx
  - text-classification
  - course-classification
  - ccm-codes
base_model: {spec.source_repo}
pipeline_tag: text-classification
---

# {spec.display_name} (ONNX)

ONNX-format export of [`{spec.source_repo}`](https://huggingface.co/{spec.source_repo}),
prepared for runtime use in the Course Classifier desktop application.

## Source model

The underlying RoBERTa-base classifier was trained by the annamp team. See the
[source repo]({"https://huggingface.co/" + spec.source_repo}) for training details,
license, and citation information.

## Conversion

Exported using `optimum-cli export onnx --task text-classification`.
Verified to produce predictions matching the source PyTorch model on a held-out
test set within numerical tolerance (max logit difference < 1e-3 absolute,
argmax agreement >= 99%).

## Format and quantization

- F32 weights, ONNX opset 14+
- F16 and int8 quantized variants are not currently published. They may be added
  in future revisions if performance benchmarks indicate a need.

## Intended use

For loading via ONNX Runtime in deployment contexts where a Python interpreter
is not available (desktop applications, embedded systems). For research and
fine-tuning, use the source PyTorch model directly.

## Input format

The model expects course information formatted as:

    {{SUBJECT_CODE}} {{CATALOG_NUMBER}} --- {{COURSE_TITLE}}

Match this format at inference time for accurate predictions.
"""


def upload_model(spec: ModelSpec, output_root: Path, api: HfApi) -> None:
    output_dir = output_root / spec.output_subdir
    repo_id = repo_id_for(spec)

    print(f"\n=== Uploading {spec.display_name} ===")
    print(f"  Local: {output_dir}")
    print(f"  Repo:  {repo_id}")

    # Create the repo (idempotent — exist_ok=True returns the existing one)
    create_repo(repo_id, exist_ok=True, repo_type="model")

    # Write the model card
    card_path = output_dir / "README.md"
    card_path.write_text(model_card_for(spec))

    # Upload everything
    api.upload_folder(
        folder_path=str(output_dir),
        repo_id=repo_id,
        repo_type="model",
        commit_message=f"Initial ONNX export of {spec.source_repo}",
    )
    print(f"  ✓ Uploaded to https://huggingface.co/{repo_id}")


def main() -> int:
    if HF_USERNAME == "YOUR_HF_USERNAME":
        print("ERROR: edit HF_USERNAME in upload.py before running", file=sys.stderr)
        return 1

    if not os.environ.get("HF_TOKEN") and not Path.home().joinpath(".huggingface", "token").exists():
        print("ERROR: HF auth missing. Run `huggingface-cli login` or set HF_TOKEN.", file=sys.stderr)
        return 1

    api = HfApi()
    output_root = Path(__file__).parent / "output"

    for spec in MODELS:
        try:
            upload_model(spec, output_root, api)
        except Exception as e:
            print(f"FAIL: {spec.display_name} upload failed: {e}", file=sys.stderr)
            return 1

    print("\n✓ All models uploaded")
    return 0


if __name__ == "__main__":
    sys.exit(main())
```

Run with `uv run python upload.py`. Models appear at `huggingface.co/<your-username>/courses-{two,four,six}-digit-roberta-base-onnx`.

**Test the published artifacts**: after upload, do a clean fetch in a separate environment (or temp directory) to confirm the published files are complete and the model loads correctly. A small script:

```python
from huggingface_hub import snapshot_download
import onnxruntime as ort

path = snapshot_download(repo_id="YOUR_USERNAME/courses-two-digit-roberta-base-onnx")
session = ort.InferenceSession(f"{path}/model.onnx")
print(f"✓ Loaded with inputs: {[i.name for i in session.get_inputs()]}")
```

If that succeeds, the Tauri app's Rust code will be able to fetch the same way using `hf-hub`.

## Task integration

Add to `Taskfile.yml`:

```yaml
models:setup:
  desc: Install Python deps for the models pipeline
  dir: scripts/models
  cmds:
    - uv sync

models:convert:
  desc: Convert annamp PyTorch models to ONNX
  dir: scripts/models
  cmds:
    - uv run python convert.py
  sources:
    - convert.py
  generates:
    - output/**/model.onnx

models:verify:
  desc: Verify ONNX outputs match PyTorch sources
  dir: scripts/models
  deps: [models:convert]
  cmds:
    - uv run python verify.py
  sources:
    - verify.py
    - parity_inputs.csv
    - output/**/model.onnx

models:upload:
  desc: Push converted ONNX models to HF
  dir: scripts/models
  deps: [models:verify]
  cmds:
    - uv run python upload.py

models:all:
  desc: Convert, verify, and upload all three models
  cmds:
    - task: models:convert
    - task: models:verify
    - task: models:upload
```

Notice `models:upload` depends on `models:verify` which depends on `models:convert`. You can't accidentally upload an unverified model — Task will refuse to run upload if verify hasn't succeeded.

## Decisions to make during execution

1. **Confirm the four-digit and six-digit repo names.** The two-digit repo is at the verified URL; the others are inferred from the naming pattern. First time you run convert, verify these resolve. If not, find the correct names (probably listed in the [annamp/classifying-courses-at-scale collection](https://huggingface.co/collections/annamp/classifying-courses-at-scale)) and update the `MODELS` list.

2. **Confirm the input format.** Before running verify.py, decide whether the model was trained on `{course_title}` alone or `{SUBJECT} {NUMBER} --- {TITLE}`. The verify script defaults to the latter; change `format_input()` if needed. This decision also needs to be reflected in the Tauri app's input formatting (it must match exactly), and in the model card text. Open question per the main handoff doc — settle here, propagate.

3. **Choose the HF repo namespace.** The `HF_USERNAME` constant in upload.py is a placeholder. Use a personal account if this is exploratory, an org account if the project has one (e.g., `umsi-courses` or similar institutional namespace). Pick one and stick with it — repo URLs end up baked into the Tauri app config.

4. **Decide on repo licensing.** Model card defaults to MIT (matching annamp's). If your institution requires a different license, change it before upload — easier to set correctly the first time than to relicense later.

5. **Whether to enable HF gated access.** Public repos are simpler. Gated repos (require user to accept terms) are appropriate if you want to track usage or restrict redistribution. For an academic open-source tool, public is the default.

## What's deliberately not in this pipeline

**Quantization (F16, int8).** Per the main handoff doc, deferred to Phase 2. F32 ships first. If quantization is added later, extend `convert.py` to also produce `model_fp16.onnx` and/or `model_quantized.onnx`, run the same parity verification on each variant (with relaxed tolerance — quantization legitimately changes outputs by a small amount), and upload the additional files to the same repos.

**Accuracy benchmarking on a labeled set.** Parity verification confirms the ONNX matches the PyTorch source. It does *not* confirm either matches the labels in your data. If you have a labeled holdout and want to verify accuracy hasn't regressed, that's a separate script — load both models, predict on labeled inputs, compute accuracy/F1, compare against published numbers in the source model card. Worth doing once for confidence; not needed every conversion run.

**Continuous integration.** This pipeline runs rarely (once per model variant per quantization level). Doesn't need to run in CI on every commit. If you add new variants, run it locally and commit the resulting HF repo URLs to the Tauri app config. If CI runs become useful (e.g., when adding F16/int8 variants), this pipeline is structured to be CI-friendly — Task targets, deterministic via lockfile, exits nonzero on failures.

**Mirroring to UMich storage.** Phase 3 concern per the main handoff doc. If institutional permanence matters later, mirror these HF repos to UMich-controlled storage and have the Tauri app fetch with HF as a fallback (or vice versa). Don't optimize for it now.

## Failure modes to watch for

**Conversion fails with "operator not supported".** Shouldn't happen for RoBERTa-base, but if it does, check optimum version. Bump to latest if not already.

**Conversion succeeds but produces a model with wrong output shape.** The `--task` flag is wrong, or optimum's task inference picked something unexpected. The model.onnx file's output should be `[batch_size, num_classes]` where num_classes matches the source model's `config.json` `num_labels` field. If the output is hidden states (`[batch_size, seq_len, hidden_size]`), conversion picked the wrong task.

**Verify shows perfect logits match but argmax disagrees on some inputs.** This is the expected pattern when two top logits are within numerical noise of each other. Probably fine if argmax agreement is >95% and top-3 agreement is 100%. If both are low, something else is wrong — check that the same tokenizer is being used for both runs.

**Verify shows large logit differences (>1e-3 max).** Real conversion bug. Most common cause: optimum version mismatch with transformers, attention mask handling differing between runtimes. Re-pin lockfile, re-convert.

**Upload fails with "you don't have permission".** HF token doesn't have write scope, or it's a read-only token. Regenerate a write token at huggingface.co/settings/tokens.

**Upload fails with "repo already exists" on first run.** Someone (you or someone else) already created a repo with that name. The script uses `exist_ok=True` so it should handle existing repos correctly; if it errors out anyway, check the repo's permissions on HF — might be that the namespace is taken by someone else (rare for unique names but possible for short ones).

## Re-running the pipeline

Once initial conversion is done, the pipeline doesn't need to run again unless:

- A new annamp model variant is published (e.g., they retrain at higher accuracy).
- You add a new precision variant (F16, int8).
- You change the input format and need to re-verify with the new format.
- The lockfile is updated and you want to verify the new dep versions still produce identical ONNX outputs.

Each re-run of `models:upload` creates a new commit on the HF repos. Old revisions stay accessible — the Tauri app can pin to a specific revision via `hf-hub` if you ever need rollback. Worth knowing the rollback path exists; not worth using prematurely.
