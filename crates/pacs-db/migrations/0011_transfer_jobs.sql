-- Resumable import uploads and completed export artifacts.

CREATE TABLE import_uploads (
    id                  UUID        PRIMARY KEY,
    job_fk              UUID        NOT NULL REFERENCES background_jobs(id) ON DELETE CASCADE,
    relative_name       TEXT        NOT NULL,
    expected_size       BIGINT      NOT NULL CHECK (expected_size >= 0),
    expected_sha256     BYTEA,
    received_size       BIGINT      NOT NULL DEFAULT 0 CHECK (received_size >= 0),
    temp_name           TEXT        NOT NULL UNIQUE,
    status              TEXT        NOT NULL DEFAULT 'uploading' CHECK (status IN (
                            'uploading', 'ready', 'failed'
                        )),
    error_message       TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (job_fk, relative_name),
    CONSTRAINT import_uploads_name_not_blank CHECK (length(btrim(relative_name)) > 0),
    CONSTRAINT import_uploads_temp_not_blank CHECK (length(btrim(temp_name)) > 0),
    CONSTRAINT import_uploads_size_valid CHECK (received_size <= expected_size),
    CONSTRAINT import_uploads_sha256_size CHECK (
        expected_sha256 IS NULL OR octet_length(expected_sha256) = 32
    )
);

CREATE INDEX import_uploads_job_idx ON import_uploads (job_fk, id);
CREATE TRIGGER import_uploads_set_updated_at BEFORE UPDATE ON import_uploads
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE export_artifacts (
    job_fk              UUID        PRIMARY KEY REFERENCES background_jobs(id) ON DELETE CASCADE,
    relative_path       TEXT        NOT NULL UNIQUE,
    file_size           BIGINT      NOT NULL CHECK (file_size >= 0),
    file_sha256         BYTEA       NOT NULL CHECK (octet_length(file_sha256) = 32),
    download_name       TEXT        NOT NULL CHECK (length(btrim(download_name)) > 0),
    expires_at          TIMESTAMPTZ NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX export_artifacts_expiry_idx ON export_artifacts (expires_at);
