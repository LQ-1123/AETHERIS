//! WADO-RS 的取回查询:按 UID 找到盘上的文件。

use sqlx::PgPool;

use crate::DbError;

/// 一个实例在盘上的位置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredInstance {
    /// 相对存储根的路径。交给 `pacs_store::Store::resolve_for_read` 还原成绝对路径 ——
    /// **不要自己拼接**,那样绕过了路径逃逸校验。
    pub storage_path: String,
    pub file_size: i64,
    pub sop_instance_uid: String,
    pub sop_class_uid: Option<String>,
    pub transfer_syntax_uid: String,
}

/// 按三层 UID 定位一个实例。
///
/// 三个 UID 都要匹配,不能只用 SOPInstanceUID。虽然它本身唯一,但 URL 里的
/// 三段路径是调用方的断言(「这个实例在这个序列、这个检查下」),
/// 断言不成立时应当回 404 而不是照样把文件给它 —— 否则
/// `/studies/A/series/B/instances/C` 会在 C 其实属于别的检查时也返回内容,
/// 而调用方由此推断出的层级关系是错的。
pub async fn find_instance(
    pool: &PgPool,
    study_uid: &str,
    series_uid: &str,
    sop_uid: &str,
) -> Result<Option<StoredInstance>, DbError> {
    let row: Option<(String, i64, String, Option<String>, String)> = sqlx::query_as(
        "SELECT i.storage_path, i.file_size, i.sop_instance_uid,
                i.sop_class_uid, i.transfer_syntax_uid
         FROM instances i
         JOIN series se ON i.series_fk = se.id
         JOIN studies st ON se.study_fk = st.id
         WHERE st.study_instance_uid = $1
           AND se.series_instance_uid = $2
           AND i.sop_instance_uid = $3",
    )
    .bind(study_uid)
    .bind(series_uid)
    .bind(sop_uid)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(
        |(storage_path, file_size, sop_instance_uid, sop_class_uid, transfer_syntax_uid)| {
            StoredInstance {
                storage_path,
                file_size,
                sop_instance_uid,
                sop_class_uid,
                transfer_syntax_uid,
            }
        },
    ))
}

/// 列出一个序列下的全部实例,按 InstanceNumber 排序。
///
/// 排序键是 `(instance_number, sop_instance_uid)`:InstanceNumber 可以为空、
/// 也可能重复(设备 bug),单靠它排序结果不稳定 —— 而不稳定的顺序会让
/// 分页和缓存都失效。用 UID 兜底保证全序。
///
/// **注意**:这个顺序不适合 CT 序列的空间排序。CT 断层要按
/// ImagePositionPatient 投影到切片法向量上算,InstanceNumber 不可靠
/// (见计划「查看器陷阱」)。那部分在阶段 6 的查看器里做。
pub async fn list_series_instances(
    pool: &PgPool,
    study_uid: &str,
    series_uid: &str,
) -> Result<Vec<StoredInstance>, DbError> {
    let rows: Vec<(String, i64, String, Option<String>, String)> = sqlx::query_as(
        "SELECT i.storage_path, i.file_size, i.sop_instance_uid,
                i.sop_class_uid, i.transfer_syntax_uid
         FROM instances i
         JOIN series se ON i.series_fk = se.id
         JOIN studies st ON se.study_fk = st.id
         WHERE st.study_instance_uid = $1
           AND se.series_instance_uid = $2
         ORDER BY i.instance_number NULLS LAST, i.sop_instance_uid",
    )
    .bind(study_uid)
    .bind(series_uid)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(storage_path, file_size, sop_instance_uid, sop_class_uid, transfer_syntax_uid)| {
                StoredInstance {
                    storage_path,
                    file_size,
                    sop_instance_uid,
                    sop_class_uid,
                    transfer_syntax_uid,
                }
            },
        )
        .collect())
}
