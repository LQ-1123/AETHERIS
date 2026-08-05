-- A remote station registers its own callback endpoint. It cannot receive routed studies until
-- an administrator explicitly approves the request.

ALTER TABLE dicom_route_destinations
    ADD COLUMN approval_status TEXT NOT NULL DEFAULT 'pending'
        CHECK (approval_status IN ('pending', 'approved')),
    ADD COLUMN approved_at TIMESTAMPTZ;

-- Destinations created before this approval workflow were already explicitly configured by an
-- administrator, so preserve their behavior during the migration.
UPDATE dicom_route_destinations
SET approval_status = 'approved', approved_at = COALESCE(updated_at, now());

CREATE INDEX dicom_route_destinations_approval_idx
    ON dicom_route_destinations (institution_id, approval_status, enabled, name);
