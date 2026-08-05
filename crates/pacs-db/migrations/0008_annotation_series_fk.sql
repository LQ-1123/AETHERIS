-- Add a relational owner after the initial UID-based annotation schema so
-- deleting a series also removes its Viewer annotations.
ALTER TABLE viewer_annotations ADD COLUMN series_fk BIGINT;

UPDATE viewer_annotations a
SET series_fk = se.id
FROM series se
JOIN studies st ON st.id = se.study_fk
WHERE st.institution_id = a.institution_id
  AND st.study_instance_uid = a.study_instance_uid
  AND se.series_instance_uid = a.series_instance_uid;

ALTER TABLE viewer_annotations ALTER COLUMN series_fk SET NOT NULL;
ALTER TABLE viewer_annotations
    ADD CONSTRAINT viewer_annotations_series_fk
    FOREIGN KEY (series_fk) REFERENCES series(id) ON DELETE CASCADE;
CREATE INDEX viewer_annotations_series_fk_idx ON viewer_annotations (series_fk);
