//! C-STORE 的落地实现:落盘 + 入库。
//!
//! # 顺序就是可靠性
//!
//! C-STORE 回 `0x0000` 是对发送方的承诺 —— 设备收到成功响应后真的会删本地
//! 副本。所以顺序必须是:
//!
//! 1. 文件落盘并 fsync(`pacs-store` 保证)
//! 2. 元数据入库,单个事务提交(`pacs-db` 保证)
//! 3. 才返回成功
//!
//! 反过来先写库再落盘的话,一次断电就会留下「库里有记录、盘上没文件」的
//! 影像 —— 查得到却取不回来,比明确的失败更糟。
//!
//! 第 2 步失败时盘上会留下一个孤儿文件。这是有意的取舍:宁可多一个孤儿文件
//! (定期核对可以清理),也不能少一份影像。

use pacs_db::{StorageRecord, ingest_instance};
use pacs_dimse::{
    FindFailure, FindHandler, FindRequest, FindResponse, IncomingInstance, StoreFailure,
    StoreHandler,
};
use pacs_store::{InstanceKey, Store, StoreOutcome};
use sqlx::PgPool;

pub struct PacsStoreHandler {
    store: Store,
    pool: PgPool,
    /// 单次 C-FIND 的结果条数上限。
    find_limit: usize,
}

impl PacsStoreHandler {
    pub fn new(store: Store, pool: PgPool) -> Self {
        Self {
            store,
            pool,
            find_limit: pacs_db::DEFAULT_LIMIT,
        }
    }
}

impl StoreHandler for PacsStoreHandler {
    async fn store(&self, instance: IncomingInstance<'_>) -> Result<(), StoreFailure> {
        // 元数据在关联线程里就已经解析好了(`pacs-dimse` 收完数据集时提取),
        // 这里不再重新读盘解析一遍 —— 那既慢又可能与已落盘的字节不一致。
        let metadata = instance.metadata;

        let stored = self
            .store
            .store(
                InstanceKey {
                    study: &metadata.study.uid,
                    series: &metadata.series.uid,
                    sop: &metadata.instance.uid,
                },
                instance.file_bytes,
            )
            .await
            .map_err(|error| classify(&error))?;

        if stored.outcome == StoreOutcome::Replaced {
            tracing::warn!(
                calling_ae_title = instance.calling_ae_title,
                sop_instance_uid = %metadata.instance.uid,
                "同一 SOPInstanceUID 收到了不同内容,已覆盖"
            );
        }

        ingest_instance(
            &self.pool,
            metadata,
            StorageRecord {
                relative_path: &stored.relative_path,
                size: stored.size,
                sha256: &stored.sha256,
            },
        )
        .await
        .map_err(|error| {
            tracing::error!(
                %error,
                path = %stored.relative_path,
                "入库失败,盘上留下孤儿文件"
            );
            StoreFailure::Processing(error.to_string())
        })?;

        tracing::debug!(
            calling_ae_title = instance.calling_ae_title,
            sop_instance_uid = %metadata.instance.uid,
            bytes = stored.size,
            "影像已存储"
        );
        Ok(())
    }
}

impl FindHandler for PacsStoreHandler {
    async fn find(&self, request: FindRequest<'_>) -> Result<FindResponse, FindFailure> {
        let results = pacs_db::find(&self.pool, request.query, self.find_limit)
            .await
            .map_err(|error| match error {
                // 结果太多是查询条件的问题,不是服务端故障 —— 让对端收窄条件重来。
                // 用 Unsupported 而不是 Processing:后者会让对方以为是我们坏了。
                pacs_db::DbError::TooManyResults { .. } => {
                    FindFailure::Unsupported(error.to_string())
                }
                other => FindFailure::Processing(other.to_string()),
            })?;

        if !results.unsupported_keys.is_empty() {
            tracing::info!(
                calling_ae_title = request.calling_ae_title,
                level = request.query.level.as_str(),
                keys = ?results.unsupported_keys,
                "查询含本层不支持的匹配键,已忽略并降级为 0xFF01"
            );
        }

        Ok(FindResponse {
            keys_unsupported: !results.unsupported_keys.is_empty(),
            identifiers: results.identifiers,
        })
    }
}

/// 把落盘错误分成「资源不足」和「其他」。
///
/// 这个区分对发送方有实际意义:资源不足是暂时的,值得稍后重传;
/// 其他失败重传也是白搭。
fn classify(error: &pacs_store::StoreError) -> StoreFailure {
    use std::io::ErrorKind;

    // 刻意逐个列举而不用 `_ =>`:StoreError 新增变体时编译器会在这里报错,
    // 强制重新判断它属于"值得重传"还是"重传也白搭"。用通配的话新变体会被
    // 默默归到不可重传一类,而那可能正好是错的。
    let out_of_resources = match error {
        pacs_store::StoreError::Io { source, .. } => matches!(
            source.kind(),
            ErrorKind::StorageFull | ErrorKind::QuotaExceeded | ErrorKind::PermissionDenied
        ),
        pacs_store::StoreError::PathEscape { .. } => false,
        // NotFound 是读路径的错误(resolve_for_read),落盘路径产生不了它。
        // 真出现说明代码走错了分支,当作不可重传的处理失败。
        pacs_store::StoreError::NotFound { .. } => false,
    };

    if out_of_resources {
        StoreFailure::OutOfResources(error.to_string())
    } else {
        StoreFailure::Processing(error.to_string())
    }
}
