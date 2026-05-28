-- Async-import lifecycle: import_csv now returns immediately with a dataset
-- id in 'importing' state and a background worker streams rows in. The UI
-- polls list_datasets to watch row_count + import_state until 'ready' or
-- 'failed'. import_error carries the failure message for surfacing.
--
-- Existing rows default to 'ready' so previously-imported datasets stay
-- selectable without a backfill step.

ALTER TABLE datasets ADD COLUMN import_state TEXT NOT NULL DEFAULT 'ready';
ALTER TABLE datasets ADD COLUMN import_error TEXT;
