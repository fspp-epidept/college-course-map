"""Shared ONNX Runtime helpers."""
from __future__ import annotations

import ctypes
import sys
from pathlib import Path

import numpy as np
import onnxruntime as ort


def _preload_cuda_libs() -> None:
    """Make the venv's nvidia-* CUDA libs visible to ONNX Runtime's CUDA EP.

    The nvidia-* PyPI packages (nvidia-cublas, nvidia-cudnn-cu13, etc.) drop
    .so files into `.venv/.../site-packages/nvidia/<pkg>/lib/`. PyTorch
    preloads them via its own import-time mechanism, but ONNX Runtime's CUDA
    provider opens them lazily at session-creation time using the system
    dynamic linker, which doesn't search those venv paths. Calling
    ctypes.CDLL with RTLD_GLOBAL pulls each lib into the process and adds
    its symbols to the global symbol table, so ORT's later dlopen finds them.

    Idempotent and safe: failures are silently swallowed (libs may not be
    present on every platform; ORT will fall back to CPU automatically).
    """
    try:
        import nvidia  # type: ignore[import-untyped]
    except ImportError:
        return

    # `nvidia` is a namespace package — no __file__, but __path__ lists
    # every directory that contributes to it (one per nvidia-* wheel).
    lib_subdirs = ("cu13/lib", "cudnn/lib")
    seen: set[Path] = set()
    for root in map(Path, nvidia.__path__):
        for sub in lib_subdirs:
            d = root / sub
            if d in seen or not d.is_dir():
                continue
            seen.add(d)
            for so in sorted(d.glob("*.so*")):
                try:
                    ctypes.CDLL(str(so), mode=ctypes.RTLD_GLOBAL)
                except OSError:
                    pass


# Preload at import time so any subsequent ort.InferenceSession with CUDA EP works.
if sys.platform == "linux":
    _preload_cuda_libs()


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
