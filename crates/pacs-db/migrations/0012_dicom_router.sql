-- Institution-scoped DICOM routing destinations and rules.

CREATE TABLE dicom_route_destinations (
    id                  UUID        PRIMARY KEY,
    institution_id      BIGINT      NOT NULL REFERENCES institutions(id),
    name                TEXT        NOT NULL,
    protocol            TEXT        NOT NULL CHECK (protocol IN ('dimse', 'stow')),
    enabled             BOOLEAN     NOT NULL DEFAULT true,
    host                TEXT,
    port                INTEGER     CHECK (port BETWEEN 1 AND 65535),
    called_ae_title     TEXT,
    calling_ae_title    TEXT,
    use_tls             BOOLEAN     NOT NULL DEFAULT false,
    stow_url            TEXT,
    auth_token          TEXT,
    ca_pem              TEXT,
    status              TEXT        NOT NULL DEFAULT 'unknown' CHECK (status IN ('unknown', 'online', 'offline')),
    last_checked_at     TIMESTAMPTZ,
    last_success_at     TIMESTAMPTZ,
    last_latency_ms     BIGINT      CHECK (last_latency_ms IS NULL OR last_latency_ms >= 0),
    last_error          TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (institution_id, name),
    CONSTRAINT dicom_route_destinations_name_not_blank CHECK (length(btrim(name)) > 0),
    CONSTRAINT dicom_route_destinations_protocol_fields CHECK (
        (protocol = 'dimse' AND host IS NOT NULL AND length(btrim(host)) > 0
            AND port IS NOT NULL AND called_ae_title IS NOT NULL
            AND length(btrim(called_ae_title)) BETWEEN 1 AND 16
            AND calling_ae_title IS NOT NULL AND length(btrim(calling_ae_title)) BETWEEN 1 AND 16
            AND stow_url IS NULL)
        OR
        (protocol = 'stow' AND stow_url IS NOT NULL AND length(btrim(stow_url)) > 0
            AND host IS NULL AND port IS NULL AND called_ae_title IS NULL
            AND calling_ae_title IS NULL AND use_tls = false)
    )
);

CREATE INDEX dicom_route_destinations_institution_idx
    ON dicom_route_destinations (institution_id, enabled, name);
CREATE TRIGGER dicom_route_destinations_set_updated_at BEFORE UPDATE ON dicom_route_destinations
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE dicom_route_rules (
    id                  UUID        PRIMARY KEY,
    institution_id      BIGINT      NOT NULL REFERENCES institutions(id),
    destination_fk      UUID        NOT NULL REFERENCES dicom_route_destinations(id) ON DELETE CASCADE,
    name                TEXT        NOT NULL,
    priority            INTEGER     NOT NULL DEFAULT 100,
    enabled             BOOLEAN     NOT NULL DEFAULT true,
    source_ae_title     TEXT,
    modality            TEXT,
    body_part_examined  TEXT,
    study_description   TEXT,
    series_description  TEXT,
    tag_matches         JSONB       NOT NULL DEFAULT '{}'::jsonb,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (institution_id, name),
    CONSTRAINT dicom_route_rules_name_not_blank CHECK (length(btrim(name)) > 0),
    CONSTRAINT dicom_route_rules_tag_object CHECK (jsonb_typeof(tag_matches) = 'object')
);

CREATE INDEX dicom_route_rules_match_idx
    ON dicom_route_rules (institution_id, enabled, priority, id);
CREATE TRIGGER dicom_route_rules_set_updated_at BEFORE UPDATE ON dicom_route_rules
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- One immutable current version can be delivered to a destination once. A manual replay
-- creates a new background job only for a failed delivery and keeps the same delivery row.
CREATE TABLE dicom_route_deliveries (
    id                  UUID        PRIMARY KEY,
    institution_id      BIGINT      NOT NULL REFERENCES institutions(id),
    destination_fk      UUID        NOT NULL REFERENCES dicom_route_destinations(id) ON DELETE CASCADE,
    rule_fk             UUID        REFERENCES dicom_route_rules(id) ON DELETE SET NULL,
    version_fk          BIGINT      NOT NULL REFERENCES dicom_instance_versions(id) ON DELETE CASCADE,
    sop_instance_uid    TEXT        NOT NULL,
    current_job_fk      UUID        REFERENCES background_jobs(id) ON DELETE SET NULL,
    status              TEXT        NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'running', 'succeeded', 'dead_letter')),
    attempts            INTEGER     NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    last_error          TEXT,
    delivered_at        TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (destination_fk, version_fk)
);

CREATE INDEX dicom_route_deliveries_list_idx
    ON dicom_route_deliveries (institution_id, created_at DESC, id);
CREATE TRIGGER dicom_route_deliveries_set_updated_at BEFORE UPDATE ON dicom_route_deliveries
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
