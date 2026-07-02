-- CCM taxonomy lookup table (EPI-59) + real confidence values (EPI-60).
--
-- ccm_taxonomy holds the official code -> title/description mapping for the
-- 2-digit (48 codes: title + short title) and 6-digit (2,119 codes: title +
-- description) levels. The government publishes no 4-digit labels; 4-digit
-- results resolve to their 2-digit parent at read time. Rows are inserted by
-- a Rust data hook that runs inside this migration's transaction (db.rs),
-- sourced from CSVs embedded in the binary — so seeding happens exactly once
-- per database with no startup check.
--
-- inference_results changes:
--   * probability changes meaning: it previously stored the raw argmax
--     logit (unbounded, not a confidence); from now on it stores the softmax
--     probability at argmax (see docs/model-confidence.md).
--   * logit_argmax (new) keeps the raw argmax logit as a research signal.
--   * The DELETE wipes pre-change cache rows: their probability values are
--     logit-scale (unfixable in place — the full logit rows are gone) and
--     their classification labels are float-mangled (e.g. '1.0' for
--     '01.0000'). New runs rewrite both correctly; classifications recompute
--     on the next run. Approved 2026-07-02, pre-release.

CREATE TABLE ccm_taxonomy (
    digit_level TINYINT NOT NULL,
    code        TEXT    NOT NULL,
    title       TEXT    NOT NULL,
    title_short TEXT,
    description TEXT,
    PRIMARY KEY (digit_level, code)
);

ALTER TABLE inference_results ADD COLUMN logit_argmax REAL;
DELETE FROM inference_results;
