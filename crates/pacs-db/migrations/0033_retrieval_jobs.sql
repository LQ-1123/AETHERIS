-- v0.4.0: durable external-PACS retrieval jobs.
-- Keep this separate from 0032 because 0032 may already be applied by users
-- testing the retrieval-source configuration migration.

ALTER TABLE background_jobs
    DROP CONSTRAINT background_jobs_kind_check;
ALTER TABLE background_jobs
    ADD CONSTRAINT background_jobs_kind_check CHECK (kind IN (
        'import', 'export', 'route', 'lifecycle', 'retrieval'
    ));
