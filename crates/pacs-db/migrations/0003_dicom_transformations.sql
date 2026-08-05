-- DICOM 标签修订、规范化与脱敏。
--
-- 原始文件永不覆盖。现有四层表继续作为“当前临床投影”，本迁移新增不可变的
-- 实例版本、持久化转换任务、模板、脱敏项目和 Calling AE 规范化策略。

ALTER TABLE series ADD COLUMN protocol_name TEXT;

-- stable logical identity, independent of the SOP Instance UID of each revision.
ALTER TABLE instances ADD COLUMN logical_instance_id UUID;
UPDATE instances
SET logical_instance_id = md5('remote-pacs-instance:' || id::text || ':' || sop_instance_uid)::uuid
WHERE logical_instance_id IS NULL;
ALTER TABLE instances ALTER COLUMN logical_instance_id SET NOT NULL;
ALTER TABLE instances ADD CONSTRAINT instances_logical_instance_id_key UNIQUE (logical_instance_id);

CREATE TABLE dicom_transform_templates (
    id              UUID        PRIMARY KEY,
    institution_id  BIGINT      NOT NULL REFERENCES institutions(id),
    name            TEXT        NOT NULL,
    version         INTEGER     NOT NULL CHECK (version > 0),
    mode            TEXT        NOT NULL CHECK (mode IN ('clinical_correction', 'deidentify', 'normalize')),
    rules           JSONB       NOT NULL,
    is_active       BOOLEAN     NOT NULL DEFAULT true,
    created_by      BIGINT      NOT NULL REFERENCES users(id),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (institution_id, name, version)
);

CREATE INDEX dicom_transform_templates_active_idx
    ON dicom_transform_templates (institution_id, mode, name)
    WHERE is_active;

CREATE TABLE deid_projects (
    id                 UUID        PRIMARY KEY,
    institution_id     BIGINT      NOT NULL REFERENCES institutions(id),
    name               TEXT        NOT NULL,
    pseudonym_prefix   TEXT        NOT NULL DEFAULT 'SUBJ',
    key_id             TEXT        NOT NULL,
    date_shift_min     INTEGER     NOT NULL DEFAULT -3650,
    date_shift_max     INTEGER     NOT NULL DEFAULT 3650,
    is_active          BOOLEAN     NOT NULL DEFAULT true,
    created_by         BIGINT      NOT NULL REFERENCES users(id),
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (institution_id, name),
    CONSTRAINT deid_project_shift_range CHECK (date_shift_min <= date_shift_max)
);

CREATE TRIGGER deid_projects_set_updated_at BEFORE UPDATE ON deid_projects
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE pseudonym_mappings (
    id                 BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    project_fk         UUID        NOT NULL REFERENCES deid_projects(id) ON DELETE CASCADE,
    original_hmac      BYTEA       NOT NULL,
    encrypted_original BYTEA       NOT NULL,
    nonce              BYTEA       NOT NULL,
    key_id             TEXT        NOT NULL,
    pseudonym          TEXT        NOT NULL,
    date_shift_days    INTEGER     NOT NULL,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_fk, original_hmac),
    UNIQUE (project_fk, pseudonym)
);

