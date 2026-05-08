# Parity report

Generated: 2026-05-08T12:30:40Z

Compares ONNX exports against PyTorch sources on a small synthetic corpus.
Argmax should be ≥99% in most cases (occasional dips on near-tie inputs are
acceptable). Max logit diff should be < 1e-3.

| Model | n | Argmax | Top-3 | Max diff | Mean diff | Pass |
|---|---:|---:|---:|---:|---:|:---:|
| Two-digit CCM | 20 | 100.0% | 100.0% | 5.72e-06 | 2.83e-06 | ✓ |
| Four-digit CCM | 20 | 100.0% | 100.0% | 4.77e-06 | 3.12e-06 | ✓ |
| Six-digit CCM | 20 | 100.0% | 100.0% | 5.25e-06 | 3.73e-06 | ✓ |
