"""Measure CIP/CCM taxonomy overlap on labeled panel data.

Loads validation.csv, filters to clean rows, optionally samples, dedups by
content hash, runs each model on the unique inputs, broadcasts predictions
back to original rows, and reports the rate at which model output (CCM)
agrees with the panel's `inventory_cip_*` columns (federal CIP).

CIP and CCM are *distinct taxonomies* that overlap heavily at the broad
2-digit level and diverge as specificity increases. The reported rate is
not model accuracy — it's a taxonomy-overlap measurement, useful for
sanity-checking the pipeline end-to-end. The meaningful correctness check
is parity (Rust ONNX == Python ONNX == annamp PyTorch), covered by
verify.py.

Defaults to a 10K random sample. Use `--size=full` for the whole panel.
GPU is used automatically when available; CPU is the documented fallback.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import sys
import time
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

import numpy as np
import pandas as pd
from transformers import AutoConfig, AutoTokenizer

from _lib.format import CourseInput, format_input
from _lib.inference import load_session, predict_batch, select_providers
from _lib.models import MODELS, ModelSpec
from _lib.reporting import render_validation_report, utc_now_iso

HERE = Path(__file__).parent
REPO_ROOT = HERE.parent.parent
OUTPUT_ROOT = HERE / "output"
REPORTS_ROOT = HERE / "reports"
DEFAULT_VALIDATION_CSV = REPO_ROOT / "data" / "validation.csv"

# Panel CSV columns
COL_SUBJECT = "sub_pref"
COL_CATALOG = "course"
COL_TITLE = "inventory_course_title"
COL_MULTIPLE = "Multiple Course?"
COL_APPROVAL = "inventory_approval"

SAMPLE_SIZES = {"10k": 10_000, "100k": 100_000, "1m": 1_000_000, "full": None}


@dataclass
class ValidationResult:
    display_name: str
    digit_level: int
    n_unique: int
    n_rows: int
    n_skipped: int
    matched: int            # rows where panel CIP matched model CCM at this digit level
    overlap_rate: float     # matched / n_rows
    latency_p50_ms: float
    latency_p95_ms: float
    label_column: str
    actual_provider: str


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--size", default="10k", choices=list(SAMPLE_SIZES.keys()))
    p.add_argument("--seed", type=int, default=42)
    p.add_argument("--batch-size", type=int, default=64)
    p.add_argument("--csv", default=None, help="path to validation csv (overrides env + default)")
    return p.parse_args()


def resolve_csv_path(arg_path: str | None) -> Path:
    if arg_path:
        return Path(arg_path)
    env = os.environ.get("COURSE_CLASSIFIER_VALIDATION_CSV")
    if env:
        return Path(env)
    return DEFAULT_VALIDATION_CSV


def load_and_filter(csv_path: Path) -> pd.DataFrame:
    print(f"Loading {csv_path} ...")
    df = pd.read_csv(csv_path, dtype=str, low_memory=False)
    print(f"  {len(df):,} rows raw")

    label_cols = [m.panel_label_column for m in MODELS]
    required = [COL_SUBJECT, COL_CATALOG, COL_TITLE] + label_cols
    missing = [c for c in required if c not in df.columns]
    if missing:
        raise RuntimeError(f"validation.csv missing required columns: {missing}")

    df = df.dropna(subset=required)
    if COL_MULTIPLE in df.columns:
        flagged = df[COL_MULTIPLE].astype(str).str.strip().str.lower().isin(
            {"y", "yes", "1", "true"}
        )
        df = df[~flagged]
    print(f"  {len(df):,} rows after filter")
    return df.reset_index(drop=True)


def maybe_sample(df: pd.DataFrame, size: int | None, seed: int) -> pd.DataFrame:
    if size is None or size >= len(df):
        return df
    return df.sample(n=size, random_state=seed).reset_index(drop=True)


def hash_input(text: str) -> str:
    return hashlib.blake2b(text.encode("utf-8"), digest_size=16).hexdigest()


def normalize_ccm(code: str, digit_level: int) -> float | None:
    """Canonicalize a hierarchical code (CIP or CCM) as a float for comparison.

    Used to compare panel CIP labels against model CCM predictions on a
    common numeric scale. CIP and CCM share the 2/4/6-digit hierarchical
    encoding even though they're distinct taxonomies — the float form is
    equality-comparable when (and only when) the two systems happen to
    agree at that level of specificity.

    Panel CIP format: bare digits, no period, leading zeros sometimes stripped
        e.g., '52', '5210', '521005', '105' (= 01.05)
    Model CCM format: dotted, leading and trailing zeros sometimes stripped
        e.g., '52', '52.1', '52.1005', '1.0' (= 01.00)

    Returns None for empty / NaN / unparseable.
    """
    s = str(code).strip()
    if not s or s.lower() in {"nan", "none"}:
        return None
    if "." in s:
        try:
            return float(s)
        except ValueError:
            return None
    # Bare digits — divide by 10^suffix_width to recover the implicit decimal.
    suffix_width = digit_level - 2
    try:
        n = int(s)
    except ValueError:
        return None
    return n / (10 ** suffix_width) if suffix_width > 0 else float(n)


def codes_match(a: str, b: str, digit_level: int) -> bool:
    """True if the two codes (one CIP, one CCM, or any pair) agree numerically."""
    fa = normalize_ccm(a, digit_level)
    fb = normalize_ccm(b, digit_level)
    if fa is None or fb is None:
        return False
    return math.isclose(fa, fb, abs_tol=1e-9)


def build_inputs(df: pd.DataFrame) -> tuple[dict[str, str], list[str]]:
    """Return (inputs_by_hash, hashes_per_row)."""
    formatted = [
        format_input(CourseInput(
            subject_code=str(s),
            catalog_number=str(c),
            course_title=str(t),
        ))
        for s, c, t in zip(df[COL_SUBJECT], df[COL_CATALOG], df[COL_TITLE])
    ]
    hashes_per_row = [hash_input(t) for t in formatted]
    inputs_by_hash: dict[str, str] = {}
    for h, t in zip(hashes_per_row, formatted):
        if h not in inputs_by_hash:
            inputs_by_hash[h] = t
    return inputs_by_hash, hashes_per_row


def validate_one(
    spec: ModelSpec,
    df: pd.DataFrame,
    inputs_by_hash: dict[str, str],
    hashes_per_row: list[str],
    batch_size: int,
    providers: list[str],
) -> tuple[ValidationResult, list[dict[str, Any]]]:
    print(f"\n=== Validating {spec.display_name} ===")
    onnx_path = OUTPUT_ROOT / spec.output_subdir / "model.onnx"
    session, actual_provider = load_session(onnx_path, providers=providers)
    print(f"  provider: {actual_provider}")

    tokenizer = AutoTokenizer.from_pretrained(spec.source_repo)
    config = AutoConfig.from_pretrained(spec.source_repo)
    id2label: dict[int, str] = {int(k): str(v) for k, v in config.id2label.items()}

    unique_hashes = list(inputs_by_hash.keys())
    print(f"  {len(unique_hashes):,} unique inputs (from {len(df):,} rows)")

    pred_by_hash: dict[str, str] = {}
    latencies: list[float] = []

    for i in range(0, len(unique_hashes), batch_size):
        batch_hashes = unique_hashes[i:i + batch_size]
        batch_texts = [inputs_by_hash[h] for h in batch_hashes]
        t0 = time.perf_counter()
        logits = predict_batch(session, tokenizer, batch_texts)
        elapsed_ms = (time.perf_counter() - t0) * 1000.0
        latencies.extend([elapsed_ms / len(batch_texts)] * len(batch_texts))
        argmax = logits.argmax(axis=1)
        for h, idx in zip(batch_hashes, argmax):
            pred_by_hash[h] = id2label[int(idx)].strip()

        if (i // batch_size) % 50 == 0 and i > 0:
            print(f"  {i:,}/{len(unique_hashes):,}")

    matched = 0
    n_skipped = 0
    disagreements: list[dict[str, Any]] = []
    label_col = spec.panel_label_column
    truth_series = df[label_col].astype(str).str.strip()

    for row_i, h in enumerate(hashes_per_row):
        truth = truth_series.iloc[row_i]
        if not truth or truth.lower() in {"nan", "none", ""}:
            n_skipped += 1
            continue
        pred = pred_by_hash[h]
        if codes_match(pred, truth, spec.digit_level):
            matched += 1
        else:
            disagreements.append({
                "model": spec.display_name,
                "row": row_i,
                "subject_code": df.iloc[row_i][COL_SUBJECT],
                "catalog_number": df.iloc[row_i][COL_CATALOG],
                "course_title": df.iloc[row_i][COL_TITLE],
                "predicted_ccm": pred,
                "panel_cip": truth,
                "predicted_normalized": normalize_ccm(pred, spec.digit_level),
                "panel_normalized": normalize_ccm(truth, spec.digit_level),
            })

    n_compared = len(df) - n_skipped
    overlap = matched / n_compared if n_compared > 0 else 0.0

    return ValidationResult(
        display_name=spec.display_name,
        digit_level=spec.digit_level,
        n_unique=len(unique_hashes),
        n_rows=n_compared,
        n_skipped=n_skipped,
        matched=matched,
        overlap_rate=overlap,
        latency_p50_ms=float(np.percentile(latencies, 50)),
        latency_p95_ms=float(np.percentile(latencies, 95)),
        label_column=label_col,
        actual_provider=actual_provider,
    ), disagreements


def main() -> int:
    args = parse_args()
    csv_path = resolve_csv_path(args.csv)
    if not csv_path.exists():
        print(
            f"FAIL: validation CSV not found at {csv_path}\n"
            f"      set COURSE_CLASSIFIER_VALIDATION_CSV or place file at default",
            file=sys.stderr,
        )
        return 1

    df = load_and_filter(csv_path)
    df = maybe_sample(df, SAMPLE_SIZES[args.size], args.seed)
    print(f"  {len(df):,} rows after sampling ({args.size})")

    print("Building inputs and dedup hash ...")
    inputs_by_hash, hashes_per_row = build_inputs(df)
    print(f"  {len(inputs_by_hash):,} unique / {len(hashes_per_row):,} total "
          f"= {1 - len(inputs_by_hash)/len(hashes_per_row):.1%} dedup")

    providers = select_providers()
    print(f"\nProviders (preferred order): {providers}")

    timestamp = utc_now_iso().replace(":", "-").replace("Z", "")
    run_dir = OUTPUT_ROOT / "validation" / timestamp
    run_dir.mkdir(parents=True, exist_ok=True)

    results: list[ValidationResult] = []
    all_disagreements: list[dict[str, Any]] = []

    for spec in MODELS:
        try:
            result, disagreements = validate_one(
                spec, df, inputs_by_hash, hashes_per_row, args.batch_size, providers,
            )
        except Exception as e:
            print(f"FAIL: {spec.display_name}: {e}", file=sys.stderr)
            import traceback
            traceback.print_exc()
            return 1
        results.append(result)
        all_disagreements.extend(disagreements)

    print("\n=== CIP/CCM Overlap Summary (not model accuracy) ===")
    print(
        f"{'Model':<25} {'Unique':>10} {'Rows':>10} {'Overlap':>9} "
        f"{'p50 ms':>10} {'p95 ms':>10}"
    )
    print("-" * 80)
    for r in results:
        print(
            f"{r.display_name:<25} {r.n_unique:>10,} {r.n_rows:>10,} "
            f"{r.overlap_rate:>8.1%} {r.latency_p50_ms:>10.2f} {r.latency_p95_ms:>10.2f}"
        )

    actual_providers = sorted({r.actual_provider for r in results})
    summary = {
        "generated_at": utc_now_iso(),
        "sample_size": len(df),
        "sample_mode": args.size,
        "format": "B (model card spec)",
        "execution_provider": ", ".join(actual_providers),
        "preferred_providers": providers,
        "source_csv": str(csv_path),
        "filter": "non-null required fields, exclude Multiple Course?",
        "results": [asdict(r) for r in results],
    }
    (run_dir / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")

    if all_disagreements:
        pd.DataFrame(all_disagreements).to_csv(run_dir / "disagreements.csv", index=False)

    REPORTS_ROOT.mkdir(parents=True, exist_ok=True)
    md = render_validation_report([asdict(r) for r in results], summary)
    (REPORTS_ROOT / "validation-latest.md").write_text(md)

    latest = OUTPUT_ROOT / "validation" / "latest"
    if latest.is_symlink() or latest.exists():
        latest.unlink()
    latest.symlink_to(timestamp, target_is_directory=True)

    print(f"\nResults: {run_dir}")
    print(f"Report:  {REPORTS_ROOT / 'validation-latest.md'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