CREATE TABLE dicom_transform_jobs (
    id                   UUID        PRIMARY KEY,
    institution_id       BIGINT      NOT NULL REFERENCES institutions(id),
    created_by            BIGINT      REFERENCES users(id),
    username              TEXT,
    mode                  TEXT        NOT NULL CHECK (mode IN (
                              'clinical_correction', 'deidentify', 'normalize', 'rollback'
                          )),
    target_type           TEXT        NOT NULL CHECK (target_type IN (
                              'patient', 'study', 'series', 'instance'
                          )),
    target_key            TEXT        NOT NULL,
    base_revisions        JSONB       NOT NULL DEFAULT '{}'::jsonb,
    rules                 JSONB       NOT NULL DEFAULT '[]'::jsonb,
    template_fk           UUID        REFERENCES dicom_transform_templates(id),
    deid_project_fk       UUID        REFERENCES deid_projects(id),
    reason                TEXT        NOT NULL,
    status                TEXT        NOT NULL CHECK (status IN (
                              'previewed', 'queued', 'running', 'succeeded',
                              'failed', 'blocked', 'expired'
                          )),
    progress_completed    INTEGER     NOT NULL DEFAULT 0,
    progress_total        INTEGER     NOT NULL DEFAULT 0,
    confirmation_hash     BYTEA,
    confirmation_expires_at TIMESTAMPTZ,
    preview               JSONB       NOT NULL DEFAULT '{}'::jsonb,
    result_summary        JSONB       NOT NULL DEFAULT '{}'::jsonb,
    pixel_risk            TEXT        NOT NULL DEFAULT 'unknown' CHECK (pixel_risk IN (
                              'safe', 'review_required', 'blocking', 'unknown'
                          )),
    manifest              JSONB,
    archive_path          TEXT,
    expires_at            TIMESTAMPTZ,
    error_message         TEXT,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at            TIMESTAMPTZ,
    completed_at          TIMESTAMPTZ,
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT dicom_transform_reason_not_blank CHECK (length(btrim(reason)) > 0)
);

CREATE INDEX dicom_transform_jobs_list_idx
    ON dicom_transform_jobs (institution_id, created_at DESC);
CREATE INDEX dicom_transform_jobs_pending_idx
    ON dicom_transform_jobs (status, created_at)
    WHERE status IN ('queued', 'running');

CREATE TRIGGER dicom_transform_jobs_set_updated_at BEFORE UPDATE ON dicom_transform_jobs
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE dicom_instance_versions (
    id                    BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    logical_instance_id   UUID        NOT NULL,
    instance_fk           BIGINT      NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
    version_number        INTEGER     NOT NULL CHECK (version_number > 0),
    source_version_fk     BIGINT      REFERENCES dicom_instance_versions(id),
    transform_job_fk      UUID        REFERENCES dicom_transform_jobs(id),
    derivation_kind       TEXT        NOT NULL CHECK (derivation_kind IN (
                               'original', 'clinical_correction', 'normalize', 'rollback'
                           )),
    study_instance_uid    TEXT        NOT NULL,
    series_instance_uid   TEXT        NOT NULL,
    sop_instance_uid      TEXT        NOT NULL,
    source_sop_instance_uid TEXT,
    transfer_syntax_uid   TEXT        NOT NULL,
    storage_path          TEXT        NOT NULL,
    file_size             BIGINT      NOT NULL,
    file_sha256           BYTEA       NOT NULL,
    metadata_snapshot     JSONB       NOT NULL DEFAULT '{}'::jsonb,
    created_by            BIGINT      REFERENCES users(id),
    reason                TEXT        NOT NULL,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (logical_instance_id, version_number)
);

CREATE INDEX dicom_instance_versions_instance_idx
    ON dicom_instance_versions (instance_fk, version_number DESC);
CREATE INDEX dicom_instance_versions_job_idx
    ON dicom_instance_versions (transform_job_fk)
    WHERE transform_job_fk IS NOT NULL;
CREATE INDEX dicom_instance_versions_storage_path_idx
    ON dicom_instance_versions (storage_path);

