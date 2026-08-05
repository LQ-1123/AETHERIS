-- Remove de-identification exports and Calling AE automatic normalization.
-- Migrations 0003-0005 may already be present in deployed databases, so their
-- schema is retired here instead of rewriting migration history.

-- Preserve immutable revision provenance while detaching retired jobs.
UPDATE dicom_instance_versions v
SET transform_job_fk = NULL
FROM dicom_transform_jobs j
WHERE v.transform_job_fk = j.id
  AND j.mode IN ('deidentify', 'normalize');

DELETE FROM dicom_transform_jobs
WHERE mode IN ('deidentify', 'normalize');

ALTER TABLE dicom_transform_jobs
    DROP CONSTRAINT dicom_transform_jobs_mode_check,
    DROP CONSTRAINT dicom_transform_jobs_status_check,
    DROP COLUMN template_fk,
    DROP COLUMN deid_project_fk,
    DROP COLUMN manifest,
    DROP COLUMN archive_path,
    DROP COLUMN expires_at,
    DROP COLUMN attempt_count,
    DROP COLUMN max_attempts,
    DROP COLUMN next_attempt_at,
    DROP COLUMN scope_study_fk,
    ADD CONSTRAINT dicom_transform_jobs_mode_check
        CHECK (mode IN ('clinical_correction', 'rollback')),
    ADD CONSTRAINT dicom_transform_jobs_status_check
        CHECK (status IN ('previewed', 'queued', 'running', 'succeeded', 'failed', 'blocked'));

DROP TABLE calling_ae_normalization_policies;
DROP TABLE pseudonym_mappings;
DROP TABLE deid_projects;
DROP TABLE dicom_transform_templates;
