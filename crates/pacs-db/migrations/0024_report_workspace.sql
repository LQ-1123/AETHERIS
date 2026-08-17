-- B2-2 报告工作台：阳性标记进入报告当前态与不可变版本快照。
ALTER TABLE diagnostic_reports
    ADD COLUMN is_positive BOOLEAN NOT NULL DEFAULT false;

ALTER TABLE diagnostic_report_versions
    ADD COLUMN is_positive BOOLEAN NOT NULL DEFAULT false;
