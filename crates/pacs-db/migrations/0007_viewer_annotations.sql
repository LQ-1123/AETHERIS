-- Viewer annotations are clinical collaboration data. They are stored separately
-- from immutable DICOM objects and scoped to the owning institution.
CREATE TABLE viewer_annotations (
    id                  UUID PRIMARY KEY,
    institution_id      BIGINT      NOT NULL REFERENCES institutions(id),
    study_instance_uid  TEXT        NOT NULL,
    series_instance_uid TEXT        NOT NULL,
    sop_instance_uid    TEXT,
    frame_number        INTEGER,
    coordinate_space    TEXT        NOT NULL,
    mpr_plane           TEXT,
    schema_version      INTEGER     NOT NULL DEFAULT 1,
    kind                TEXT        NOT NULL,
    geometry            JSONB       NOT NULL,
    revision            BIGINT      NOT NULL DEFAULT 1,
    created_by          BIGINT      REFERENCES users(id) ON DELETE SET NULL,
    modified_by         BIGINT      REFERENCES users(id) ON DELETE SET NULL,
    deleted_at          TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT viewer_annotations_coordinate_space_known
        CHECK (coordinate_space IN ('image', 'patient')),
    CONSTRAINT viewer_annotations_kind_known
        CHECK (kind IN ('length', 'arrow', 'ellipse_roi', 'rectangle_roi', 'angle', 'point_probe')),
    CONSTRAINT viewer_annotations_frame_positive
        CHECK (frame_number IS NULL OR frame_number > 0),
    CONSTRAINT viewer_annotations_target_consistent CHECK (
        (coordinate_space = 'image' AND sop_instance_uid IS NOT NULL AND frame_number IS NOT NULL AND mpr_plane IS NULL)
        OR
        (coordinate_space = 'patient' AND sop_instance_uid IS NULL AND frame_number IS NULL AND mpr_plane IN ('axial', 'coronal', 'sagittal'))
    )
);

CREATE INDEX viewer_annotations_series_updated_idx
    ON viewer_annotations (institution_id, study_instance_uid, series_instance_uid, updated_at, id);
CREATE INDEX viewer_annotations_sop_idx
    ON viewer_annotations (institution_id, sop_instance_uid, frame_number)
    WHERE deleted_at IS NULL;

CREATE TRIGGER viewer_annotations_set_updated_at BEFORE UPDATE ON viewer_annotations
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
