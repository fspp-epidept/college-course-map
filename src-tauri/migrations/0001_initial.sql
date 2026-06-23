-- Phase 1 schema. See CLAUDE.md "Schema" ground rule for the design rationale.
--
-- Surrogate ids use sequences + DEFAULT nextval(); DuckDB rejects
-- `GENERATED ALWAYS AS IDENTITY` ("Constraint not implemented").

CREATE SEQUENCE source_files_id_seq;
CREATE TABLE source_files (
    id              BIGINT PRIMARY KEY DEFAULT nextval('source_files_id_seq'),
    path            TEXT NOT NULL,
    display_name    TEXT NOT NULL,
    imported_at     TIMESTAMP NOT NULL,
    imported_hash   VARCHAR NOT NULL,
    size_bytes      BIGINT,
    last_checked_at TIMESTAMP,
    current_hash    VARCHAR,
    is_missing      BOOLEAN NOT NULL DEFAULT FALSE,
    is_dirty        BOOLEAN NOT NULL DEFAULT FALSE,
    column_mapping  JSON,
    notes           TEXT
);
CREATE INDEX idx_source_files_imported_hash ON source_files(imported_hash);

CREATE TABLE datasets (
    id                TEXT PRIMARY KEY,
    title             TEXT NOT NULL,
    source_kind       TEXT NOT NULL CHECK (source_kind IN ('file', 'derived', 'manual')),
    source_file_id    BIGINT REFERENCES source_files(id),
    parent_dataset_id TEXT REFERENCES datasets(id),
    filter_spec       JSON,
    is_materialized   BOOLEAN NOT NULL DEFAULT TRUE,
    imported_at       TIMESTAMP NOT NULL,
    row_count         BIGINT,
    supersedes_id     TEXT REFERENCES datasets(id),
    notes             TEXT
);

CREATE SEQUENCE courses_id_seq;
CREATE TABLE courses (
    id                   BIGINT PRIMARY KEY DEFAULT nextval('courses_id_seq'),
    dataset_id           TEXT NOT NULL REFERENCES datasets(id),
    row_index            BIGINT NOT NULL,
    subject_code         VARCHAR,
    catalog_number       VARCHAR,
    course_title         VARCHAR,
    course_description   VARCHAR,
    school_name          VARCHAR,
    school_year_enrolled VARCHAR,
    extra_columns        JSON,
    content_hash         VARCHAR NOT NULL,
    is_classifiable      BOOLEAN NOT NULL DEFAULT TRUE,
    parse_warnings       JSON
    -- intentionally no uniqueness on (dataset_id, content_hash):
    -- real CSVs contain legitimate duplicates (same course offered
    -- fall + spring, same applicant with two semesters of the same
    -- course). Inference cache deduplicates compute; courses table
    -- preserves source fidelity 1:1.
);
CREATE INDEX idx_courses_dataset_row    ON courses(dataset_id, row_index);
CREATE INDEX idx_courses_dataset_hash   ON courses(dataset_id, content_hash);
CREATE INDEX idx_courses_content_hash   ON courses(content_hash);

CREATE SEQUENCE models_id_seq;
CREATE TABLE models (
    id            BIGINT PRIMARY KEY DEFAULT nextval('models_id_seq'),
    hf_repo       TEXT NOT NULL,
    hf_revision   VARCHAR NOT NULL,
    model_type    TEXT NOT NULL,
    precision     TEXT NOT NULL,
    display_name  TEXT,
    size_bytes    BIGINT,
    local_path    TEXT,
    downloaded_at TIMESTAMP,
    last_used_at  TIMESTAMP,
    UNIQUE (hf_repo, hf_revision, model_type, precision)
);

CREATE TABLE runs (
    id                  TEXT PRIMARY KEY,
    dataset_id          TEXT NOT NULL REFERENCES datasets(id),
    description         TEXT,
    state               TEXT NOT NULL CHECK (state IN (
                            'pending', 'running', 'paused', 'completed',
                            'failed', 'interrupted', 'cancelled'
                        )),
    model_ids           JSON NOT NULL,
    course_filter       JSON,
    rows_total          BIGINT,
    rows_processed      BIGINT,
    unique_inputs_total BIGINT,
    unique_inputs_done  BIGINT,
    cache_hits          BIGINT,
    created_at          TIMESTAMP NOT NULL,
    started_at          TIMESTAMP,
    completed_at        TIMESTAMP,
    last_progress_at    TIMESTAMP,
    resume_count        INTEGER NOT NULL DEFAULT 0,
    error_message       TEXT,
    execution_provider  TEXT
);

CREATE TABLE inference_results (
    model_id        BIGINT NOT NULL REFERENCES models(id),
    content_hash    VARCHAR NOT NULL,
    classification  VARCHAR NOT NULL,
    probability     REAL,
    computed_at     TIMESTAMP NOT NULL,
    computed_by_run TEXT REFERENCES runs(id),
    PRIMARY KEY (model_id, content_hash)
);
CREATE INDEX idx_inference_results_content_hash ON inference_results(content_hash);
