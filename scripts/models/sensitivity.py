"""Measure how much input malformations move the models' predictions.

Samples unique (subject, catalog, title) rows from the labeled panel, runs the
app-active ONNX models on the correctly assembled string and on perturbed
variants of it (subject repeated in the catalog number, `101.0`, lowercase,
missing fields, ...), and reports per variant and model:

- flip rate: share of rows whose top-1 prediction differs from the baseline
- mean top-1 probability: whether confidence reacts to the damage (it does not)
- CIP agreement: share of rows whose prediction matches the panel's
  `inventory_cip_*` label, canonicalized the same way validate.py does

CIP agreement is a directional proxy, not accuracy: the panel carries federal
CIP codes and the models output CCM codes (see validate.py). The flip rate is
the primary number. The results feed `docs/input-contract.md` and justify the
import-time checks in `src-tauri/src/profile.rs`.

Defaults to a 1k sample of the two- and six-digit models. GPU is used when
available; CPU is the fallback.
"""
from __future__ import annotations

import argparse
import json
import random
import sys
import time
from pathlib import Path
from typing import Any, Callable

import numpy as np
from transformers import AutoConfig, AutoTokenizer

from _lib.format import CourseInput, format_input
from _lib.inference import load_session, predict_batch, select_providers
from _lib.models import APP_ACTIVE, ModelSpec
from _lib.reporting import render_sensitivity_report, utc_now_iso
from validate import (
    COL_CATALOG,
    COL_SUBJECT,
    COL_TITLE,
    OUTPUT_ROOT,
    REPO_ROOT,
    REPORTS_ROOT,
    codes_match,
    load_and_filter,
    resolve_csv_path,
)

SAMPLE_SIZES = {"1k": 1_000, "5k": 5_000, "20k": 20_000}
LEVELS = {"two-digit": 2, "four-digit": 4, "six-digit": 6}

Perturb = Callable[[str, str, str], CourseInput]


def course(s: str, c: str, t: str) -> CourseInput:
    return CourseInput(subject_code=s, catalog_number=c, course_title=t)


# Each variant rebuilds the three fields from a correctly formatted row, then
# the string is assembled by the real formatter. Keys are report labels.
VARIANTS: dict[str, Perturb] = {
    "baseline": lambda s, c, t: course(s, c, t),
    "subject repeated in catalog (`PSYC 4325`)": lambda s, c, t: course(s, f"{s} {c}", t),
    "subject repeated in catalog, no space (`PSYC4325`)": lambda s, c, t: course(s, f"{s}{c}", t),
    "full code in both subject and catalog": lambda s, c, t: course(f"{s} {c}", f"{s} {c}", t),
    "catalog as converted number (`4325.0`)": lambda s, c, t: course(s, f"{c}.0", t),
    "catalog letters stripped (`4304L` -> `4304`)": lambda s, c, t: course(
        s, "".join(ch for ch in c if ch.isdigit()), t
    ),
    "catalog missing": lambda s, c, t: course(s, "", t),
    "subject missing": lambda s, c, t: course("", c, t),
    "title missing": lambda s, c, t: course(s, c, ""),
    "title only": lambda s, c, t: course("", "", t),
    "subject and catalog swapped": lambda s, c, t: course(c, s, t),
    "title in the catalog column": lambda s, c, t: course(s, t, c),
    "catalog is a year (`2024`)": lambda s, c, t: course(s, "2024", t),
    "subject lowercased": lambda s, c, t: course(s.lower(), c, t),
    "title in Title Case": lambda s, c, t: course(s, c, t.title()),
    "title lowercased": lambda s, c, t: course(s, c, t.lower()),
    "trailing whitespace kept": lambda s, c, t: course(f"{s}  ", f"{c}   ", f"{t}    "),
    "title prefixed with the code": lambda s, c, t: course(s, c, f"{s} {c} {t}"),
}


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--size", default="1k", choices=list(SAMPLE_SIZES))
    p.add_argument("--seed", type=int, default=114)
    p.add_argument("--batch-size", type=int, default=128)
    p.add_argument("--csv", default=None, help="path to validation csv (overrides env + default)")
    p.add_argument(
        "--levels",
        nargs="+",
        default=["two-digit", "six-digit"],
        choices=list(LEVELS),
        help="models to run (default: two-digit six-digit)",
    )
    return p.parse_args()


