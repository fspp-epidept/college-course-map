"""Convert annamp PyTorch models to ONNX via optimum-cli, then apply the
fp32 Neg→Mul export pass (`_lib/neg_rewrite.py`).

Idempotent: if `output/<dir>/model.onnx` already exists and is non-empty,
the export is skipped; the Neg→Mul pass still runs, and is itself a no-op
once its `metadata_props` stamp is present. Wipe `output/` to force a
fresh conversion.
"""
from __future__ import annotations

import json
import shutil
import subprocess
import sys
from pathlib import Path

import numpy as np
import pandas as pd
from tokenizers import Tokenizer

from _lib import neg_rewrite
from _lib.format import CourseInput, export_spec, format_input
from _lib.models import MODELS, ModelSpec

HERE = Path(__file__).parent
OUTPUT_ROOT = HERE / "output"
PARITY_CSV = HERE / "data" / "parity_inputs.csv"


def export_one(spec: ModelSpec) -> Path:
    out_dir = OUTPUT_ROOT / spec.output_subdir
    onnx_path = out_dir / "model.onnx"

    if onnx_path.exists() and onnx_path.stat().st_size > 0:
        print(f"  skip {spec.display_name}: already converted ({onnx_path.stat().st_size / 1e6:.1f} MB)")
        return out_dir

    if out_dir.exists():
        shutil.rmtree(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    print(f"\n=== Converting {spec.display_name} ===")
    print(f"  source: {spec.source_repo}")
    print(f"  output: {out_dir}")

    cmd = [
        "optimum-cli", "export", "onnx",
        "--model", spec.source_repo,
        "--task", "text-classification",
        str(out_dir),
    ]
    subprocess.run(cmd, check=True)

    if not onnx_path.exists():
        raise RuntimeError(f"Conversion succeeded but {onnx_path} missing")
    print(f"  → model.onnx: {onnx_path.stat().st_size / 1e6:.1f} MB")
    return out_dir


def fixture_feeds(out_dir: Path, texts: list[str]) -> list[dict[str, np.ndarray]]:
    """Tokenize the parity corpus one input at a time (no padding), as the app does."""
    tokenizer = Tokenizer.from_file(str(out_dir / "tokenizer.json"))
    feeds = []
    for enc in tokenizer.encode_batch(texts):
        feeds.append({
            "input_ids": np.asarray([enc.ids], dtype=np.int64),
            "attention_mask": np.asarray([enc.attention_mask], dtype=np.int64),
        })
    return feeds


def convert_one(spec: ModelSpec, texts: list[str]) -> Path:
    out_dir = export_one(spec)
    neg_rewrite.apply(out_dir / "model.onnx", fixture_feeds(out_dir, texts))
    return out_dir


def main() -> int:
    OUTPUT_ROOT.mkdir(exist_ok=True)

    df = pd.read_csv(PARITY_CSV)
    texts = [
        format_input(CourseInput(
            subject_code=str(row.subject_code),
            catalog_number=str(row.catalog_number),
            course_title=str(row.course_title),
        ))
        for row in df.itertuples()
    ]

    for spec in MODELS:
        try:
            convert_one(spec, texts)
        except subprocess.CalledProcessError:
            print(f"\nFAIL: {spec.display_name} conversion failed", file=sys.stderr)
            return 1
        except Exception as e:
            print(f"\nFAIL: {spec.display_name}: {e}", file=sys.stderr)
            return 1

    spec_path = OUTPUT_ROOT / "format_spec.json"
    spec_path.write_text(json.dumps(export_spec(), indent=2) + "\n")
    print(f"\n✓ All {len(MODELS)} models converted")
    print(f"✓ Format spec at {spec_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
