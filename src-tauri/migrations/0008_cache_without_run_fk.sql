-- Results cache without the foreign key to runs, and without the secondary
-- indexes (#196, #198).
--
-- inference_results changes:
--   * computed_by_run was declared `REFERENCES runs(id)` in 0001, which
--     contradicts the cache's design (keyed by (model_id, content_hash), not
--     tied to runs) and made every run undeletable once it had cached a
--     result. It stays as a plain provenance column and may name a run that
--     has since been deleted.
--   * DuckDB can't drop the constraint or the column in place ("Cannot drop
--     column ... because there is a FOREIGN KEY constraint that depends on
--     it"), so the table is rebuilt. Rows are staged, the table is dropped
--     and recreated UNDER ITS FINAL NAME, and the rows are reinserted. Never
--     `ALTER TABLE ... RENAME` a table that has a foreign key: DuckDB 1.5.3
--     leaves the parent (`models`) pointing at the old name, and every later
--     DELETE on the parent fails, even after a reopen.
--   * Column order is unchanged. The primary key is added after the bulk
--     load so its index is built once instead of maintained per row.
--
-- Secondary indexes dropped (decision 2026-10-02): idx_courses_dataset_row,
-- idx_courses_dataset_hash, idx_courses_content_hash and
-- idx_inference_results_content_hash (the last goes with the old table and
-- is not recreated). DuckDB answers this app's queries with scans and hash
-- joins; measured on a 2.9M-course database, none was faster with them, and
-- together they were about 40% of the file and doubled insert time. Primary
-- keys and foreign keys are untouched.

CREATE TABLE inference_results_stage AS SELECT * FROM inference_results;
DROP TABLE inference_results;
CREATE TABLE inference_results (
    model_id        BIGINT NOT NULL REFERENCES models(id),
    content_hash    VARCHAR NOT NULL,
    classification  VARCHAR NOT NULL,
    probability     REAL,
    computed_at     TIMESTAMP NOT NULL,
    computed_by_run TEXT,
    logit_argmax    REAL,
    top2_code       VARCHAR,
    top2_prob       REAL,
    top3_code       VARCHAR,
    top3_prob       REAL,
    top4_code       VARCHAR,
    top4_prob       REAL,
    top5_code       VARCHAR,
    top5_prob       REAL
);
INSERT INTO inference_results SELECT * FROM inference_results_stage;
DROP TABLE inference_results_stage;
ALTER TABLE inference_results ADD PRIMARY KEY (model_id, content_hash);

DROP INDEX idx_courses_dataset_row;
DROP INDEX idx_courses_dataset_hash;
DROP INDEX idx_courses_content_hash;
