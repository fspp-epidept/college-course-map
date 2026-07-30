-- Round-trip export + top-5 candidates (EPI-79, EPI-98).
--
-- source_files changes:
--   * original_headers (new): JSON array of the CSV's header row, in file
--     order. Together with column_mapping (0001, populated by import from now
--     on) this lets export reconstruct a column-identical copy of the input
--     with ccm_* columns appended. Rows imported before this migration stay
--     NULL and export in the legacy fixed-column shape.
--
-- inference_results changes:
--   * top2..top5 (new): candidate codes ranked 2-5 by logit, with their
--     softmax probabilities. Rank 1 is the existing classification /
--     probability pair. Codes persist in canonical zero-padded form like
--     classification does.
--   * The DELETE wipes pre-change cache rows: their rank 2-5 candidates were
--     discarded at inference time and cannot be backfilled. Same precedent as
--     0003 — classifications recompute on the next run. Approved 2026-07-28,
--     pre-release (stakeholder meeting: top-5 export columns).

ALTER TABLE source_files ADD COLUMN original_headers JSON;

ALTER TABLE inference_results ADD COLUMN top2_code VARCHAR;
ALTER TABLE inference_results ADD COLUMN top2_prob REAL;
ALTER TABLE inference_results ADD COLUMN top3_code VARCHAR;
ALTER TABLE inference_results ADD COLUMN top3_prob REAL;
ALTER TABLE inference_results ADD COLUMN top4_code VARCHAR;
ALTER TABLE inference_results ADD COLUMN top4_prob REAL;
ALTER TABLE inference_results ADD COLUMN top5_code VARCHAR;
ALTER TABLE inference_results ADD COLUMN top5_prob REAL;
DELETE FROM inference_results;
