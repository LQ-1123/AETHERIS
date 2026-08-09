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

use pacs_db::{
    IngestPreflight, StorageRecord, ingest_instance, preflight_instance_for_institution,
};
use pacs_dimse::{
    FindFailure, FindHandler, FindRequest, FindResponse, IncomingAssociation, IncomingInstance,
    StoreFailure, StoreHandler,
};
use pacs_store::{InstanceKey, Store};
use sha2::{Digest, Sha256};
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
    async fn association_opened(&self, association: &IncomingAssociation) {
        if let Err(error) = pacs_db::observe_dicom_association_opened(
            &self.pool,
            1,
            &association.calling_ae_title,
            &association.remote_addr.ip().to_string(),
        )
        .await
        {
            tracing::error!(%error, calling_ae_title=%association.calling_ae_title, remote_addr=%association.remote_addr, "记录 DIMSE 入站设备失败");
        }
    }

    async fn association_closed(&self, association: &IncomingAssociation) {
        if let Err(error) = pacs_db::observe_dicom_association_closed(
            &self.pool,
            1,
            &association.calling_ae_title,
            &association.remote_addr.ip().to_string(),
        )
        .await
        {
            tracing::error!(%error, calling_ae_title=%association.calling_ae_title, remote_addr=%association.remote_addr, "更新 DIMSE 入站设备断开状态失败");
        }
    }

    async fn store(&self, instance: IncomingInstance<'_>) -> Result<(), StoreFailure> {
        // 元数据在关联线程里就已经解析好了(`pacs-dimse` 收完数据集时提取),
        // 这里不再重新读盘解析一遍 —— 那既慢又可能与已落盘的字节不一致。
        let metadata = instance.metadata;

        let sha256: [u8; 32] = Sha256::digest(instance.file_bytes).into();
        match preflight_instance_for_institution(&self.pool, metadata, &sha256, 1)
            .await
            .map_err(|error| StoreFailure::Processing(error.to_string()))?
        {
            IngestPreflight::Duplicate => {
                if let Err(error) = pacs_db::record_dimse_origin(
                    &self.pool,
                    1,
                    metadata.instance.uid.as_str(),
                    instance.calling_ae_title,
                    &instance.remote_addr.ip().to_string(),
                )
                .await
                {
                    tracing::error!(%error, sop_instance_uid=%metadata.instance.uid, "重传影像来源记录失败");
                }
                tracing::debug!(
                    calling_ae_title = instance.calling_ae_title,
                    sop_instance_uid = %metadata.instance.uid,
                    "重复影像已归档，不在热层创建额外副本"
                );
                return Ok(());
            }
            IngestPreflight::Accept => {}
        }

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

        let ingested = ingest_instance(
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

        if let Err(error) = pacs_db::record_dimse_origin(
            &self.pool,
            1,
            metadata.instance.uid.as_str(),
            instance.calling_ae_title,
            &instance.remote_addr.ip().to_string(),
        )
        .await
        {
            // Origin classification is access-control metadata. The immutable image is already
            // durable, so do not lie to the modality with a failed C-STORE response; keep it
            // administrator-only until the source can be resolved.
            tracing::error!(%error, sop_instance_uid=%metadata.instance.uid, "影像已入库，但来源设备记录失败");
        }

        if ingested.instance_created
            && let Err(error) = pacs_web::router::enqueue_for_instance(
                &self.pool,
                1,
                metadata.instance.uid.as_str(),
                Some(instance.calling_ae_title),
            )
            .await
        {
            // Routing is post-commit. A remote destination failure must never turn a durable
            // local C-STORE into a failed C-STORE response.
            tracing::error!(%error, sop_instance_uid=%metadata.instance.uid, "影像已入库，但创建路由投递失败");
        }

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
        // UID/content conflicts indicate a sender bug. Retrying the same object cannot help and
        // must never overwrite the archived original.
        pacs_store::StoreError::ContentConflict { .. }
        | pacs_store::StoreError::DestinationExists { .. } => false,
    };

    if out_of_resources {
        StoreFailure::OutOfResources(error.to_string())
    } else {
        StoreFailure::Processing(error.to_string())
    }
}
