"""Cross-check: run an HF source PyTorch model directly on the same panel
sample validate.py uses, to confirm whether the observed CIP/CCM overlap
rate is intrinsic to the source PyTorch model or an artifact of our ONNX
pipeline. (Confirmed: PyTorch and ONNX argmax-agree.)

Default: six-digit model, 10k sample, same seed as validate.py defaults.
PyTorch will use CUDA if available — much faster than ONNX-on-CPU here.
"""
from __future__ import annotations

import argparse
import sys
import time

import torch
from transformers import AutoConfig, AutoModelForSequenceClassification, AutoTokenizer

from _lib.format import CourseInput, format_input
from _lib.models import MODELS
from validate import (
    COL_CATALOG,
    COL_SUBJECT,
    COL_TITLE,
    SAMPLE_SIZES,
    codes_match,
    load_and_filter,
    maybe_sample,
    resolve_csv_path,
)


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--digit", type=int, default=6, choices=[2, 4, 6])
    p.add_argument("--size", default="10k", choices=list(SAMPLE_SIZES.keys()))
    p.add_argument("--seed", type=int, default=42)
    p.add_argument("--batch-size", type=int, default=32)
    p.add_argument("--csv", default=None)
    args = p.parse_args()

    spec = next(m for m in MODELS if m.digit_level == args.digit)
    csv_path = resolve_csv_path(args.csv)
    df = load_and_filter(csv_path)
    df = maybe_sample(df, SAMPLE_SIZES[args.size], args.seed)
    print(f"Sample: {len(df):,} rows ({args.size})")

    inputs = [
        format_input(CourseInput(
            subject_code=str(s),
            catalog_number=str(c),
            course_title=str(t),
        ))
        for s, c, t in zip(df[COL_SUBJECT], df[COL_CATALOG], df[COL_TITLE])
    ]
    truths = df[spec.panel_label_column].astype(str).str.strip().tolist()

    device = "cuda" if torch.cuda.is_available() else "cpu"
    print(f"Running {spec.display_name} (PyTorch) on {device}")

    tokenizer = AutoTokenizer.from_pretrained(spec.source_repo)
    model = AutoModelForSequenceClassification.from_pretrained(spec.source_repo)
    model.train(False)
    model = model.to(device)
    config = AutoConfig.from_pretrained(spec.source_repo)
    id2label = {int(k): str(v).strip() for k, v in config.id2label.items()}

    matched = 0
    n_compared = 0
    t0 = time.perf_counter()

    with torch.no_grad():
        for i in range(0, len(inputs), args.batch_size):
            batch_texts = inputs[i:i + args.batch_size]
            batch_truths = truths[i:i + args.batch_size]
            encoded = tokenizer(
                batch_texts,
                return_tensors="pt",
                truncation=True,
                padding=True,
                max_length=512,
            )
            encoded = {k: v.to(device) for k, v in encoded.items()}
            logits = model(**encoded).logits
            argmax = logits.argmax(dim=1).cpu().tolist()
            for idx, truth in zip(argmax, batch_truths):
                if not truth or truth.lower() in {"nan", "none", ""}:
                    continue
                pred = id2label[int(idx)]
                if codes_match(pred, truth, args.digit):
                    matched += 1
                n_compared += 1
            if (i // args.batch_size) % 100 == 0 and i > 0:
                print(f"  {i:,}/{len(inputs):,}")

    elapsed = time.perf_counter() - t0
    overlap = matched / n_compared if n_compared else 0.0
    print()
    print(f"=== {spec.display_name} (PyTorch / {device}) ===")
    print(f"  rows compared:   {n_compared:,} / {len(df):,}")
    print(f"  CIP/CCM overlap: {overlap:.1%}")
    print(f"  elapsed:         {elapsed:.1f}s")
    return 0


if __name__ == "__main__":
    sys.exit(main())
