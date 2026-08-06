-- Editable segmentation state is kept separate from lightweight viewer annotations.
-- Masks use compact binary RLE and optimistic frame-level revisions.
CREATE TABLE segmentation_projects (
    id                  UUID PRIMARY KEY,
    institution_id      BIGINT      NOT NULL REFERENCES institutions(id),
    series_fk           BIGINT      NOT NULL REFERENCES series(id) ON DELETE CASCADE,
    study_instance_uid  TEXT        NOT NULL,
    series_instance_uid TEXT        NOT NULL,
    name                TEXT        NOT NULL,
    status              TEXT        NOT NULL DEFAULT 'draft',
    revision            BIGINT      NOT NULL DEFAULT 1,
    created_by          BIGINT      REFERENCES users(id) ON DELETE SET NULL,
    modified_by         BIGINT      REFERENCES users(id) ON DELETE SET NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT segmentation_projects_name_valid CHECK (char_length(name) BETWEEN 1 AND 120),
    CONSTRAINT segmentation_projects_status_known CHECK (status IN ('draft', 'published', 'archived'))
);

CREATE INDEX segmentation_projects_series_idx
    ON segmentation_projects (institution_id, series_fk, updated_at DESC);

CREATE TABLE segmentation_segments (
    id             UUID PRIMARY KEY,
    project_fk     UUID        NOT NULL REFERENCES segmentation_projects(id) ON DELETE CASCADE,
    segment_number INTEGER     NOT NULL,
    label          TEXT        NOT NULL,
    description    TEXT,
    color_r        SMALLINT    NOT NULL,
    color_g        SMALLINT    NOT NULL,
    color_b        SMALLINT    NOT NULL,
    algorithm_type TEXT        NOT NULL DEFAULT 'manual',
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT segmentation_segments_number_positive CHECK (segment_number BETWEEN 1 AND 65535),
    CONSTRAINT segmentation_segments_label_valid CHECK (char_length(label) BETWEEN 1 AND 120),
    CONSTRAINT segmentation_segments_color_valid CHECK (
        color_r BETWEEN 0 AND 255 AND color_g BETWEEN 0 AND 255 AND color_b BETWEEN 0 AND 255
    ),
    CONSTRAINT segmentation_segments_algorithm_known CHECK (
        algorithm_type IN ('manual', 'semiautomatic', 'automatic')
    ),
    UNIQUE (project_fk, segment_number)
);

CREATE TABLE segmentation_masks (
    segment_fk        UUID        NOT NULL REFERENCES segmentation_segments(id) ON DELETE CASCADE,
    sop_instance_uid  TEXT        NOT NULL,
    frame_number      INTEGER     NOT NULL,
    rows              INTEGER     NOT NULL,
    cols              INTEGER     NOT NULL,
    encoding          TEXT        NOT NULL DEFAULT 'rle-v1',
    mask_data         BYTEA       NOT NULL,
    revision          BIGINT      NOT NULL DEFAULT 1,
    modified_by       BIGINT      REFERENCES users(id) ON DELETE SET NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (segment_fk, sop_instance_uid, frame_number),
    CONSTRAINT segmentation_masks_frame_positive CHECK (frame_number > 0),
    CONSTRAINT segmentation_masks_dimensions_valid CHECK (rows > 0 AND cols > 0 AND rows <= 65535 AND cols <= 65535),
    CONSTRAINT segmentation_masks_encoding_known CHECK (encoding = 'rle-v1'),
    CONSTRAINT segmentation_masks_data_bounded CHECK (octet_length(mask_data) <= 67108864)
);

CREATE INDEX segmentation_masks_source_idx
    ON segmentation_masks (sop_instance_uid, frame_number);

CREATE TRIGGER segmentation_projects_set_updated_at BEFORE UPDATE ON segmentation_projects
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
CREATE TRIGGER segmentation_segments_set_updated_at BEFORE UPDATE ON segmentation_segments
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
CREATE TRIGGER segmentation_masks_set_updated_at BEFORE UPDATE ON segmentation_masks
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
