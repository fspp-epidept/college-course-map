# Validation report

Generated: 2026-05-08T12:40:16Z

- Source CSV: `/home/nick/dev/um/jurgens/course-classifier/data/validation.csv`
- Sample mode: `full` (1,757,531 rows)
- Input format: B (model card spec)
- Execution provider (actual): CUDAExecutionProvider
- Preferred order: CUDAExecutionProvider, CPUExecutionProvider
- Filter: non-null required fields, exclude Multiple Course?

**Train/test split status: unknown.** Until annamp confirms whether this
panel was held out from training, treat the accuracy numbers as preliminary.
If the panel overlaps with the training set, accuracy will be inflated by
memorization. The numbers are still useful as a sanity check that conversion
didn't break the model.

Accuracy compares predictions to `inventory_cip_*` after canonicalizing both
to a common numeric form (panel uses bare digits, models use dotted CCM form;
both are converted to floats for equality).

| Model | Unique inputs | Rows compared | Accuracy | p50 (ms) | p95 (ms) |
|---|---:|---:|---:|---:|---:|
| Two-digit CCM | 209,582 | 1,757,531 | 77.7% | 0.17 | 0.20 |
| Four-digit CCM | 209,582 | 1,757,531 | 57.5% | 0.18 | 0.19 |
| Six-digit CCM | 209,582 | 1,757,531 | 36.7% | 0.16 | 0.18 |

Disagreement detail (predicted ≠ panel label) lives in the run's
`disagreements.csv`. See `output/validation/<run-id>/`.
