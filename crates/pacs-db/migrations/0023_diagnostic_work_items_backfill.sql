-- 为历史序列幂等补建待诊工作项（status 默认 'pending'）。
-- 新入库序列由 record_dimse_origin 实时创建（ON CONFLICT DO NOTHING），
-- 本迁移只负责补历史数据，重复执行安全。
INSERT INTO diagnostic_work_items (id, institution_id, series_fk)
SELECT gen_random_uuid(), st.institution_id, se.id
FROM series se
JOIN studies st ON st.id = se.study_fk
ON CONFLICT (institution_id, series_fk) DO NOTHING;
