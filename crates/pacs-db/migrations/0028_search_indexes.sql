-- 搜索增强 v1（设计文档 §8.2，模块 D）：病人列表组合筛选 + keyset 分页支撑索引。
-- 编号 0028 由任务台账锁定（0026/0027 属并行模块 A/B/Phase0）。
--
-- 模态筛选不在此建索引：`studies.modalities` 是 TEXT[]，元素包含查询
-- `modalities @> ARRAY[...]` 由迁移 0001 的 GIN 索引 studies_modalities_idx 服务；
-- 设计原文 `modalities_in_study` 列不存在，且 btree 数组 opclass 无 contains 策略，
-- 机构前缀 btree 对元素查询不可用（审查 I-1，冗余已移除）。

-- keyset 分页 (study_date, id) 游标 + 日期范围筛选：按机构、日期倒序扫描。
-- 注意：游标键是聚合 MAX(study_date)，HAVING 无法下推到索引（见 worklist.rs 注释），
-- 该索引服务日期范围筛选与排序数据访问，不承诺消除排序。
CREATE INDEX studies_institution_date_idx
    ON studies(institution_id, study_date DESC, id);
