-- Personal display-window presets follow the authenticated user across Viewer installations.
-- DICOM-provided VOI windows remain image metadata and are not copied into this table.
CREATE TABLE user_window_presets (
    id              BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    institution_id  BIGINT           NOT NULL REFERENCES institutions(id),
    user_fk         BIGINT           NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    modality        TEXT             NOT NULL,
    name            TEXT             NOT NULL,
    window_center   DOUBLE PRECISION NOT NULL,
    window_width    DOUBLE PRECISION NOT NULL,
    voi_function    TEXT             NOT NULL,
    created_at      TIMESTAMPTZ      NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ      NOT NULL DEFAULT now(),
    CONSTRAINT user_window_presets_modality_valid
        CHECK (modality ~ '^[A-Z0-9]{1,16}$'),
    CONSTRAINT user_window_presets_name_valid
        CHECK (name = btrim(name) AND char_length(name) BETWEEN 1 AND 64),
    CONSTRAINT user_window_presets_center_finite
        CHECK (window_center > '-Infinity'::double precision
           AND window_center < 'Infinity'::double precision),
    CONSTRAINT user_window_presets_width_positive_finite
        CHECK (window_width > 0
           AND window_width < 'Infinity'::double precision),
    CONSTRAINT user_window_presets_voi_function_known
        CHECK (voi_function IN ('LINEAR', 'LINEAR_EXACT', 'SIGMOID'))
);

CREATE UNIQUE INDEX user_window_presets_owner_modality_name_unique
    ON user_window_presets (institution_id, user_fk, modality, lower(name));

CREATE INDEX user_window_presets_owner_idx
    ON user_window_presets (institution_id, user_fk, modality, lower(name), id);

CREATE TRIGGER user_window_presets_set_updated_at BEFORE UPDATE ON user_window_presets
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
