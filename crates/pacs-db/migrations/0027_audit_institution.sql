-- 审计日志按机构隔离（Phase 0 审计补链）。
--
-- audit_log 此前没有 institution_id：合规查询（某人某段时间做了什么、
-- 某个检查被谁看过）在单机构部署下够用，但多机构部署时管理员只能看
-- 本机构的数据，审计也必须按机构切分，否则管理员能读到别家机构的日志。
--
-- 回填规则与设计文档 §6.2 一致：按 user_fk 关联 users.institution_id；
-- 无法关联（用户已删除、登录失败等无 user_fk 的行）统一补机构 1（默认机构）。

ALTER TABLE audit_log
    ADD COLUMN institution_id BIGINT REFERENCES institutions(id);

-- 历史行回填：按 user_fk 关联用户机构
UPDATE audit_log a
SET institution_id = u.institution_id
FROM users u
WHERE a.user_fk = u.id AND a.institution_id IS NULL;

-- 无法关联的补机构 1（默认机构）
UPDATE audit_log SET institution_id = 1 WHERE institution_id IS NULL;

-- 默认 1：pacs_auth::audit::record 的 INSERT 不写 institution_id 列
-- （pacs-auth 的 Entry 不携带机构信息），无默认值则 SET NOT NULL 之后
-- 每一次审计写入都会违反非空约束而失败（审计写入是尽力而为、只记日志，
-- 失败会静默丢记录——比留 NULL 更危险）。单机构部署机构恒为 1，正确；
-- 多机构下新行落 1 是已知妥协，随 v0.5 权限体系化把机构信息接入 Entry 时修正。
ALTER TABLE audit_log ALTER COLUMN institution_id SET DEFAULT 1;
ALTER TABLE audit_log ALTER COLUMN institution_id SET NOT NULL;

-- 合规查询的典型形态：某机构某段时间按动作/用户筛
CREATE INDEX audit_log_institution_idx ON audit_log(institution_id, occurred_at DESC);
