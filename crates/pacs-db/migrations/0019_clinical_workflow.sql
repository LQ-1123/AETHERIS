-- Device-scoped clinical access, diagnostic work queue and immutable reports.

ALTER TABLE institutions
    ADD COLUMN timezone TEXT NOT NULL DEFAULT 'Asia/Shanghai';

CREATE TABLE dicom_devices (
    id                  UUID PRIMARY KEY,
    institution_id      BIGINT NOT NULL REFERENCES institutions(id),
    name                TEXT NOT NULL,
    calling_ae_title    TEXT NOT NULL,
    source_ip           TEXT NOT NULL,
    modality_hint       TEXT,
    status              TEXT NOT NULL DEFAULT 'pending',
    approved_by         BIGINT REFERENCES users(id),
    approved_at         TIMESTAMPTZ,
    first_seen_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (institution_id, calling_ae_title, source_ip),
    CONSTRAINT dicom_devices_status_known
        CHECK (status IN ('pending', 'active', 'disabled')),
    CONSTRAINT dicom_devices_ae_length
        CHECK (length(btrim(calling_ae_title)) BETWEEN 1 AND 16)
);

CREATE TRIGGER dicom_devices_set_updated_at BEFORE UPDATE ON dicom_devices
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

ALTER TABLE series
    ADD COLUMN source_device_fk UUID REFERENCES dicom_devices(id),
    ADD COLUMN source_status TEXT NOT NULL DEFAULT 'legacy_unattributed',
    ADD CONSTRAINT series_source_status_known
        CHECK (source_status IN ('trusted', 'pending', 'needs_review', 'legacy_unattributed'));

CREATE INDEX series_source_device_idx ON series(source_device_fk, study_fk);

CREATE TABLE user_device_grants (
    user_fk       BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_fk     UUID NOT NULL REFERENCES dicom_devices(id) ON DELETE CASCADE,
    granted_by    BIGINT NOT NULL REFERENCES users(id),
    granted_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_fk, device_fk)
);

CREATE TABLE service_account_device_grants (
    service_account_fk UUID NOT NULL REFERENCES service_accounts(id) ON DELETE CASCADE,
    device_fk          UUID NOT NULL REFERENCES dicom_devices(id) ON DELETE CASCADE,
    granted_by         BIGINT NOT NULL REFERENCES users(id),
    granted_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (service_account_fk, device_fk)
);

-- The first accepted origin is immutable. Retransmission through another peer must not
-- silently broaden or change clinical access.
CREATE TABLE dicom_instance_origins (
    instance_fk      BIGINT PRIMARY KEY REFERENCES instances(id) ON DELETE CASCADE,
    device_fk        UUID REFERENCES dicom_devices(id),
    calling_ae_title TEXT,
    source_ip        TEXT,
    ingress_kind     TEXT NOT NULL,
    received_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT dicom_instance_origins_ingress_known
        CHECK (ingress_kind IN ('dimse', 'stow', 'import', 'legacy'))
);

-- Existing files have no trustworthy source evidence.
INSERT INTO dicom_instance_origins (instance_fk, ingress_kind)
SELECT id, 'legacy' FROM instances;

CREATE INDEX dicom_instance_origins_device_idx
    ON dicom_instance_origins(device_fk, instance_fk);

CREATE TABLE diagnostic_work_items (
    id                 UUID PRIMARY KEY,
    institution_id     BIGINT NOT NULL REFERENCES institutions(id),
    series_fk          BIGINT NOT NULL REFERENCES series(id) ON DELETE CASCADE,
    status             TEXT NOT NULL DEFAULT 'pending',
    assignee_fk        BIGINT REFERENCES users(id),
    claimed_at         TIMESTAMPTZ,
    completed_at       TIMESTAMPTZ,
    revision           INTEGER NOT NULL DEFAULT 1,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (institution_id, series_fk),
    CONSTRAINT diagnostic_work_status_known
        CHECK (status IN ('pending', 'claimed', 'reporting', 'completed'))
);

CREATE INDEX diagnostic_work_queue_idx
    ON diagnostic_work_items(institution_id, status, created_at DESC);
CREATE TRIGGER diagnostic_work_items_set_updated_at BEFORE UPDATE ON diagnostic_work_items
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE diagnostic_reports (
    id                 UUID PRIMARY KEY,
    institution_id     BIGINT NOT NULL REFERENCES institutions(id),
    study_fk           BIGINT NOT NULL REFERENCES studies(id) ON DELETE CASCADE,
    author_fk          BIGINT NOT NULL REFERENCES users(id),
    status             TEXT NOT NULL DEFAULT 'draft',
    findings           TEXT NOT NULL DEFAULT '',
    impression         TEXT NOT NULL DEFAULT '',
    recommendation     TEXT,
    revision           INTEGER NOT NULL DEFAULT 1,
    access_incomplete  BOOLEAN NOT NULL DEFAULT false,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT diagnostic_reports_status_known
        CHECK (status IN ('draft', 'signed', 'amending'))
);

CREATE INDEX diagnostic_reports_study_idx
    ON diagnostic_reports(institution_id, study_fk, updated_at DESC);
CREATE TRIGGER diagnostic_reports_set_updated_at BEFORE UPDATE ON diagnostic_reports
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE diagnostic_report_series (
    report_fk UUID NOT NULL REFERENCES diagnostic_reports(id) ON DELETE CASCADE,
    series_fk BIGINT NOT NULL REFERENCES series(id),
    PRIMARY KEY (report_fk, series_fk)
);

CREATE TABLE diagnostic_report_versions (
    id                 UUID PRIMARY KEY,
    report_fk          UUID NOT NULL REFERENCES diagnostic_reports(id) ON DELETE CASCADE,
    version_number     INTEGER NOT NULL,
    findings           TEXT NOT NULL,
    impression         TEXT NOT NULL,
    recommendation     TEXT,
    covered_series_uids TEXT[] NOT NULL,
    access_incomplete  BOOLEAN NOT NULL,
    amendment_reason   TEXT,
    signed_by          BIGINT NOT NULL REFERENCES users(id),
    signed_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (report_fk, version_number)
);

CREATE INDEX diagnostic_report_versions_report_idx
    ON diagnostic_report_versions(report_fk, version_number DESC);