def sample_unique_rows(csv_path: Path, size: int, seed: int) -> list[dict[str, str]]:
    """Distinct (subject, catalog, title) triples with their panel labels."""
    df = load_and_filter(csv_path)
    seen: set[tuple[str, str, str]] = set()
    rows: list[dict[str, str]] = []
    for rec in df.to_dict("records"):
        key = (str(rec[COL_SUBJECT]).strip(), str(rec[COL_CATALOG]).strip(), str(rec[COL_TITLE]).strip())
        if not all(key) or key in seen:
            continue
        seen.add(key)
        rows.append({
            "subject": key[0],
            "catalog": key[1],
            "title": key[2],
            **{spec.panel_label_column: str(rec[spec.panel_label_column]).strip() for spec in APP_ACTIVE},
        })
    print(f"  {len(rows):,} unique inputs; sampling {size:,}")
    rng = random.Random(seed)
    return rng.sample(rows, min(size, len(rows)))


def softmax_top1(logits: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    z = logits - logits.max(axis=1, keepdims=True)
    p = np.exp(z)
    p /= p.sum(axis=1, keepdims=True)
    return p.argmax(axis=1), p.max(axis=1)


def predict(session, tokenizer, texts: list[str], batch_size: int) -> tuple[np.ndarray, np.ndarray]:
    preds: list[np.ndarray] = []
    probs: list[np.ndarray] = []
    for i in range(0, len(texts), batch_size):
        top, prob = softmax_top1(predict_batch(session, tokenizer, texts[i:i + batch_size]))
        preds.append(top)
        probs.append(prob)
    return np.concatenate(preds), np.concatenate(probs)


def run_level(
    spec: ModelSpec,
    rows: list[dict[str, str]],
    batch_size: int,
    providers: list[str],
) -> tuple[list[dict[str, Any]], str]:
    print(f"\n=== {spec.display_name} ===")
    model_dir = OUTPUT_ROOT / spec.output_subdir
    session, provider = load_session(model_dir / "model.onnx", providers=providers)
    print(f"  provider: {provider}")
    tokenizer = AutoTokenizer.from_pretrained(model_dir)
    config = AutoConfig.from_pretrained(model_dir)
    id2label = {int(k): str(v).strip() for k, v in config.id2label.items()}
    truths = [r[spec.panel_label_column] for r in rows]

    results: list[dict[str, Any]] = []
    base_pred: np.ndarray | None = None
    for name, perturb in VARIANTS.items():
        t0 = time.perf_counter()
        texts = [format_input(perturb(r["subject"], r["catalog"], r["title"])) for r in rows]
        pred, prob = predict(session, tokenizer, texts, batch_size)
        if base_pred is None:
            base_pred = pred
        flip_rate = float((pred != base_pred).mean())
        agree = sum(codes_match(id2label[int(p)], truth, spec.digit_level) for p, truth in zip(pred, truths))
        results.append({
            "variant": name,
            "flip_rate": flip_rate,
            "mean_top1_prob": float(prob.mean()),
            "cip_agreement": agree / len(rows),
        })
        print(
            f"  {name:<52} flip={flip_rate:6.1%}  mean_p={prob.mean():.3f}"
            f"  cip={agree / len(rows):.3f}  ({time.perf_counter() - t0:.1f}s)"
        )
    return results, provider


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

    rows = sample_unique_rows(csv_path, SAMPLE_SIZES[args.size], args.seed)
    providers = select_providers()
    print(f"\nProviders (preferred order): {providers}")

    specs = [next(s for s in APP_ACTIVE if s.digit_level == LEVELS[level]) for level in args.levels]
    per_level: list[dict[str, Any]] = []
    actual_providers: set[str] = set()
    for spec in specs:
        results, provider = run_level(spec, rows, args.batch_size, providers)
        actual_providers.add(provider)
        per_level.append({
            "display_name": spec.display_name,
            "digit_level": spec.digit_level,
            "label_column": spec.panel_label_column,
            "results": results,
        })

    summary = {
        "generated_at": utc_now_iso(),
        "sample_size": len(rows),
        "sample_mode": args.size,
        "seed": args.seed,
        "execution_provider": ", ".join(sorted(actual_providers)),
        "preferred_providers": providers,
        # Relative to the repo root so committed reports carry no local paths.
        "source_csv": str(
            csv_path.relative_to(REPO_ROOT) if csv_path.is_relative_to(REPO_ROOT) else csv_path.name
        ),
        "levels": per_level,
    }
    run_dir = OUTPUT_ROOT / "sensitivity" / utc_now_iso().replace(":", "-").replace("Z", "")
    run_dir.mkdir(parents=True, exist_ok=True)
    (run_dir / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")

    REPORTS_ROOT.mkdir(parents=True, exist_ok=True)
    report = REPORTS_ROOT / "sensitivity-latest.md"
    report.write_text(render_sensitivity_report(summary))

    print(f"\nResults: {run_dir}")
    print(f"Report:  {report}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
