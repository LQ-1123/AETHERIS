//! Postgres 访问层。
//!
//! 服务端独占数据库连接,客户端绝不直连(见实施计划"信任边界")。软件要分发到
//! 不同机器、不同账号,客户端一旦内嵌连接串就等于把库凭据发给每个用户 ——
//! 无法做权限控制、无法吊销、无法轮换。
//!
//! # 关于 SQL 的编译期校验
//!
//! 这里用运行期校验的 `sqlx::query`,而不是编译期校验的 `query!` 宏。
//! 宏要求编译时能连上数据库或提交 `.sqlx` 离线缓存,而 C-FIND 的查询是按
//! 匹配键动态拼的(阶段 4),本来就只能运行期构造 —— 最容易出错的那部分覆盖不到,
//! 却要为此在每次改表后多一道 `cargo sqlx prepare`。SQL 的正确性交给
//! 跑在真实数据库上的集成测试来保证。

mod annotations;
mod clinical;
mod exam_requests;
pub mod find;
pub mod ingest;
mod jobs;
mod lifecycle;
pub mod retrieve;
mod router;
mod segmentations;
mod transfers;
mod transformations;
mod window_presets;
pub mod worklist;

use std::time::Duration;

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use thiserror::Error;

pub use find::{DEFAULT_LIMIT, FindResults, find, find_for_institution};
pub use ingest::{
    IngestPreflight, Ingested, StorageRecord, ingest_instance, ingest_instance_for_institution,
    preflight_instance_for_institution,
};
pub use jobs::{
    BackgroundJob, BackgroundJobItem, JobItemStatus, JobKind, JobStatus, NewJob,
    add_job_item as add_background_job_item, claim_job as claim_background_job,
    complete_job as complete_background_job, create_job as create_background_job,
    fail_job as fail_background_job, finish_job_item as finish_background_job_item,
    get_job as get_background_job, heartbeat_job as heartbeat_background_job,
    list_job_items as list_background_job_items, list_jobs as list_background_jobs,
    recover_expired_jobs as recover_background_jobs, release_job as release_background_job,
    request_job_cancellation, start_job_item as start_background_job_item,
    update_job_progress as update_background_job_progress,
};
pub use lifecycle::{
    LegalHold, LifecycleEvent, LifecycleFile, LifecyclePathUpdate, LifecyclePolicy,
    LifecyclePolicyInput, LifecycleStudy, LifecycleSummary, PurgeFile, PurgeRequest, StorageTier,
    approve_purge_request, begin_purge, commit_purge_metadata, create_legal_hold,
    create_lifecycle_policy, create_purge_request, delete_lifecycle_policy, finalize_purge,
    get_lifecycle_policy, lifecycle_files_for_study, lifecycle_summary,
    list_due_lifecycle_policies, list_legal_holds, list_lifecycle_events, list_lifecycle_policies,
    list_lifecycle_studies, list_purge_requests, mark_lifecycle_policy_run,
    mark_purge_file_deleted, preview_lifecycle_policy, record_lifecycle_preview,
    record_purge_error, record_study_access, reject_purge_request, release_legal_hold,
    switch_study_storage_tier, update_lifecycle_policy,
};
pub use retrieve::{
    StoredInstance, find_instance, find_instance_for_institution, list_series_instances,
};
pub use router::{
    ObservedDicomPeer, RoutableSeries, RouteDelivery, RouteDestination, RouteDestinationInput,
    RouteProtocol, RouteRule, RouteRuleInput, RouteSource, approve_route_destination,
    create_route_destination, create_route_rule, delete_route_destination, delete_route_rule,
    enqueue_route_delivery, finish_delivery, get_delivery_source, get_route_destination,
    list_observed_dicom_peers, list_routable_series, list_route_deliveries,
    list_route_destinations, list_route_rules, mark_delivery_running, matching_route_rules,
    observe_dicom_association_closed, observe_dicom_association_opened, record_destination_health,
    replay_route_delivery, reset_observed_dicom_associations, retry_delivery, route_source_by_sop,
    route_sources_for_scope, update_route_destination, update_route_rule,
};
pub use segmentations::{
    NewSegmentationProject, SegmentationMask, SegmentationMaskUpdate, SegmentationProject,
    SegmentationSegment, UpdateSegmentationSegmentTags, UpsertSegmentationMask,
    create_segmentation_project, delete_segmentation_project, find_segmentation_segments_by_tag,
    list_segmentation_masks, list_segmentation_projects, list_segmentation_segment_masks,
    list_segmentation_segments, update_segmentation_segment_tags, upsert_segmentation_mask,
    upsert_segmentation_masks_batch,
};
pub use transfers::{
    ExportArtifact, ExportSource, ImportUpload, UploadStatus, advance_upload, create_import_upload,
    find_export_artifact, list_export_sources, list_import_uploads, mark_upload_failed,
    mark_upload_ready, purge_expired_export_artifacts, save_export_artifact,
};
pub use transformations::{
    ActivatedVersion, JobRecord, NewPreviewJob, RevisionRecord, RunnableJob, TargetType,
    TransformMode, TransformSource, TransformTarget, UidAlias, VersionSource,
    activate_clinical_job, claim_job, create_preview_job, get_job, get_version_source, job_sources,
    list_jobs, list_revisions, list_runnable_jobs, list_uid_aliases,
    logical_instance_id_for_current_sop, mark_job_failed, queue_preview_job,
    recover_interrupted_jobs, select_transform_sources, update_job_progress,
};
pub use window_presets::{
    NewUserWindowPreset, UserWindowPreset, create_user_window_preset, delete_user_window_preset,
    list_user_window_presets, rename_user_window_preset,
};
pub use worklist::{
    PatientSummary, QueueFilter, QueueSort, QueueStudyRow, SeriesSummary, StudySummary,
    list_patient_studies, list_patients, list_queue_studies, list_study_series,
};

