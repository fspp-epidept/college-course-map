"""Convert annamp PyTorch models to ONNX via optimum-cli.

Idempotent: if `output/<dir>/model.onnx` already exists and is non-empty,
the model is skipped. Wipe `output/` to force a fresh conversion.
"""
from __future__ import annotations

import json
import shutil
import subprocess
import sys
from pathlib import Path

from _lib.format import export_spec
from _lib.models import MODELS, ModelSpec

OUTPUT_ROOT = Path(__file__).parent / "output"


def convert_one(spec: ModelSpec) -> Path:
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


def main() -> int:
    OUTPUT_ROOT.mkdir(exist_ok=True)

    for spec in MODELS:
        try:
            convert_one(spec)
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
