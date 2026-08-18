-- 审核人直接修改后签发的独立审计动作。
-- `rejected` 为兼容 0026 的历史数据保留，新流程不再写入。
ALTER TABLE report_review_events
    DROP CONSTRAINT report_review_events_action_known,
    ADD CONSTRAINT report_review_events_action_known
        CHECK (action IN (
            'submitted',
            'review_started',
            'approved',
            'rejected',
            'reviewer_modified'
        ));
