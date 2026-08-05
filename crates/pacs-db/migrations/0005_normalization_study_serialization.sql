-- Serialize automatic normalization by the stable database identity of a Study.
-- The visible StudyInstanceUID changes on every derived revision, so it cannot be used as the
-- concurrency key for C-STORE post-processing.

ALTER TABLE dicom_transform_jobs
    ADD COLUMN scope_study_fk BIGINT REFERENCES studies(id);

-- Preserve the scope of any automatic jobs created by the first implementation. Historical jobs
-- may stay NULL when their items no longer resolve to exactly one Study.
UPDATE dicom_transform_jobs j
SET scope_study_fk = scope.study_fk,
    target_type = 'study',
    target_key = st.study_instance_uid
FROM (
    SELECT item.job_fk, min(se.study_fk) AS study_fk
    FROM dicom_transform_items item
    JOIN instances i ON i.id = item.instance_fk
    JOIN series se ON se.id = i.series_fk
    GROUP BY item.job_fk
    HAVING count(DISTINCT se.study_fk) = 1
) scope
JOIN studies st ON st.id = scope.study_fk
WHERE j.id = scope.job_fk AND j.mode = 'normalize';

-- The previous instance-scoped implementation could queue one job per C-STORE response. Keep the
-- earliest queued job for each real scope; execution-time rebasing makes it cover every current
-- instance in the Study.
WITH duplicates AS (
    SELECT id,
           row_number() OVER (
               PARTITION BY institution_id, scope_study_fk, result_summary->>'calling_ae'
               ORDER BY created_at, id
           ) AS position
    FROM dicom_transform_jobs
    WHERE mode = 'normalize' AND status = 'queued' AND scope_study_fk IS NOT NULL
)
UPDATE dicom_transform_jobs j
SET status = 'failed', completed_at = now(), next_attempt_at = NULL,
    error_message = '升级时合并了同一检查的重复自动规范化任务'
FROM duplicates d
WHERE j.id = d.id AND d.position > 1;

CREATE UNIQUE INDEX dicom_transform_jobs_one_queued_normalization_idx
    ON dicom_transform_jobs (
        institution_id,
        scope_study_fk,
        (result_summary->>'calling_ae')
    )
    WHERE mode = 'normalize' AND status = 'queued' AND scope_study_fk IS NOT NULL;

CREATE INDEX dicom_transform_jobs_running_normalization_scope_idx
    ON dicom_transform_jobs (institution_id, scope_study_fk)
    WHERE mode = 'normalize' AND status = 'running' AND scope_study_fk IS NOT NULL;
