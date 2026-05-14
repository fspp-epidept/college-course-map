# Validation report

Generated: 2026-05-08T12:40:16Z

- Source CSV: `/home/nick/dev/um/jurgens/course-classifier/data/validation.csv`
- Sample mode: `full` (1,757,531 rows)
- Input format: B (model card spec)
- Execution provider (actual): CUDAExecutionProvider
- Preferred order: CUDAExecutionProvider, CPUExecutionProvider
- Filter: non-null required fields, exclude Multiple Course?

**This is a CIP/CCM overlap measurement, not a model-accuracy measurement.**
The panel's `inventory_cip_*` columns contain federal **CIP codes**; the
models output **CCM codes** — a distinct hierarchical taxonomy. The two
overlap heavily at the broad 2-digit level (subject area) but diverge as
specificity increases. The descending overlap rate across digit levels is
the *expected* taxonomy divergence, not a regression. The meaningful
correctness check is parity (Rust ONNX == Python ONNX == annamp PyTorch),
covered by `verify.py` and the Rust integration tests.

The columns below compare predictions to `inventory_cip_*` after
canonicalizing both to a common numeric form (panel uses bare digits,
models use dotted form; both converted to floats for equality).

| Model | Unique inputs | Rows compared | CIP/CCM overlap | p50 (ms) | p95 (ms) |
|---|---:|---:|---:|---:|---:|
| Two-digit CCM | 209,582 | 1,757,531 | 77.7% | 0.17 | 0.20 |
| Four-digit CCM | 209,582 | 1,757,531 | 57.5% | 0.18 | 0.19 |
| Six-digit CCM | 209,582 | 1,757,531 | 36.7% | 0.16 | 0.18 |

Per-row mismatches (predicted ≠ panel label) live in the run's
`disagreements.csv`. See `output/validation/<run-id>/`.
