"""Post-export graph pass: fp32 `Neg` → `Mul(x, -1.0)`.

ONNX Runtime's CoreML execution provider has no `Neg` builder. ModernBERT's
export carries two `Neg` nodes per layer (the rotary `rotate_half`), so on
CoreML every layer splits into CoreML/CPU partitions around them. `Neg(x)`
and `Mul(x, -1.0)` are bit-identical in IEEE float, so the rewrite changes
the graph's partitioning without changing a single output bit — and this
module proves that per run on the parity fixture.

Only fp32 `Neg` is rewritten. Anything else is left alone and reported:
integer `Neg` stays on CPU either way, and an fp16 rewrite would be an
undecided precision question. A graph with nothing to rewrite is left
untouched; a rewritten one is stamped `metadata_props` `coreml_neg_rewrite=1`
so re-running is a no-op.
"""
from __future__ import annotations

from pathlib import Path

import numpy as np
import onnx
import onnxruntime as ort
from onnx import TensorProto, numpy_helper

METADATA_KEY = "coreml_neg_rewrite"
NEG_ONE_NAME = "coreml_neg_rewrite/neg_one_fp32"


def is_stamped(model: onnx.ModelProto) -> bool:
    return any(p.key == METADATA_KEY and p.value == "1" for p in model.metadata_props)


def _elem_types(model: onnx.ModelProto) -> dict[str, int]:
    """Tensor name → elem_type for everything shape inference can name."""
    inferred = onnx.shape_inference.infer_shapes(model)
    types: dict[str, int] = {}
    for vi in (*inferred.graph.input, *inferred.graph.output, *inferred.graph.value_info):
        if vi.type.HasField("tensor_type"):
            types[vi.name] = vi.type.tensor_type.elem_type
    for init in model.graph.initializer:
        types[init.name] = init.data_type
    return types


def rewrite_neg(model: onnx.ModelProto) -> tuple[int, list[str]]:
    """Rewrite every fp32 `Neg` in place. Returns (rewritten, skipped names)."""
    types = _elem_types(model)
    rewritten = 0
    skipped: list[str] = []
    for node in model.graph.node:
        if node.op_type != "Neg":
            continue
        if types.get(node.input[0]) != TensorProto.FLOAT:
            dtype = TensorProto.DataType.Name(types.get(node.input[0], TensorProto.UNDEFINED))
            skipped.append(f"{node.name} ({dtype})")
            continue
        node.op_type = "Mul"
        node.input.append(NEG_ONE_NAME)
        rewritten += 1
    if rewritten:
        model.graph.initializer.append(
            numpy_helper.from_array(np.array(-1.0, dtype=np.float32), NEG_ONE_NAME)
        )
        model.metadata_props.append(onnx.StringStringEntryProto(key=METADATA_KEY, value="1"))
    return rewritten, skipped


def _logits(model_bytes: bytes, feeds: list[dict[str, np.ndarray]]) -> list[np.ndarray]:
    opts = ort.SessionOptions()
    opts.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
    session = ort.InferenceSession(model_bytes, sess_options=opts, providers=["CPUExecutionProvider"])
    names = {i.name for i in session.get_inputs()}
    return [session.run(None, {k: v for k, v in f.items() if k in names})[0] for f in feeds]


def apply(onnx_path: Path, feeds: list[dict[str, np.ndarray]]) -> None:
    """Rewrite `onnx_path` in place and prove bitwise equality on `feeds`.

    `feeds` are pre-tokenized fixture inputs. Raises if any output bit
    differs between the original and rewritten graphs (CPU EP, ORT_ENABLE_ALL).
    """
    model = onnx.load(str(onnx_path))
    if is_stamped(model):
        print(f"  neg-rewrite: already applied to {onnx_path.name}")
        return

    before = model.SerializeToString()
    rewritten, skipped = rewrite_neg(model)
    for name in skipped:
        print(f"  neg-rewrite: left alone: {name}")
    if not rewritten:
        print(f"  neg-rewrite: no fp32 Neg in {onnx_path.name}; left untouched")
        return

    after = model.SerializeToString()
    ref = _logits(before, feeds)
    out = _logits(after, feeds)
    max_diff = max(float(np.abs(a - b).max()) for a, b in zip(ref, out, strict=True))
    if max_diff != 0.0:
        raise RuntimeError(f"neg-rewrite: outputs differ (max |diff| {max_diff:.3e}); not saving")
    onnx.save(model, str(onnx_path))
    print(f"  neg-rewrite: {rewritten} fp32 Neg → Mul(-1.0); bitwise identical on {len(feeds)} fixture inputs")
