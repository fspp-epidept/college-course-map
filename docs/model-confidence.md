# Model confidence: definition, formula, and dependency chain

Audience: the modeling / statistics team validating that the confidence values
this app reports and exports are sound from a research perspective. This
document is the authoritative description of how `inference_results.probability`
and `inference_results.logit_argmax` are produced. It is versioned with the
code that implements it (`src-tauri/src/inference.rs::softmax_at`,
`classify_batch`).

## Definitions

For one course input, the classifier produces a logit vector
`z ∈ ℝ^K` (one raw, unbounded score per class; `K` = 48 / 353 / 1,413 for the
2- / 4- / 6-digit models). The model has no probability output of its own —
logits are the only signal, and everything below is derived from them.

- **`classification`** — the class with the largest logit,
  `argmax_i z_i`, mapped through the model's `id2label` table and normalized
  to the canonical zero-padded CCM code (`normalize_ccm_code`).
- **`probability`** — the softmax probability of that class:

  ```text
  p_i = exp(z_i) / Σ_{j=1..K} exp(z_j)          (definition)

  probability = p_argmax
  ```

  Computed in the numerically stable max-shifted form (mathematically
  identical; prevents f32 overflow of `exp` for large-magnitude logits):

  ```text
  m = max_j z_j
  probability = exp(z_argmax − m) / Σ_j exp(z_j − m)
              = 1 / Σ_j exp(z_j − m)             (since z_argmax = m)
  ```

  Range `(0, 1]`; the full softmax vector sums to 1. This equals
  `torch.softmax(logits, dim=-1)[argmax]` up to floating-point accumulation
  order.
- **`logit_argmax`** — the raw value `z_argmax`, persisted unchanged as a
  research signal (see "Calibration caveats").

## Where each step happens

| Step | Implementation | Precision |
|---|---|---|
| Input assembly | `format.rs` — `"{SUBJECT} {NUMBER} --- {TITLE}"` | text |
| Tokenization | `tokenizers` crate (HF `tokenizer.json`), truncation at 512 tokens, right padding | i64 ids |
| Forward pass | ONNX Runtime via `ort` v2.0.0-rc.12, graph exported by `optimum-cli export onnx --task text-classification` from the annamp PyTorch checkpoints | f32 |
| Softmax | `inference.rs::softmax_at` (Rust, formula above) | f32 in, f32 out |
| Persistence | `runs.rs::flush_batch` → DuckDB `inference_results.probability` / `.logit_argmax` | stored as f64 (REAL columns), widened from f32 |

The softmax is computed **in this app, not inside the ONNX graph** — the
exported graph ends at the logits, exactly like the Python reference
(`scripts/models/_lib/inference.py::predict_batch` returns logits and
`verify.py` compares them raw).

Rust-vs-Python **logit parity** is asserted by `task check:parity`
(100% argmax/top-3 agreement, max |Δlogit| ≤ 1e-3 tolerance, observed ≈ 5e-6
for RoBERTa). Since `probability` is a deterministic function of the logits,
parity of logits implies parity of confidence up to f32 rounding.

## Calibration caveats (why `logit_argmax` is kept)

Softmax probabilities from fine-tuned transformers are typically
**overconfident**: `probability = 0.97` should not be read as "97% of such
predictions are correct" without a calibration study (reliability diagram /
ECE, temperature scaling) against labeled data. Until such a study exists,
treat `probability` as a *ranking* signal (higher = more confident) rather
than a calibrated frequency.

`logit_argmax` is preserved so pre-softmax signal survives; note that
temperature scaling and entropy/margin analyses need the **full logit
vector**, which is currently discarded after argmax — persisting it (or a
top-k reduction) is tracked as Linear EPI-61.

## Reproducing a value

Given the same input string, `tokenizer.json`, and `model.onnx`:

```python
enc = tokenizer(text, return_tensors="np", truncation=True, max_length=512)
logits = onnx_session.run(None, dict(enc))[0][0]     # shape (K,)
prob = np.exp(logits - logits.max()) / np.exp(logits - logits.max()).sum()
assert np.isclose(prob[logits.argmax()], app_probability, atol=1e-6)
```
