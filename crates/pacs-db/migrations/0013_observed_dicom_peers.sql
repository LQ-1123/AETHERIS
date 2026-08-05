-- DIMSE peers observed while connecting to the PACS SCP. An inbound association does not
-- reveal the peer's listening port, so these records stay separate from route destinations.

CREATE TABLE dicom_observed_peers (
    id                      BIGSERIAL   PRIMARY KEY,
    institution_id          BIGINT      NOT NULL REFERENCES institutions(id),
    calling_ae_title        TEXT        NOT NULL,
    remote_host             TEXT        NOT NULL,
    active_associations     INTEGER     NOT NULL DEFAULT 0 CHECK (active_associations >= 0),
    association_count       BIGINT      NOT NULL DEFAULT 0 CHECK (association_count >= 0),
    first_seen_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_disconnected_at    TIMESTAMPTZ,
    UNIQUE (institution_id, calling_ae_title, remote_host),
    CONSTRAINT dicom_observed_peers_ae_not_blank
        CHECK (length(btrim(calling_ae_title)) BETWEEN 1 AND 16),
    CONSTRAINT dicom_observed_peers_host_not_blank
        CHECK (length(btrim(remote_host)) > 0)
);

CREATE INDEX dicom_observed_peers_recent_idx
    ON dicom_observed_peers (institution_id, last_seen_at DESC, id);
