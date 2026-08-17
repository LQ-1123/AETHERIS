-- 报告按检查一份：一个 study 最多一份报告（草稿/签发共用同一行，修订走 versions 表）。
-- 开发库已验证无重复；若存在历史重复会显式失败，需先人工去重。
CREATE UNIQUE INDEX one_report_per_study
    ON diagnostic_reports(study_fk);
