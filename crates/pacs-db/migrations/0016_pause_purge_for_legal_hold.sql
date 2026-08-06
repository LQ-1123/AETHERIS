-- Freeze an approved purge grace period while a Legal Hold is active.

ALTER TABLE background_jobs
    DROP CONSTRAINT background_jobs_status_check;
ALTER TABLE background_jobs
    ADD CONSTRAINT background_jobs_status_check CHECK (status IN (
        'queued', 'running', 'paused', 'succeeded', 'failed', 'cancelled'
    ));

ALTER TABLE dicom_purge_requests
    DROP CONSTRAINT dicom_purge_requests_status_check;
ALTER TABLE dicom_purge_requests
    ADD COLUMN grace_remaining_seconds BIGINT
        CHECK (grace_remaining_seconds IS NULL OR grace_remaining_seconds >= 0),
    ADD CONSTRAINT dicom_purge_requests_status_check CHECK (status IN (
        'pending', 'approved', 'paused_hold', 'executing', 'completed',
        'rejected', 'cancelled', 'failed'
    ));

DROP INDEX dicom_purge_requests_open_idx;
CREATE UNIQUE INDEX dicom_purge_requests_open_idx
    ON dicom_purge_requests (institution_id, study_instance_uid)
    WHERE status IN ('pending', 'approved', 'paused_hold', 'executing');

ALTER TABLE dicom_lifecycle_events
    DROP CONSTRAINT dicom_lifecycle_events_action_check;
ALTER TABLE dicom_lifecycle_events
    ADD CONSTRAINT dicom_lifecycle_events_action_check CHECK (action IN (
        'move_to_cold', 'restore_to_hot', 'quarantine',
        'purge_requested', 'purge_approved', 'purge_paused_hold',
        'purge_resumed_hold', 'purge_rejected', 'purged',
        'legal_hold_created', 'legal_hold_released'
    ));

-- Repair an approved request that was already inside a grace period when this
-- migration is applied while its Study has an active Hold.
WITH paused AS (
    UPDATE dicom_purge_requests r
    SET status = 'paused_hold',
        grace_remaining_seconds = GREATEST(
            0,
            CEIL(EXTRACT(EPOCH FROM (r.grace_until - now())))::BIGINT
        ),
        grace_until = NULL,
        error_message = NULL
    WHERE r.status = 'approved'
      AND EXISTS (
          SELECT 1 FROM dicom_legal_holds h
          WHERE h.institution_id = r.institution_id
            AND h.study_instance_uid = r.study_instance_uid
            AND h.released_at IS NULL
            AND (h.expires_at IS NULL OR h.expires_at > now())
      )
    RETURNING r.job_fk
)
UPDATE background_jobs j
SET status = 'paused',
    attempts = 0,
    lease_owner = NULL,
    lease_expires_at = NULL,
    error_message = '因 Legal Hold 暂停',
    completed_at = NULL
FROM paused
WHERE j.id = paused.job_fk
  AND j.status IN ('queued', 'running', 'failed');

INSERT INTO dicom_lifecycle_events (
    institution_id, study_instance_uid, action, from_tier, job_fk, details
)
SELECT r.institution_id, r.study_instance_uid, 'purge_paused_hold',
       'quarantine', r.job_fk,
       jsonb_build_object(
           'request_id', r.id,
           'remaining_grace_seconds', r.grace_remaining_seconds,
           'migration_repair', true
       )
FROM dicom_purge_requests r
WHERE r.status = 'paused_hold'
  AND NOT EXISTS (
      SELECT 1 FROM dicom_lifecycle_events e
      WHERE e.institution_id = r.institution_id
        AND e.study_instance_uid = r.study_instance_uid
        AND e.action = 'purge_paused_hold'
        AND e.details ->> 'request_id' = r.id::TEXT
  );
