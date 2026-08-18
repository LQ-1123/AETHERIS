-- 密码重置改为「用户提交新密码 -> 管理员审核」流程。
--
-- 申请中只保存 Argon2id 哈希，管理员既不能查看新密码，也不能代用户填写密码。
-- 同一用户最多一条 pending 申请；再次提交会覆盖尚未审核的申请。

CREATE TABLE password_reset_requests (
    id              BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    institution_id  BIGINT      NOT NULL REFERENCES institutions(id),
    user_fk         BIGINT      NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    password_hash   TEXT        NOT NULL,
    status          TEXT        NOT NULL DEFAULT 'pending',
    requested_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    reviewed_by     BIGINT      REFERENCES users(id) ON DELETE SET NULL,
    reviewed_at     TIMESTAMPTZ,
    CONSTRAINT password_reset_status_known
        CHECK (status IN ('pending', 'approved', 'rejected'))
);

CREATE UNIQUE INDEX password_reset_one_pending_per_user
    ON password_reset_requests (user_fk)
    WHERE status = 'pending';

CREATE INDEX password_reset_institution_status_idx
    ON password_reset_requests (institution_id, status, requested_at DESC);
