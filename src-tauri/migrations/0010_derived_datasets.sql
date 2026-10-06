-- Derived datasets (#254): rows copied from one or more source datasets
-- that match a filter, with chosen columns.
--
-- dataset_sources: which datasets a derived dataset was built from, in the
-- order the user listed them (that order is the row order of the result).
-- No foreign key on source_dataset_id: a derived dataset does not depend on
-- its sources once built, so a source may be deleted and the link then
-- names a dataset that is gone. source_title is the title at creation time
-- so the page can still name it.
--
-- datasets.layout: every dataset's row layout, the ordered header list plus
-- the positions of the three mapped columns:
--   {"headers": [...], "mapping": {"subject": i, "catalog": j, "title": k}}
-- extra_columns keys are positions in headers. Backfilled from source_files
-- for imports that stored their header row (0004 and later); NULL for older
-- ones, which keep the legacy export shape. Export reads this and nothing
-- else; from now on import writes it here and leaves
-- source_files.original_headers and column_mapping NULL. Those two columns
-- stay: DuckDB refuses DROP COLUMN on a table that a foreign key references.
--
-- datasets.dedupe_columns: for a derived dataset, the output column names
-- that defined a duplicate when it was built (JSON array); NULL when every
-- row was kept.
--
-- Not dropped (measured on 1.5.3): datasets.is_materialized ("Cannot drop
-- this column: an index depends on a column after it", the self-reference
-- foreign keys index later columns) and parent_dataset_id (foreign key).
-- Both stay unused; parent_dataset_id is always NULL, replaced by
-- dataset_sources.

CREATE TABLE dataset_sources (
    dataset_id        TEXT    NOT NULL REFERENCES datasets(id),
    position          INTEGER NOT NULL,
    source_dataset_id TEXT    NOT NULL,
    source_title      TEXT    NOT NULL,
    PRIMARY KEY (dataset_id, position)
);

-- Both columns before the backfill: an UPDATE followed by an ALTER TABLE on
-- the same table in one transaction fails at commit ("another transaction
-- has altered this table").
ALTER TABLE datasets ADD COLUMN layout JSON;
ALTER TABLE datasets ADD COLUMN dedupe_columns JSON;

UPDATE datasets d
SET layout = json_object('headers', sf.original_headers, 'mapping', sf.column_mapping)
FROM source_files sf
WHERE sf.id = d.source_file_id
  AND sf.original_headers IS NOT NULL
  AND sf.column_mapping IS NOT NULL;
