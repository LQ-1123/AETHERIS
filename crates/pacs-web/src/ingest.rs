//! One ingestion path shared by STOW-RS and bulk archive imports.

use std::io::Cursor;

use pacs_db::{StorageRecord, ingest_instance_for_institution};
use pacs_store::{InstanceKey, StoreOutcome};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestDisposition {
    Created,
    Duplicate,
    Conflict,
    Invalid,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct IngestOutcome {
    pub disposition: IngestDisposition,
    pub study_instance_uid: Option<String>,
    pub series_instance_uid: Option<String>,
    pub sop_class_uid: Option<String>,
    pub sop_instance_uid: Option<String>,
    pub error: Option<String>,
}

impl IngestOutcome {
    pub fn success(&self) -> bool {
        matches!(
            self.disposition,
            IngestDisposition::Created | IngestDisposition::Duplicate
        )
    }

    fn error(disposition: IngestDisposition, message: impl Into<String>) -> Self {
        Self {
            disposition,
            study_instance_uid: None,
            series_instance_uid: None,
            sop_class_uid: None,
            sop_instance_uid: None,
            error: Some(message.into()),
        }
    }
}

pub async fn ingest_dicom(
    store: &pacs_store::Store,
    pool: &sqlx::PgPool,
    institution_id: i64,
    bytes: &[u8],
) -> IngestOutcome {
    let mut object = match dicom::object::from_reader(Cursor::new(bytes)) {
        Ok(object) => object,
        Err(error) => {
            return IngestOutcome::error(
                IngestDisposition::Invalid,
                format!("DICOM 解析失败: {error}"),
            );
        }
    };
    pacs_core::normalize_file_text(&mut object);
    let metadata = match pacs_core::extract_metadata(&object) {
        Ok(metadata) => metadata,
        Err(error) => return IngestOutcome::error(IngestDisposition::Invalid, error.to_string()),
    };
    let study = metadata.study.uid.to_string();
    let series = metadata.series.uid.to_string();
    let sop = metadata.instance.uid.to_string();
    let class = metadata
        .instance
        .sop_class_uid
        .as_ref()
        .map(ToString::to_string);
    let stored = match store
        .store(
            InstanceKey {
                study: &metadata.study.uid,
                series: &metadata.series.uid,
                sop: &metadata.instance.uid,
            },
            bytes,
        )
        .await
    {
        Ok(stored) => stored,
        Err(pacs_store::StoreError::ContentConflict { .. }) => {
            return IngestOutcome {
                disposition: IngestDisposition::Conflict,
                study_instance_uid: Some(study),
                series_instance_uid: Some(series),
                sop_class_uid: class,
                sop_instance_uid: Some(sop),
                error: Some("相同 SOPInstanceUID 的文件内容不同".to_owned()),
            };
        }
        Err(error) => {
            return IngestOutcome {
                disposition: IngestDisposition::Failed,
                study_instance_uid: Some(study),
                series_instance_uid: Some(series),
                sop_class_uid: class,
                sop_instance_uid: Some(sop),
                error: Some(error.to_string()),
            };
        }
    };
    let duplicate = stored.outcome == StoreOutcome::AlreadyIdentical;
    if let Err(error) = ingest_instance_for_institution(
        pool,
        &metadata,
        StorageRecord {
            relative_path: &stored.relative_path,
            size: stored.size,
            sha256: &stored.sha256,
        },
        institution_id,
    )
    .await
    {
        let disposition = if matches!(error, pacs_db::DbError::Conflict(_)) {
            IngestDisposition::Conflict
        } else {
            IngestDisposition::Failed
        };
        return IngestOutcome {
            disposition,
            study_instance_uid: Some(study),
            series_instance_uid: Some(series),
            sop_class_uid: class,
            sop_instance_uid: Some(sop),
            error: Some(error.to_string()),
        };
    }
    IngestOutcome {
        disposition: if duplicate {
            IngestDisposition::Duplicate
        } else {
            IngestDisposition::Created
        },
        study_instance_uid: Some(study),
        series_instance_uid: Some(series),
        sop_class_uid: class,
        sop_instance_uid: Some(sop),
        error: None,
    }
}
