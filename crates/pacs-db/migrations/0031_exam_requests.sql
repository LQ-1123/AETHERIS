-- 检查申请单：先申请、后检查，独立于按序列生成的 diagnostic_work_items。
CREATE TABLE exam_requests (
    id                  UUID PRIMARY KEY,
    institution_id      BIGINT NOT NULL REFERENCES institutions(id),
    patient_id           TEXT NOT NULL,
    patient_name         TEXT NOT NULL,
    patient_birth_date   DATE,
    patient_sex          TEXT,
    modality             TEXT NOT NULL,
    body_part            TEXT NOT NULL,
    request_type         TEXT NOT NULL,
    clinical_indication  TEXT NOT NULL,
    requested_by         BIGINT NOT NULL REFERENCES users(id),
    requested_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    scheduled_at         TIMESTAMPTZ,
    status               TEXT NOT NULL DEFAULT 'pending',
    study_fk             BIGINT REFERENCES studies(id),
    revision             INTEGER NOT NULL DEFAULT 1,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT exam_requests_status_known
        CHECK (status IN ('pending', 'executed', 'completed')),
    CONSTRAINT exam_requests_patient_id_present CHECK (length(btrim(patient_id)) BETWEEN 1 AND 64),
    CONSTRAINT exam_requests_patient_name_present CHECK (length(btrim(patient_name)) BETWEEN 1 AND 256),
    CONSTRAINT exam_requests_modality_present CHECK (length(btrim(modality)) BETWEEN 1 AND 16),
    CONSTRAINT exam_requests_body_part_present CHECK (length(btrim(body_part)) BETWEEN 1 AND 128),
    CONSTRAINT exam_requests_type_present CHECK (length(btrim(request_type)) BETWEEN 1 AND 64),
    CONSTRAINT exam_requests_indication_present CHECK (length(btrim(clinical_indication)) BETWEEN 1 AND 4096),
    CONSTRAINT exam_requests_sex_length CHECK (patient_sex IS NULL OR length(btrim(patient_sex)) BETWEEN 1 AND 16),
    CONSTRAINT exam_requests_study_unique UNIQUE (institution_id, study_fk)
);

CREATE INDEX exam_requests_queue_idx
    ON exam_requests(institution_id, status, requested_at DESC);
CREATE INDEX exam_requests_requester_idx
    ON exam_requests(institution_id, requested_by, requested_at DESC);
CREATE TRIGGER exam_requests_set_updated_at BEFORE UPDATE ON exam_requests
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