-- Existing files become version 1 by reference. No bytes are copied or rewritten.
INSERT INTO dicom_instance_versions (
    logical_instance_id, instance_fk, version_number, derivation_kind,
    study_instance_uid, series_instance_uid, sop_instance_uid,
    transfer_syntax_uid, storage_path, file_size, file_sha256,
    metadata_snapshot, reason
)
SELECT
    i.logical_instance_id,
    i.id,
    1,
    'original',
    st.study_instance_uid,
    se.series_instance_uid,
    i.sop_instance_uid,
    i.transfer_syntax_uid,
    i.storage_path,
    i.file_size,
    i.file_sha256,
    jsonb_build_object(
        'patient', jsonb_build_object(
            'patient_id', p.patient_id,
            'issuer_of_patient_id', p.issuer_of_patient_id,
            'name', p.name,
            'birth_date', p.birth_date,
            'sex', p.sex
        ),
        'study', jsonb_build_object(
            'study_instance_uid', st.study_instance_uid,
            'study_date', st.study_date,
            'study_time', st.study_time,
            'accession_number', st.accession_number,
            'study_id', st.study_id,
            'description', st.description,
            'referring_physician', st.referring_physician
        ),
        'series', jsonb_build_object(
            'series_instance_uid', se.series_instance_uid,
            'series_number', se.series_number,
            'modality', se.modality,
            'description', se.description,
            'body_part_examined', se.body_part_examined,
            'protocol_name', se.protocol_name
        ),
        'instance', jsonb_build_object(
            'sop_instance_uid', i.sop_instance_uid,
            'sop_class_uid', i.sop_class_uid,
            'instance_number', i.instance_number
        )
    ),
    'existing archive backfill'
FROM instances i
JOIN series se ON i.series_fk = se.id
JOIN studies st ON se.study_fk = st.id
JOIN patients p ON st.patient_fk = p.id;

ALTER TABLE instances ADD COLUMN current_version_id BIGINT;
UPDATE instances i
SET current_version_id = v.id
FROM dicom_instance_versions v
WHERE v.instance_fk = i.id AND v.version_number = 1;
-- Kept nullable at the SQL type level to break the insert cycle: a new instance row must exist
-- before its version can reference it. The ingest transaction creates the version and fills this
-- column before commit, and integration tests enforce that no committed row is left without one.
ALTER TABLE instances ADD CONSTRAINT instances_current_version_fk
    FOREIGN KEY (current_version_id) REFERENCES dicom_instance_versions(id);

CREATE TABLE dicom_transform_items (
    id                  BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    job_fk              UUID        NOT NULL REFERENCES dicom_transform_jobs(id) ON DELETE CASCADE,
    logical_instance_id UUID        NOT NULL,
    instance_fk         BIGINT      REFERENCES instances(id) ON DELETE SET NULL,
    source_version_fk   BIGINT      NOT NULL REFERENCES dicom_instance_versions(id),
    output_version_fk   BIGINT      REFERENCES dicom_instance_versions(id),
    source_path         TEXT        NOT NULL,
    output_path         TEXT,
    uid_map             JSONB       NOT NULL DEFAULT '{}'::jsonb,
    status              TEXT        NOT NULL CHECK (status IN ('pending', 'staged', 'activated', 'failed')),
    error_message       TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (job_fk, logical_instance_id)
);

CREATE INDEX dicom_transform_items_job_idx ON dicom_transform_items (job_fk, id);
CREATE TRIGGER dicom_transform_items_set_updated_at BEFORE UPDATE ON dicom_transform_items
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE calling_ae_normalization_policies (
    id              UUID        PRIMARY KEY,
    institution_id  BIGINT      NOT NULL REFERENCES institutions(id),
    calling_ae      TEXT        NOT NULL,
    template_fk     UUID        NOT NULL REFERENCES dicom_transform_templates(id),
    priority        INTEGER     NOT NULL DEFAULT 100,
    is_active       BOOLEAN     NOT NULL DEFAULT true,
    created_by      BIGINT      NOT NULL REFERENCES users(id),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (institution_id, calling_ae, template_fk)
);

CREATE INDEX calling_ae_normalization_active_idx
    ON calling_ae_normalization_policies (institution_id, calling_ae, priority)
    WHERE is_active;
CREATE TRIGGER calling_ae_normalization_set_updated_at
    BEFORE UPDATE ON calling_ae_normalization_policies
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
