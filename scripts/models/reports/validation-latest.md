# Validation report

Generated: 2026-05-08T05:21:59Z

- Source CSV: `/home/nick/dev/um/jurgens/course-classifier/data/validation.csv`
- Sample mode: `10k` (10,000 rows)
- Input format: B (model card spec)
- Execution provider (actual): CPUExecutionProvider
- Preferred order: CUDAExecutionProvider, CPUExecutionProvider
- Filter: non-null required fields, exclude Multiple Course?

**Train/test split status: unknown.** Until annamp confirms whether this
panel was held out from training, treat the accuracy numbers as preliminary.
If the panel overlaps with the training set, accuracy will be inflated by
memorization. The numbers are still useful as a sanity check that conversion
didn't break the model.

Accuracy compares predictions to `inventory_cip_*` after canonicalizing both
to a common numeric form (panel uses bare digits, models use dotted CIP form;
both are converted to floats for equality).

| Model | Unique inputs | Rows compared | Accuracy | p50 (ms) | p95 (ms) |
|---|---:|---:|---:|---:|---:|
| Two-digit CIP | 9,705 | 10,000 | 77.5% | 3.69 | 4.42 |
| Four-digit CIP | 9,705 | 10,000 | 57.0% | 4.05 | 4.87 |
| Six-digit CIP | 9,705 | 10,000 | 36.1% | 4.01 | 4.61 |

Disagreement detail (predicted ≠ panel label) lives in the run's
`disagreements.csv`. See `output/validation/<run-id>/`.
