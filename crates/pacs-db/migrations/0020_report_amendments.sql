-- Amendment reason is staged with the editable report and copied into the immutable
-- signed version in the same transaction as clinical completion and audit.
ALTER TABLE diagnostic_reports
    ADD COLUMN pending_amendment_reason TEXT;
