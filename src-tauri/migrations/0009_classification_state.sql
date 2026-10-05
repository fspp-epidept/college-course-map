-- Classification state on the dataset (#249). A run owned no data (results
-- are cached by (model_id, content_hash)), so the runs table goes and what a
-- user cares about, per dataset, becomes four columns:
--   classify_state       idle | running | stopped | failed
--   classify_error       message of the last failure, else NULL
--   classify_ep          execution provider the latest job ran on
--   classify_updated_at  when the state last changed
-- How far each model got is not stored: it is the cache coverage.
--
-- No NOT NULL or CHECK: DuckDB refuses constraints on ADD COLUMN (see 0002).
-- Inserts write 'idle' explicitly and list_datasets COALESCEs on read.
--
-- Each dataset's latest run carries over: running, interrupted, paused and
-- pending become stopped (a running row here is a crash leftover), failed
-- stays failed with its message, everything else becomes idle.
--
-- runs is a child of datasets with no children of its own since 0008, so it
-- drops cleanly. inference_results.computed_by_run lost its foreign key in
-- 0008, so DROP COLUMN is allowed. Nothing is renamed.

ALTER TABLE datasets ADD COLUMN classify_state TEXT;
ALTER TABLE datasets ADD COLUMN classify_error TEXT;
ALTER TABLE datasets ADD COLUMN classify_ep TEXT;
ALTER TABLE datasets ADD COLUMN classify_updated_at TIMESTAMP;

UPDATE datasets SET classify_state = 'idle';
UPDATE datasets d
SET classify_state = CASE latest.state
        WHEN 'failed' THEN 'failed'
        WHEN 'completed' THEN 'idle'
        WHEN 'cancelled' THEN 'idle'
        ELSE 'stopped'
    END,
    classify_error = CASE WHEN latest.state = 'failed' THEN latest.error_message END,
    classify_ep = latest.execution_provider,
    classify_updated_at = COALESCE(latest.completed_at, latest.last_progress_at, latest.created_at)
FROM (
    SELECT * FROM runs
    QUALIFY row_number() OVER (PARTITION BY dataset_id ORDER BY created_at DESC) = 1
) latest
WHERE latest.dataset_id = d.id;

DROP TABLE runs;
ALTER TABLE inference_results DROP COLUMN computed_by_run;
