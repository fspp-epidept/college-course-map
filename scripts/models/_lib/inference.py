"""Shared ONNX Runtime helpers."""
from __future__ import annotations

from pathlib import Path

import numpy as np
import onnxruntime as ort


def select_providers() -> list[str]:
    """Return providers in priority order, filtered to what's available."""
    available = set(ort.get_available_providers())
    preferred = ["CUDAExecutionProvider", "CPUExecutionProvider"]
    selected = [p for p in preferred if p in available]
    if not selected:
        raise RuntimeError(f"No usable ONNX EP. Available: {sorted(available)}")
    return selected


def load_session(
    onnx_path: Path,
    providers: list[str] | None = None,
) -> tuple[ort.InferenceSession, str]:
    """Open an inference session; return session and the actual provider chosen."""
    if not onnx_path.exists():
        raise FileNotFoundError(onnx_path)
    providers = providers or select_providers()
    sess_options = ort.SessionOptions()
    session = ort.InferenceSession(str(onnx_path), sess_options=sess_options, providers=providers)
    actual = session.get_providers()[0]
    return session, actual


def predict_batch(
    session: ort.InferenceSession,
    tokenizer,
    texts: list[str],
    max_length: int = 512,
) -> np.ndarray:
    """Tokenize, run, return logits array of shape (batch, n_classes)."""
    encoded = tokenizer(
        texts,
        return_tensors="np",
        truncation=True,
        padding=True,
        max_length=max_length,
    )
    input_names = {inp.name for inp in session.get_inputs()}
    feed = {k: np.asarray(v) for k, v in encoded.items() if k in input_names}
    return session.run(None, feed)[0]
