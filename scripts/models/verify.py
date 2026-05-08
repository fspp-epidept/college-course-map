"""Verify ONNX exports match PyTorch sources within numerical tolerance.

Runs both backends on the small synthetic parity corpus and reports
argmax / top-3 agreement plus max/mean logit diff. Fails if any model
falls below threshold.
"""
from __future__ import annotations

import json
import sys
from dataclasses import asdict, dataclass
from pathlib import Path

import numpy as np
import pandas as pd
import torch
from transformers import AutoModelForSequenceClassification, AutoTokenizer

from _lib.format import CourseInput, format_input
from _lib.inference import load_session, predict_batch
from _lib.models import MODELS, ModelSpec
from _lib.reporting import render_parity_report

HERE = Path(__file__).parent
OUTPUT_ROOT = HERE / "output"
PARITY_CSV = HERE / "data" / "parity_inputs.csv"
REPORTS_ROOT = HERE / "reports"
RESULTS_ROOT = OUTPUT_ROOT / "parity"

LOGIT_ABS_TOLERANCE = 1e-3
ARGMAX_AGREEMENT_THRESHOLD = 0.95


@dataclass
class ParityResult:
    display_name: str
    source_repo: str
    n_inputs: int
    argmax_agreement: float
    top3_agreement: float
    max_logit_diff: float
    mean_logit_diff: float
    passed: bool


def verify_one(
    spec: ModelSpec,
    inputs: list[str],
    per_input: list[dict],
) -> ParityResult:
    print(f"\n=== Verifying {spec.display_name} ===")
    onnx_path = OUTPUT_ROOT / spec.output_subdir / "model.onnx"

    tokenizer = AutoTokenizer.from_pretrained(spec.source_repo)
    pt_model = AutoModelForSequenceClassification.from_pretrained(spec.source_repo)
    # Switch to inference mode (equivalent to .eval(); avoids dropout etc.)
    pt_model.train(False)
    # CPU EP for parity — deterministic, matches PyTorch default device
    session, _ = load_session(onnx_path, providers=["CPUExecutionProvider"])

    argmax_matches = 0
    top3_matches = 0
    diffs: list[float] = []

    for text in inputs:
        encoded = tokenizer(text, return_tensors="pt", truncation=True, padding=True, max_length=512)
        with torch.no_grad():
            pt_logits = pt_model(**encoded).logits.numpy()[0]

        ort_logits = predict_batch(session, tokenizer, [text])[0]

        diff = float(np.abs(pt_logits - ort_logits).max())
        diffs.append(diff)

        if pt_logits.argmax() == ort_logits.argmax():
            argmax_matches += 1

        pt_top3 = set(pt_logits.argsort()[-3:].tolist())
        ort_top3 = set(ort_logits.argsort()[-3:].tolist())
        if pt_top3 == ort_top3:
            top3_matches += 1

        # Record per-input ONNX outputs so the Rust spike can assert byte-identical
        # behaviour from its own ORT session against this fixture.
        per_input.append({
            "model_subdir": spec.output_subdir,
            "input": text,
            "argmax": int(ort_logits.argmax()),
            "top3": [int(x) for x in ort_logits.argsort()[-3:][::-1].tolist()],
            "logit_argmax_value": float(ort_logits.max()),
        })

    n = len(inputs)
    argmax_agree = argmax_matches / n
    top3_agree = top3_matches / n
    max_diff = max(diffs)
    mean_diff = float(np.mean(diffs))
    passed = argmax_agree >= ARGMAX_AGREEMENT_THRESHOLD and max_diff <= LOGIT_ABS_TOLERANCE

    print(
        f"  argmax={argmax_agree:.1%}  top3={top3_agree:.1%}  "
        f"max={max_diff:.2e}  mean={mean_diff:.2e}  {'PASS' if passed else 'FAIL'}"
    )

    return ParityResult(
        display_name=spec.display_name,
        source_repo=spec.source_repo,
        n_inputs=n,
        argmax_agreement=argmax_agree,
        top3_agreement=top3_agree,
        max_logit_diff=max_diff,
        mean_logit_diff=mean_diff,
        passed=passed,
    )


def main() -> int:
    if not PARITY_CSV.exists():
        print(f"FAIL: {PARITY_CSV} missing", file=sys.stderr)
        return 1

    df = pd.read_csv(PARITY_CSV)
    print(f"Loaded {len(df)} parity inputs from {PARITY_CSV.name}")
    inputs = [
        format_input(CourseInput(
            subject_code=str(row.subject_code),
            catalog_number=str(row.catalog_number),
            course_title=str(row.course_title),
        ))
        for row in df.itertuples()
    ]

    RESULTS_ROOT.mkdir(parents=True, exist_ok=True)
    REPORTS_ROOT.mkdir(parents=True, exist_ok=True)

    results: list[ParityResult] = []
    per_input: list[dict] = []
    for spec in MODELS:
        try:
            results.append(verify_one(spec, inputs, per_input))
        except Exception as e:
            print(f"FAIL: {spec.display_name}: {e}", file=sys.stderr)
            import traceback
            traceback.print_exc()
            return 1

    print("\n=== Summary ===")
    print(f"{'Model':<25} {'Argmax':>8} {'Top-3':>8} {'MaxDiff':>11} {'MeanDiff':>11}  Pass")
    print("-" * 72)
    for r in results:
        print(
            f"{r.display_name:<25} {r.argmax_agreement:>7.1%} {r.top3_agreement:>7.1%} "
            f"{r.max_logit_diff:>11.2e} {r.mean_logit_diff:>11.2e}  "
            f"{'PASS' if r.passed else 'FAIL'}"
        )

    json_path = RESULTS_ROOT / "summary.json"
    json_path.write_text(json.dumps([asdict(r) for r in results], indent=2) + "\n")

    per_input_path = RESULTS_ROOT / "per_input.json"
    per_input_path.write_text(json.dumps(per_input, indent=2) + "\n")

    md_path = REPORTS_ROOT / "parity-latest.md"
    md_path.write_text(render_parity_report([asdict(r) for r in results]))

    print(f"\nResults:    {json_path}")
    print(f"Per-input:  {per_input_path}")
    print(f"Report:     {md_path}")

    failed = [r for r in results if not r.passed]
    if failed:
        print(f"\nFAIL: {len(failed)} model(s) below threshold", file=sys.stderr)
        return 1
    print("\nAll models passed parity")
    return 0


if __name__ == "__main__":
    sys.exit(main())
