-- 账号体系与审计。
--
-- 与影像表分开一个迁移:影像那部分已经在跑了,账号是新增能力,
-- 分开便于回溯「哪次变更引入了什么」。

CREATE TABLE users (
    id              BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    institution_id  BIGINT      NOT NULL DEFAULT 1 REFERENCES institutions(id),
    username        TEXT        NOT NULL,
    display_name    TEXT,
    -- argon2id 的 PHC 字符串,自带盐和参数。参数会随硬件升级而调整,
    -- 所以必须存在哈希里而不是写死在代码里 —— 否则改参数就验不了老密码。
    password_hash   TEXT        NOT NULL,
    role            TEXT        NOT NULL,
    -- 停用而不是删除:删了会让审计日志里的外键悬空,
    -- 而「这个人当时做了什么」是必须留存的。
    is_active       BOOLEAN     NOT NULL DEFAULT true,
    -- 强制改密:管理员重置密码后,用户下次登录必须自己设一个新的,
    -- 免得初始密码长期沿用。
    must_change_password BOOLEAN NOT NULL DEFAULT false,
    last_login_at   TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (institution_id, username),
    CONSTRAINT users_role_known CHECK (role IN ('admin', 'radiologist', 'technician', 'viewer'))
);

CREATE TRIGGER users_set_updated_at BEFORE UPDATE ON users
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();


-- 不透明 refresh token。
--
-- 为什么不是纯 JWT:JWT 签发后无法吊销,只能等它过期。医疗场景里
-- 「员工离职」「设备丢失」要求立刻断访问,这是硬需求。所以 access token
-- 短命(15 分钟)且无状态,refresh token 存库、可吊销、可轮换。
CREATE TABLE refresh_tokens (
    id           BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    user_fk      BIGINT      NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- 存哈希不存原文:数据库泄露时,拿到的记录不能直接当令牌用。
    -- 这里用 SHA-256 而不是 argon2 —— token 是 256 位随机数,没有猜测空间,
    -- 不需要慢哈希;而 refresh 是热路径,慢哈希会拖慢每次续期。
    token_hash   BYTEA       NOT NULL UNIQUE,
    issued_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at   TIMESTAMPTZ NOT NULL,
    revoked_at   TIMESTAMPTZ,
    -- 轮换链。每次续期都换新令牌,旧的指向新的。
    -- 已经被换掉的令牌又被使用 → 说明它泄露了(合法客户端不会用旧的),
    -- 此时要把整条链全部吊销,而不只是拒绝这一次。
    replaced_by  BIGINT      REFERENCES refresh_tokens(id) ON DELETE SET NULL,
    user_agent   TEXT,
    client_ip    TEXT
);

CREATE INDEX refresh_tokens_user_fk_idx ON refresh_tokens (user_fk);
-- 定期清理过期令牌时按这个扫
CREATE INDEX refresh_tokens_expires_at_idx ON refresh_tokens (expires_at);


-- 审计日志。
--
-- 医疗合规的硬要求:谁在何时访问了哪个病人/检查。必须记在数据库而不是只写
-- 文件 —— 文件会轮转、会被删、查询也不方便。
CREATE TABLE audit_log (
    id            BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    occurred_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- 用户可能后来被删除,外键置空;但「是谁」不能跟着消失,
    -- 所以用户名在这里冗余存一份快照。
    user_fk       BIGINT      REFERENCES users(id) ON DELETE SET NULL,
    username      TEXT,
    -- 登录失败等场景下没有用户,但仍要记录尝试
    action        TEXT        NOT NULL,
    outcome       TEXT        NOT NULL,
    -- 被访问的对象。按层级冗余存 UID 而不是外键:
    -- 影像被删除后审计记录仍要能说明「当时访问的是哪一份」。
    patient_id            TEXT,
    study_instance_uid    TEXT,
    series_instance_uid   TEXT,
    sop_instance_uid      TEXT,
    -- 来源 IP 存字符串而不是 INET。INET 本来更合适(能做子网包含查询,
    -- 比如「院外网段的全部访问」),但那要额外拉 ipnetwork crate。
    -- 阶段 8 加固时真需要子网查询,一条
    -- `ALTER TABLE audit_log ALTER COLUMN client_ip TYPE inet USING client_ip::inet`
    -- 就能升级,信息本身没有损失。
    client_ip     TEXT,
    user_agent    TEXT,
    -- 各动作特有的细节,不为此不断加列
    detail        JSONB       NOT NULL DEFAULT '{}'::jsonb,
    CONSTRAINT audit_log_outcome_known CHECK (outcome IN ('success', 'failure', 'denied'))
);

-- 合规查询的典型形态:「某人某段时间做了什么」「某个检查被谁看过」
CREATE INDEX audit_log_occurred_at_idx ON audit_log (occurred_at DESC);
CREATE INDEX audit_log_user_fk_idx ON audit_log (user_fk, occurred_at DESC);
CREATE INDEX audit_log_study_idx ON audit_log (study_instance_uid, occurred_at DESC)
    WHERE study_instance_uid IS NOT NULL;
CREATE INDEX audit_log_patient_idx ON audit_log (patient_id, occurred_at DESC)
    WHERE patient_id IS NOT NULL;
