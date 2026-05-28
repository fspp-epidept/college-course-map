-- Async-import lifecycle: import_csv now returns immediately with a dataset
-- id in 'importing' state and a background worker streams rows in. The UI
-- polls list_datasets to watch row_count + import_state until 'ready' or
-- 'failed'. import_error carries the failure message for surfacing.
--
-- DuckDB doesn't support `ADD COLUMN ... NOT NULL DEFAULT ...` (parser error
-- "Adding columns with constraints not yet supported"), so the column is
-- added nullable and backfilled in two steps. The Rust insert path always
-- writes an explicit value, and list_datasets COALESCEs on read, so the
-- nullability is never user-visible.

ALTER TABLE datasets ADD COLUMN import_state TEXT;
UPDATE datasets SET import_state = 'ready' WHERE import_state IS NULL;
ALTER TABLE datasets ADD COLUMN import_error TEXT;
