-- v0.4.0: external PACS retrieval sources and safe C-MOVE destinations.
-- Reuse dicom_devices so source identity, clinical grants and retrieval configuration
-- remain one institution-scoped record.

ALTER TABLE dicom_devices
    ADD COLUMN is_retrieval_source BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN retrieval_port INTEGER,
    ADD COLUMN retrieval_use_tls BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN retrieval_ca_pem TEXT,
    ADD CONSTRAINT dicom_devices_retrieval_port_valid
        CHECK (retrieval_port IS NULL OR retrieval_port BETWEEN 1 AND 65535),
    ADD CONSTRAINT dicom_devices_retrieval_config_complete
        CHECK (NOT is_retrieval_source OR retrieval_port IS NOT NULL);

CREATE INDEX dicom_devices_retrieval_sources_idx
    ON dicom_devices (institution_id, status, name)
    WHERE is_retrieval_source;
