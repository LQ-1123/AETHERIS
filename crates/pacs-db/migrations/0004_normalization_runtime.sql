-- Calling AE automatic normalization runtime and retry state.

ALTER TABLE dicom_transform_jobs
    ADD COLUMN attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    ADD COLUMN max_attempts INTEGER NOT NULL DEFAULT 1 CHECK (max_attempts BETWEEN 1 AND 10),
    ADD COLUMN next_attempt_at TIMESTAMPTZ;

UPDATE dicom_transform_jobs
SET next_attempt_at = created_at
WHERE status = 'queued';

CREATE INDEX dicom_transform_jobs_runnable_idx
    ON dicom_transform_jobs (next_attempt_at, created_at)
    WHERE status = 'queued';

