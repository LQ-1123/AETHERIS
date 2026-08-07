-- Clinical labels attached to a segment make persisted masks searchable without
-- overloading the free-form Segment label or description.
ALTER TABLE segmentation_segments
    ADD COLUMN tags TEXT[] NOT NULL DEFAULT '{}',
    ADD CONSTRAINT segmentation_segments_tags_count CHECK (cardinality(tags) <= 16);

CREATE INDEX segmentation_segments_tags_idx
    ON segmentation_segments USING GIN (tags);
