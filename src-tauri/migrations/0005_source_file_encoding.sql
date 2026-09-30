-- Source file text encoding (EPI-113). Import now accepts legacy single-byte
-- CSVs (Windows-1252, Mac Roman) once the user confirms the encoding in the
-- pre-flight; the WHATWG label is recorded so any later re-read of the source
-- (refresh / drift checks) decodes it the same way.
--
-- DuckDB rejects `ADD COLUMN ... NOT NULL` (see 0002), so the column is
-- nullable with a default; the DEFAULT backfills existing rows, which were
-- all imported as strict UTF-8.

ALTER TABLE source_files ADD COLUMN encoding VARCHAR DEFAULT 'utf-8';
