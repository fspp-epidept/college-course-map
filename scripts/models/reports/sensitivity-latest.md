# Input sensitivity report

Generated: 2026-10-01T12:37:53Z

- Source CSV: `data/validation.csv`
- Sample: `5k` (5,000 unique inputs, seed 114)
- Execution provider (actual): CUDAExecutionProvider
- Preferred order: CUDAExecutionProvider, CPUExecutionProvider

How much each model's predictions change when a correctly formatted
input is damaged the way a bad export or column mapping damages it.
Every variant is assembled by the real formatter from altered fields;
the baseline row is the unaltered input. Flip rate is the share of
inputs whose top-1 prediction differs from the baseline prediction.
Mean top-1 probability shows whether confidence reacts to the damage.

**CIP agreement is a CIP/CCM overlap measurement, not model accuracy.**
The panel's `inventory_cip_*` columns contain federal **CIP codes**; the
models output **CCM codes** — a distinct hierarchical taxonomy. The two
overlap heavily at the broad 2-digit level and diverge as specificity
increases, so the column is a directional proxy: a drop against the
baseline row means the damage moved predictions away from the panel
label. Codes are canonicalized the same way as in `validate.py`.

See `docs/input-contract.md` for the input rules these numbers justify.

## Two-digit CCM (ModernBERT)

Panel label column: `inventory_cip_two`.

| Variant | Predictions changed | Mean top-1 probability | CIP agreement |
|---|---:|---:|---:|
| baseline | 0.0% | 0.921 | 75.2% |
| subject repeated in catalog (`PSYC 4325`) | 10.5% | 0.918 | 74.5% |
| subject repeated in catalog, no space (`PSYC4325`) | 11.7% | 0.917 | 74.3% |
| full code in both subject and catalog | 10.7% | 0.918 | 74.7% |
| catalog as converted number (`4325.0`) | 8.8% | 0.920 | 75.6% |
| catalog letters stripped (`4304L` -> `4304`) | 1.0% | 0.921 | 75.2% |
| catalog missing | 14.2% | 0.913 | 70.9% |
| subject missing | 35.0% | 0.850 | 52.7% |
| title missing | 29.9% | 0.880 | 69.8% |
| title only | 36.9% | 0.863 | 51.7% |
| subject and catalog swapped | 17.0% | 0.902 | 69.5% |
| title in the catalog column | 17.7% | 0.893 | 68.7% |
| catalog is a year (`2024`) | 11.1% | 0.914 | 73.7% |
| subject lowercased | 15.3% | 0.900 | 70.7% |
| title in Title Case | 15.4% | 0.919 | 73.6% |
| title lowercased | 16.1% | 0.910 | 74.0% |
| trailing whitespace kept | 12.7% | 0.909 | 73.1% |
| title prefixed with the code | 10.7% | 0.918 | 75.3% |

## Six-digit CCM (ModernBERT)

Panel label column: `inventory_cip_six`.

| Variant | Predictions changed | Mean top-1 probability | CIP agreement |
|---|---:|---:|---:|
| baseline | 0.0% | 0.680 | 34.2% |
| subject repeated in catalog (`PSYC 4325`) | 20.1% | 0.670 | 34.6% |
| subject repeated in catalog, no space (`PSYC4325`) | 21.7% | 0.673 | 35.4% |
| full code in both subject and catalog | 21.6% | 0.667 | 35.3% |
| catalog as converted number (`4325.0`) | 16.6% | 0.687 | 35.5% |
| catalog letters stripped (`4304L` -> `4304`) | 1.3% | 0.680 | 34.2% |
| catalog missing | 27.4% | 0.674 | 35.4% |
| subject missing | 40.4% | 0.622 | 24.9% |
| title missing | 78.0% | 0.593 | 36.3% |
| title only | 44.7% | 0.629 | 24.5% |
| subject and catalog swapped | 27.7% | 0.659 | 33.3% |
| title in the catalog column | 28.4% | 0.658 | 34.5% |
| catalog is a year (`2024`) | 20.8% | 0.671 | 34.8% |
| subject lowercased | 25.5% | 0.643 | 32.5% |
| title in Title Case | 31.7% | 0.672 | 34.7% |
| title lowercased | 35.9% | 0.629 | 33.7% |
| trailing whitespace kept | 24.7% | 0.671 | 35.9% |
| title prefixed with the code | 23.0% | 0.659 | 36.0% |
