-- Study-scoped storage lifecycle management.

ALTER TABLE studies
    ADD COLUMN storage_tier TEXT NOT NULL DEFAULT 'hot'
        CHECK (storage_tier IN ('hot', 'cold', 'quarantine')),
    ADD COLUMN last_accessed_at TIMESTAMPTZ,
    ADD COLUMN lifecycle_updated_at TIMESTAMPTZ NOT NULL DEFAULT now();

ALTER TABLE dicom_instance_versions
    ADD COLUMN storage_tier TEXT NOT NULL DEFAULT 'hot'
        CHECK (storage_tier IN ('hot', 'cold', 'quarantine'));

CREATE INDEX studies_lifecycle_match_idx
    ON studies (institution_id, storage_tier, study_date, last_accessed_at);
CREATE INDEX dicom_instance_versions_storage_tier_idx
    ON dicom_instance_versions (storage_tier, instance_fk);

CREATE TABLE dicom_lifecycle_policies (
    id                          UUID        PRIMARY KEY,
    institution_id              BIGINT      NOT NULL REFERENCES institutions(id),
    name                        TEXT        NOT NULL,
    priority                    INTEGER     NOT NULL DEFAULT 100,
    enabled                     BOOLEAN     NOT NULL DEFAULT false,
    target_tier                 TEXT        NOT NULL CHECK (target_tier IN ('cold', 'quarantine')),
    modalities                  TEXT[]      NOT NULL DEFAULT '{}',
    study_date_before           DATE,
    last_accessed_before        TIMESTAMPTZ,
    tag_matches                 JSONB       NOT NULL DEFAULT '{}'::jsonb,
    minimum_study_bytes         BIGINT      CHECK (minimum_study_bytes IS NULL OR minimum_study_bytes >= 0),
    minimum_storage_used_percent DOUBLE PRECISION CHECK (
        minimum_storage_used_percent IS NULL OR
        minimum_storage_used_percent BETWEEN 0 AND 100
    ),
    preview_signature           BYTEA,
    last_preview_at             TIMESTAMPTZ,
    last_preview                JSONB       NOT NULL DEFAULT '{}'::jsonb,
    last_run_at                 TIMESTAMPTZ,
    created_by                  BIGINT      REFERENCES users(id) ON DELETE SET NULL,
    created_at                  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (institution_id, name),
    CONSTRAINT dicom_lifecycle_policy_name_not_blank CHECK (length(btrim(name)) > 0)
);

CREATE INDEX dicom_lifecycle_policies_order_idx
    ON dicom_lifecycle_policies (institution_id, enabled DESC, priority, created_at);
CREATE TRIGGER dicom_lifecycle_policies_set_updated_at BEFORE UPDATE ON dicom_lifecycle_policies
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE dicom_legal_holds (
    id                  UUID        PRIMARY KEY,
    institution_id      BIGINT      NOT NULL REFERENCES institutions(id),
    study_fk            BIGINT      REFERENCES studies(id) ON DELETE SET NULL,
    study_instance_uid  TEXT        NOT NULL,
    reason              TEXT        NOT NULL,
    expires_at          TIMESTAMPTZ,
    created_by          BIGINT      REFERENCES users(id) ON DELETE SET NULL,
    released_by         BIGINT      REFERENCES users(id) ON DELETE SET NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    released_at         TIMESTAMPTZ,
    CONSTRAINT dicom_legal_hold_reason_not_blank CHECK (length(btrim(reason)) > 0),
    CONSTRAINT dicom_legal_hold_release_complete CHECK (
        (released_at IS NULL AND released_by IS NULL) OR released_at IS NOT NULL
    )
);

CREATE UNIQUE INDEX dicom_legal_holds_active_idx
    ON dicom_legal_holds (institution_id, study_instance_uid)
    WHERE released_at IS NULL;

CREATE TABLE dicom_purge_requests (
    id                  UUID        PRIMARY KEY,
    institution_id      BIGINT      NOT NULL REFERENCES institutions(id),
    study_fk            BIGINT      REFERENCES studies(id) ON DELETE SET NULL,
    study_instance_uid  TEXT        NOT NULL,
    reason              TEXT        NOT NULL,
    status              TEXT        NOT NULL DEFAULT 'pending' CHECK (status IN (
                            'pending', 'approved', 'executing', 'completed',
                            'rejected', 'cancelled', 'failed'
                        )),
    grace_until         TIMESTAMPTZ,
    requested_by        BIGINT      REFERENCES users(id) ON DELETE SET NULL,
    approved_by         BIGINT      REFERENCES users(id) ON DELETE SET NULL,
    job_fk              UUID        REFERENCES background_jobs(id) ON DELETE SET NULL,
    error_message       TEXT,
    requested_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    approved_at         TIMESTAMPTZ,
    completed_at        TIMESTAMPTZ,
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT dicom_purge_request_reason_not_blank CHECK (length(btrim(reason)) > 0)
);

CREATE UNIQUE INDEX dicom_purge_requests_open_idx
    ON dicom_purge_requests (institution_id, study_instance_uid)
    WHERE status IN ('pending', 'approved', 'executing');
CREATE INDEX dicom_purge_requests_list_idx
    ON dicom_purge_requests (institution_id, requested_at DESC);
CREATE TRIGGER dicom_purge_requests_set_updated_at BEFORE UPDATE ON dicom_purge_requests
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- Kept after Study deletion so a retried worker can finish physical deletion safely.
CREATE TABLE dicom_purge_files (
    request_fk         UUID        NOT NULL REFERENCES dicom_purge_requests(id) ON DELETE CASCADE,
    storage_kind       TEXT        NOT NULL CHECK (storage_kind IN ('dicom', 'export')),
    relative_path      TEXT        NOT NULL,
    file_size          BIGINT      NOT NULL CHECK (file_size >= 0),
    file_sha256        BYTEA       NOT NULL CHECK (octet_length(file_sha256) = 32),
    deleted_at         TIMESTAMPTZ,
    PRIMARY KEY (request_fk, storage_kind, relative_path)
);

CREATE TABLE dicom_lifecycle_events (
    id                  BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    institution_id      BIGINT      NOT NULL REFERENCES institutions(id),
    study_instance_uid  TEXT        NOT NULL,
    action              TEXT        NOT NULL CHECK (action IN (
                            'move_to_cold', 'restore_to_hot', 'quarantine',
                            'purge_requested', 'purge_approved', 'purge_rejected',
                            'purged', 'legal_hold_created', 'legal_hold_released'
                        )),
    from_tier           TEXT,
    to_tier             TEXT,
    job_fk              UUID        REFERENCES background_jobs(id) ON DELETE SET NULL,
    actor_fk            BIGINT      REFERENCES users(id) ON DELETE SET NULL,
    details             JSONB       NOT NULL DEFAULT '{}'::jsonb,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX dicom_lifecycle_events_study_idx
    ON dicom_lifecycle_events (institution_id, study_instance_uid, created_at DESC);

CREATE FUNCTION reject_lifecycle_event_mutation() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'dicom_lifecycle_events is append-only';
END;
$$;

CREATE TRIGGER dicom_lifecycle_events_no_update
    BEFORE UPDATE OR DELETE ON dicom_lifecycle_events
    FOR EACH ROW EXECUTE FUNCTION reject_lifecycle_event_mutation();
