-- v0.3 模块 A + B：报告审核状态机 + 权限可配置（设计文档 §4.2 + §5.2）。
--
-- 模块 A：draft → submitted → under_review → signed | draft（退回）。
-- 机构级开关 institutions.review_required（默认 false，关闭时保留现有直签流程）。
-- 模块 B：user_permission_grants 用户级正向授予，生效规则 role.can(p) OR EXISTS(grant)。

-- 机构级审核开关：关闭时现有 draft → signed 直签流程原样保留（单医生/演示环境不卡死）。
ALTER TABLE institutions
    ADD COLUMN review_required BOOLEAN NOT NULL DEFAULT false;

-- 报告审核列 + 5 态 CHECK（先 DROP 旧约束再加新约束）。
ALTER TABLE diagnostic_reports
    ADD COLUMN submitted_at TIMESTAMPTZ,
    ADD COLUMN reviewer_fk   BIGINT REFERENCES users(id),
    ADD COLUMN reviewed_at   TIMESTAMPTZ,
    ADD COLUMN review_comment TEXT,
    DROP CONSTRAINT diagnostic_reports_status_known,
    ADD CONSTRAINT diagnostic_reports_status_known
        CHECK (status IN ('draft', 'submitted', 'under_review', 'signed', 'amending'));

-- 版本快照的审核人（approve 时与版本快照同事务写入，保持「签发即不可变」）。
ALTER TABLE diagnostic_report_versions
    ADD COLUMN reviewed_by BIGINT REFERENCES users(id),
    ADD COLUMN reviewed_at TIMESTAMPTZ;

-- 审核全链路留痕：谁提交/谁开始审/谁通过/谁退回、何时、意见。
-- 不依赖版本快照（reject 不产生版本）；拒绝事件不污染版本历史。
CREATE TABLE report_review_events (
    id          BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    report_fk   UUID NOT NULL REFERENCES diagnostic_reports(id) ON DELETE CASCADE,
    actor_fk    BIGINT NOT NULL REFERENCES users(id),
    action      TEXT NOT NULL,
    comment     TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT report_review_events_action_known
        CHECK (action IN ('submitted', 'review_started', 'approved', 'rejected'))
);

CREATE INDEX report_review_events_report_idx ON report_review_events(report_fk, created_at);

-- 用户级权限授予表：仅正向授予（不引入「撤销角色自带权限」的负向逻辑，保持矩阵可审计）。
-- 白名单只放权限位字符串，与 Permission::as_str 两侧一致（测试对照）。
CREATE TABLE user_permission_grants (
    user_fk      BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    permission   TEXT NOT NULL,
    granted_by   BIGINT NOT NULL REFERENCES users(id),
    granted_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_fk, permission),
    CONSTRAINT user_permission_grants_permission_known
        CHECK (permission IN ('review_report'))
);