#[derive(Debug, Error)]
pub enum DbError {
    #[error("数据库操作失败")]
    Query(#[from] sqlx::Error),
    #[error("数据库迁移失败")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    /// 查询命中的记录数超过上限。
    ///
    /// 刻意报错而不是截断:截断会让对方以为"结果就这么多",
    /// 一次静默漏掉的检查比一次明确的失败危险得多。
    #[error("结果超过 {limit} 条,请收窄查询条件")]
    TooManyResults { limit: usize },
    #[error("资源不存在")]
    NotFound,
    #[error("并发冲突: {0}")]
    Conflict(String),
    #[error("数据无效: {0}")]
    Invalid(String),
}

/// 连接数据库并建立连接池。
pub async fn connect(url: &str) -> Result<PgPool, DbError> {
    let pool = PgPoolOptions::new()
        .max_connections(16)
        .acquire_timeout(Duration::from_secs(5))
        .connect(url)
        .await?;
    Ok(pool)
}

/// 执行所有未应用的迁移。
///
/// 迁移在编译期嵌入二进制,部署时不用带 SQL 文件。
pub async fn migrate(pool: &PgPool) -> Result<(), DbError> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}
pub use annotations::{
    AnnotationRecord, AnnotationUpdate, NewAnnotation, create_annotation, list_annotations,
    update_annotation,
};
pub use clinical::{
    ApproveDevice, ClinicalWorkItem, DiagnosticReport, DicomDevice, ReportReviewEvent,
    ReportTemplate, ReportVersion, SeriesSourceEntry, approve_device, approve_report,
    assign_work_item, begin_report_amendment, can_access_series, can_access_study, claim_study,
    claim_work_item, create_report, institution_today, list_clinical_work, list_devices,
    list_report_review_events, list_report_templates, list_report_versions, list_reports,
    list_series_sources, observe_device, record_dimse_origin, register_device, release_study,
    release_work_item, replace_user_device_grants, resolve_series_source, set_device_status,
    sign_report, start_report_review, study_work_items, submit_report, update_report_draft,
    user_device_grants, work_item_for_series,
};
pub use exam_requests::{
    ExamRequest, ExamRequestInput, ExamRequestStudyCandidate, ExistingStudyExamRequestInput,
    WorkloadRow, bind_exam_request, create_exam_request, create_exam_request_for_study,
    exam_request_for_study, list_exam_request_study_candidates, list_exam_requests,
    update_exam_request, workload_report,
};
