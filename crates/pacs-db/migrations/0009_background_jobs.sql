-- Shared durable jobs for imports, exports, routing and lifecycle operations.
-- DICOM transformation jobs remain separate because they have a clinical
-- preview/confirmation state machine which does not fit this queue.

CREATE TABLE background_jobs (
    id                  UUID        PRIMARY KEY,
    institution_id      BIGINT      NOT NULL REFERENCES institutions(id),
    created_by           BIGINT      REFERENCES users(id) ON DELETE SET NULL,
    kind                TEXT        NOT NULL CHECK (kind IN (
                            'import', 'export', 'route', 'lifecycle'
                        )),
    status              TEXT        NOT NULL DEFAULT 'queued' CHECK (status IN (
                            'queued', 'running', 'succeeded', 'failed', 'cancelled'
                        )),
    idempotency_key     TEXT,
    payload             JSONB       NOT NULL DEFAULT '{}'::jsonb,
    result              JSONB       NOT NULL DEFAULT '{}'::jsonb,
    progress_completed  BIGINT      NOT NULL DEFAULT 0 CHECK (progress_completed >= 0),
    progress_total      BIGINT      NOT NULL DEFAULT 0 CHECK (progress_total >= 0),
    attempts            INTEGER     NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    max_attempts        INTEGER     NOT NULL DEFAULT 3 CHECK (max_attempts > 0),
    available_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    lease_owner         UUID,
    lease_expires_at    TIMESTAMPTZ,
    cancel_requested    BOOLEAN     NOT NULL DEFAULT false,
    error_message       TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at          TIMESTAMPTZ,
    completed_at        TIMESTAMPTZ,
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT background_jobs_progress_valid CHECK (
        progress_total = 0 OR progress_completed <= progress_total
    ),
    CONSTRAINT background_jobs_lease_complete CHECK (
        (lease_owner IS NULL) = (lease_expires_at IS NULL)
    ),
    CONSTRAINT background_jobs_idempotency_not_blank CHECK (
        idempotency_key IS NULL OR length(btrim(idempotency_key)) > 0
    )
);

CREATE UNIQUE INDEX background_jobs_idempotency_idx
    ON background_jobs (institution_id, kind, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
CREATE INDEX background_jobs_runnable_idx
    ON background_jobs (available_at, created_at, id)
    WHERE status = 'queued' AND cancel_requested = false;
CREATE INDEX background_jobs_list_idx
    ON background_jobs (institution_id, created_at DESC, id);
CREATE INDEX background_jobs_expired_lease_idx
    ON background_jobs (lease_expires_at)
    WHERE status = 'running';

CREATE TRIGGER background_jobs_set_updated_at BEFORE UPDATE ON background_jobs
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE background_job_items (
    id                  BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    job_fk              UUID        NOT NULL REFERENCES background_jobs(id) ON DELETE CASCADE,
    item_key            TEXT        NOT NULL,
    status              TEXT        NOT NULL DEFAULT 'pending' CHECK (status IN (
                            'pending', 'running', 'succeeded', 'skipped',
                            'conflict', 'failed', 'cancelled'
                        )),
    attempts            INTEGER     NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    input               JSONB       NOT NULL DEFAULT '{}'::jsonb,
    result              JSONB       NOT NULL DEFAULT '{}'::jsonb,
    error_message       TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at          TIMESTAMPTZ,
    completed_at        TIMESTAMPTZ,
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (job_fk, item_key),
    CONSTRAINT background_job_items_key_not_blank CHECK (length(btrim(item_key)) > 0)
);

CREATE INDEX background_job_items_status_idx
    ON background_job_items (job_fk, status, id);

CREATE TRIGGER background_job_items_set_updated_at BEFORE UPDATE ON background_job_items
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
