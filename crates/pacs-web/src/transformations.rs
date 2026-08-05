//! Versioned DICOM correction, preview, history, and job APIs.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Cursor;
use std::sync::Arc;

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use pacs_auth::{AuthService, Identity, Permission, Role};
use pacs_core::{
    PixelRiskLevel, TagDiff, TagRule, TagScope, TransformContext, Uid, apply_transform,
    manual_tag_specs, validate_manual_rules,
};
use pacs_db::{
    ActivatedVersion, NewPreviewJob, TargetType, TransformMode, TransformSource, TransformTarget,
    UidAlias, VersionSource,
};
use pacs_store::{InstanceKey, StagedFile, Store};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::routes::WebState;

const PREVIEW_TTL_MINUTES: i64 = 15;
const MAX_REASON_CHARS: usize = 1024;
const MAX_RULES: usize = 128;

pub fn dicom_transformation_routes(state: WebState, auth: Arc<AuthService>) -> Router {
    Router::new()
        .route("/schema", get(schema))
        .route("/transformations/preview", post(preview))
        .route("/transformations", get(jobs).post(confirm))
        .route("/transformations/{job_id}", get(job))
        .route(
            "/instances/by-sop/{sop_uid}/revisions",
            get(instance_revisions_by_sop),
        )
        .route("/instances/{logical_id}/revisions", get(instance_revisions))
        .route("/instances/{logical_id}/rollback", post(rollback_preview))
        .with_state(state)
        .layer(axum::middleware::from_fn(move |request, next| {
            let auth = Arc::clone(&auth);
            async move { pacs_auth::require(auth, Permission::ViewImages, request, next).await }
        }))
}

async fn schema(
    Extension(identity): Extension<Identity>,
) -> Result<Json<Value>, TransformApiError> {
    require_any(
        &identity,
        &[Permission::EditDicomTags, Permission::ViewDicomRevisions],
    )?;
    let tags: Vec<Value> = manual_tag_specs()
        .iter()
        .map(|spec| {
            serde_json::json!({
                "keyword": spec.keyword,
                "tag": format!("({:04X},{:04X})", spec.tag.group(), spec.tag.element()),
                "vr": format!("{:?}", spec.vr),
                "scope": spec.scope,
                "actions": ["replace", "empty", "remove"]
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "manual_tags": tags })))
}

#[derive(Debug, Deserialize)]
struct PreviewRequest {
    mode: TransformMode,
    target: TransformTarget,
    #[serde(default)]
    rules: Vec<TagRule>,
    reason: String,
}

#[derive(Debug, Clone, Serialize)]
struct AggregatedDiff {
    tag: String,
    keyword: String,
    old_value: Option<String>,
    new_value: Option<String>,
    action: String,
    affected_instances: usize,
}

type AggregatedDiffKey = (String, String, Option<String>, Option<String>, String);

#[derive(Debug, Serialize)]
struct PreviewResponse {
    job_id: Uuid,
    confirmation_token: String,
    confirmation_expires_at: chrono::DateTime<Utc>,
    preview: Value,
}

async fn preview(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Json(request): Json<PreviewRequest>,
) -> Result<Json<PreviewResponse>, TransformApiError> {
    authorize_mode(&identity, request.mode)?;
    validate_reason(&request.reason)?;
    if request.rules.len() > MAX_RULES {
        return Err(TransformApiError::BadRequest(format!(
            "一次最多 {MAX_RULES} 条规则"
        )));
    }
    if request.mode != TransformMode::ClinicalCorrection {
        return Err(TransformApiError::BadRequest(
            "预览端点只接受 clinical_correction".to_owned(),
        ));
    }
    let scope = match request.target.target_type {
        TargetType::Patient => TagScope::Patient,
        TargetType::Study => TagScope::Study,
        TargetType::Series => TagScope::Series,
        TargetType::Instance => {
            return Err(TransformApiError::BadRequest(
                "临床手工修改必须选择病人、检查或序列层级".to_owned(),
            ));
        }
    };
    validate_manual_rules(&request.rules, scope)
        .map_err(|error| TransformApiError::BadRequest(error.to_string()))?;
    let store = state
        .store
        .clone()
        .ok_or(TransformApiError::StorageUnavailable)?;
    let sources =
        pacs_db::select_transform_sources(&state.pool, identity.institution_id, &request.target)
            .await
            .map_err(TransformApiError::db)?;
    let aliases = uid_aliases(&state, identity.institution_id, &sources).await?;
    let uid_map = build_uid_map(&sources, &aliases);
    let job_id = Uuid::new_v4();
    let context = TransformContext {
        uid_map: uid_map.clone(),
        derivation_description: format!("remote_pacs clinical correction job {job_id}"),
    };
    let preview_data = preview_sources(&store, &sources, &request.rules, &context).await?;
    let expires = Utc::now() + Duration::minutes(PREVIEW_TTL_MINUTES);
    let token = Uuid::new_v4().to_string();
    let token_hash: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    let mut stored_preview = preview_data.clone();
    if let Value::Object(map) = &mut stored_preview {
        map.insert(
            "uid_map".to_owned(),
            serde_json::to_value(&uid_map).map_err(TransformApiError::json)?,
        );
    }
    let rules = serde_json::to_value(&request.rules).map_err(TransformApiError::json)?;
    let pixel_risk = preview_data
        .get("pixel_risk")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    pacs_db::create_preview_job(
        &state.pool,
        NewPreviewJob {
            id: job_id,
            institution_id: identity.institution_id,
            user_id: identity.user_id,
            username: &identity.username,
            mode: request.mode,
            target: &request.target,
            rules: &rules,
            reason: request.reason.trim(),
            confirmation_hash: &token_hash,
            confirmation_expires_at: expires,
            preview: &stored_preview,
            pixel_risk,
        },
        &sources,
    )
    .await
    .map_err(TransformApiError::db)?;

    Ok(Json(PreviewResponse {
        job_id,
        confirmation_token: token,
        confirmation_expires_at: expires,
        preview: preview_data,
    }))
}

#[derive(Debug, Deserialize)]
struct RollbackRequest {
    version_id: i64,
    reason: String,
}

async fn rollback_preview(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(logical_id): Path<Uuid>,
    Json(request): Json<RollbackRequest>,
) -> Result<Json<PreviewResponse>, TransformApiError> {
    require(&identity, Permission::EditDicomTags)?;
    validate_reason(&request.reason)?;
    let store = state
        .store
        .clone()
        .ok_or(TransformApiError::StorageUnavailable)?;
    let target = TransformTarget {
        target_type: TargetType::Instance,
        key: logical_id.to_string(),
    };
    let sources = pacs_db::select_transform_sources(&state.pool, identity.institution_id, &target)
        .await
        .map_err(TransformApiError::db)?;
    let historical = pacs_db::get_version_source(
        &state.pool,
        identity.institution_id,
        logical_id,
        request.version_id,
    )
    .await
    .map_err(TransformApiError::db)?;
    if historical.is_current {
        return Err(TransformApiError::Conflict(
            "所选修订已经是当前版本".to_owned(),
        ));
    }

    let aliases = uid_aliases(&state, identity.institution_id, &sources).await?;
    let uid_map = build_uid_map(&sources, &aliases);
    let job_id = Uuid::new_v4();
    let context = TransformContext {
        uid_map: uid_map.clone(),
        derivation_description: format!("remote_pacs rollback job {job_id}"),
    };
    let mut preview_sources_data = sources.clone();
    replace_with_historical_source(&mut preview_sources_data, &historical)?;
    let mut preview_data = preview_sources(&store, &preview_sources_data, &[], &context).await?;
    if let Value::Object(map) = &mut preview_data {
        map.insert(
            "rollback".to_owned(),
            serde_json::json!({
                "logical_instance_id": logical_id,
                "from_current_version": sources
                    .iter()
                    .find(|source| source.logical_instance_id == logical_id)
                    .map(|source| source.version_number),
                "to_historical_version": historical.version_number,
                "source_version_id": historical.id
            }),
        );
    }
    let expires = Utc::now() + Duration::minutes(PREVIEW_TTL_MINUTES);
    let token = Uuid::new_v4().to_string();
    let token_hash: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    let mut stored_preview = preview_data.clone();
    if let Value::Object(map) = &mut stored_preview {
        map.insert(
            "uid_map".to_owned(),
            serde_json::to_value(&uid_map).map_err(TransformApiError::json)?,
        );
        map.insert(
            "rollback_source_version_id".to_owned(),
            Value::from(historical.id),
        );
    }
    let rules = Value::Array(Vec::new());
    let pixel_risk = preview_data
        .get("pixel_risk")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    pacs_db::create_preview_job(
        &state.pool,
        NewPreviewJob {
            id: job_id,
            institution_id: identity.institution_id,
            user_id: identity.user_id,
            username: &identity.username,
            mode: TransformMode::Rollback,
            target: &target,
            rules: &rules,
            reason: request.reason.trim(),
            confirmation_hash: &token_hash,
            confirmation_expires_at: expires,
            preview: &stored_preview,
            pixel_risk,
        },
        &sources,
    )
    .await
    .map_err(TransformApiError::db)?;

    Ok(Json(PreviewResponse {
        job_id,
        confirmation_token: token,
        confirmation_expires_at: expires,
        preview: preview_data,
    }))
}

async fn preview_sources(
    store: &Store,
    sources: &[TransformSource],
    rules: &[TagRule],
    context: &TransformContext,
) -> Result<Value, TransformApiError> {
    let mut changes: BTreeMap<AggregatedDiffKey, usize> = BTreeMap::new();
    let mut highest_risk = PixelRiskLevel::Safe;
    let mut risk_reasons = HashSet::new();
    let mut target_count = 0usize;
    for source in sources {
        target_count += usize::from(source.apply_rules);
        let bytes = read_verified(store, source).await?;
        let rules = rules.to_vec();
        let context = context.clone();
        let apply_rules = source.apply_rules;
        let outcome = tokio::task::spawn_blocking(move || {
            let mut object = dicom::object::from_reader(Cursor::new(bytes))
                .map_err(|error| WorkerError::Dicom(error.to_string()))?;
            apply_transform(&mut object, &rules, &context, apply_rules)
                .map_err(|error| WorkerError::Transform(error.to_string()))
        })
        .await
        .map_err(|error| TransformApiError::Internal(format!("预览任务失败: {error}")))?
        .map_err(TransformApiError::worker)?;
        highest_risk = highest_risk.max(outcome.pixel_risk.level);
        risk_reasons.extend(outcome.pixel_risk.reasons);
        for diff in outcome
            .diffs
            .into_iter()
            .filter(|diff| diff.action != "uid_remap")
        {
            *changes.entry(diff_key(diff)).or_default() += 1;
        }
    }
    let changes: Vec<AggregatedDiff> = changes
        .into_iter()
        .map(
            |((tag, keyword, old_value, new_value, action), affected_instances)| AggregatedDiff {
                tag,
                keyword,
                old_value,
                new_value,
                action,
                affected_instances,
            },
        )
        .collect();
    let study_count = sources
        .iter()
        .map(|source| source.study_pk)
        .collect::<HashSet<_>>()
        .len();
    let series_count = sources
        .iter()
        .map(|source| source.series_pk)
        .collect::<HashSet<_>>()
        .len();
    Ok(serde_json::json!({
        "affected_instances": sources.len(),
        "rule_target_instances": target_count,
        "affected_studies": study_count,
        "affected_series": series_count,
        "uid_remaps": {
            "studies": study_count,
            "series": series_count,
            "instances": sources.len()
        },
        "changes": changes,
        "pixel_risk": risk_name(highest_risk),
        "pixel_risk_reasons": risk_reasons.into_iter().collect::<Vec<_>>()
    }))
}

fn diff_key(diff: TagDiff) -> (String, String, Option<String>, Option<String>, String) {
    (
        diff.tag,
        diff.keyword,
        diff.old_value,
        diff.new_value,
        diff.action,
    )
}

#[derive(Debug, Deserialize)]
struct ConfirmRequest {
    job_id: Uuid,
    confirmation_token: String,
}

async fn confirm(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Json(request): Json<ConfirmRequest>,
) -> Result<(StatusCode, Json<Value>), TransformApiError> {
    require(&identity, Permission::EditDicomTags)?;
    let token_hash: [u8; 32] = Sha256::digest(request.confirmation_token.as_bytes()).into();
    pacs_db::queue_preview_job(
        &state.pool,
        identity.institution_id,
        identity.user_id,
        request.job_id,
        &token_hash,
    )
    .await
    .map_err(TransformApiError::db)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "job_id": request.job_id, "status": "queued" })),
    ))
}

/// Start the persistent transformation dispatcher.
///
/// The dispatcher is intentionally process-scoped rather than request-scoped: queued jobs survive
/// client disconnects, and jobs left running by a process crash are returned to the queue before
/// normal polling begins.
pub fn start_transform_worker(state: WebState) -> tokio::task::JoinHandle<()> {
    let process_started_at = Utc::now();
    tokio::spawn(async move {
        match pacs_db::recover_interrupted_jobs(&state.pool, process_started_at).await {
            Ok(0) => {}
            Ok(count) => tracing::warn!(count, "已恢复服务重启前中断的 DICOM 转换任务"),
            Err(error) => tracing::error!(%error, "恢复中断的 DICOM 转换任务失败"),
        }
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            interval.tick().await;
            let jobs = match pacs_db::list_runnable_jobs(&state.pool, 8).await {
                Ok(jobs) => jobs,
                Err(error) => {
                    tracing::error!(%error, "扫描待执行 DICOM 转换任务失败");
                    continue;
                }
            };
            for job in jobs {
                let worker_state = state.clone();
                tokio::spawn(async move { process_runnable_job(worker_state, job).await });
            }
        }
    })
}

async fn process_runnable_job(state: WebState, runnable: pacs_db::RunnableJob) {
    let identity = Identity {
        user_id: runnable.user_id,
        institution_id: runnable.institution_id,
        username: runnable.username,
        role: Role::Admin,
    };
    let job_id = runnable.id;
    if let Err(error) = run_job(state.clone(), identity, job_id).await {
        if matches!(&error, WorkerError::NotRunnable) {
            return;
        }
        tracing::error!(%job_id, %error, "DICOM 转换任务失败");
        if let Err(mark_error) =
            pacs_db::mark_job_failed(&state.pool, job_id, error.public_message()).await
        {
            tracing::error!(%job_id, %mark_error, "无法记录转换任务失败状态");
        }
    }
}

async fn run_job(state: WebState, identity: Identity, job_id: Uuid) -> Result<(), WorkerError> {
    let job = pacs_db::claim_job(&state.pool, identity.institution_id, job_id)
        .await
        .map_err(|error| match error {
            pacs_db::DbError::Conflict(_) => WorkerError::NotRunnable,
            other => WorkerError::Db(other),
        })?;
    let store = state.store.clone().ok_or(WorkerError::StorageUnavailable)?;
    let rules: Vec<TagRule> = serde_json::from_value(job.rules.clone())
        .map_err(|error| WorkerError::Data(error.to_string()))?;
    let mut sources = pacs_db::job_sources(&state.pool, job_id)
        .await
        .map_err(WorkerError::Db)?;
    let rollback_source = if job.mode == TransformMode::Rollback {
        let logical_id = Uuid::parse_str(&job.target.key)
            .map_err(|error| WorkerError::Data(format!("回滚目标无效: {error}")))?;
        let version_id = job
            .preview
            .get("rollback_source_version_id")
            .and_then(Value::as_i64)
            .ok_or_else(|| WorkerError::Data("回滚任务缺少历史修订".to_owned()))?;
        let historical = pacs_db::get_version_source(
            &state.pool,
            identity.institution_id,
            logical_id,
            version_id,
        )
        .await
        .map_err(WorkerError::Db)?;
        replace_with_historical_source(&mut sources, &historical)
            .map_err(|error| WorkerError::Data(error.to_string()))?;
        Some(historical)
    } else {
        None
    };
    let uid_map: HashMap<String, String> = serde_json::from_value(
        job.preview
            .get("uid_map")
            .cloned()
            .ok_or_else(|| WorkerError::Data("任务缺少 UID 映射".to_owned()))?,
    )
    .map_err(|error| WorkerError::Data(error.to_string()))?;
    let context = TransformContext {
        uid_map: uid_map.clone(),
        derivation_description: format!("remote_pacs {} job {job_id}", job.mode.as_str()),
    };

    let mut staged_outputs = Vec::with_capacity(sources.len());
    for (index, source) in sources.into_iter().enumerate() {
        let bytes = read_verified(&store, &source)
            .await
            .map_err(|error| WorkerError::Data(error.to_string()))?;
        let rules = rules.clone();
        let context = context.clone();
        let apply_rules = source.apply_rules;
        let transformed = tokio::task::spawn_blocking(move || {
            transform_to_bytes(bytes, &rules, &context, apply_rules)
        })
        .await
        .map_err(|error| WorkerError::Task(error.to_string()))??;
        let staged = store
            .stage_derived(
                job_id,
                InstanceKey {
                    study: &transformed.metadata.study.uid,
                    series: &transformed.metadata.series.uid,
                    sop: &transformed.metadata.instance.uid,
                },
                &transformed.encoded,
            )
            .await
            .map_err(WorkerError::Store)?;
        staged_outputs.push(StagedOutput {
            derivation_source_version_id: rollback_source
                .as_ref()
                .filter(|historical| historical.logical_instance_id == source.logical_instance_id)
                .map_or(source.current_version_id, |historical| historical.id),
            source,
            metadata: transformed.metadata,
            staged,
        });
        pacs_db::update_job_progress(&state.pool, job_id, index + 1)
            .await
            .map_err(WorkerError::Db)?;
    }

    let mut activated = Vec::with_capacity(staged_outputs.len());
    let mut remaining = staged_outputs.into_iter();
    while let Some(output) = remaining.next() {
        let stored = match store.activate_staged(output.staged).await {
            Ok(stored) => stored,
            Err(error) => {
                for pending in remaining {
                    let _ = store.discard_staged(pending.staged).await;
                }
                return Err(WorkerError::Store(error));
            }
        };
        activated.push(ActivatedVersion {
            source: output.source,
            derivation_source_version_id: output.derivation_source_version_id,
            metadata: output.metadata,
            storage_path: stored.relative_path,
            file_size: stored.size,
            file_sha256: stored.sha256,
            uid_map: serde_json::to_value(&uid_map)
                .map_err(|error| WorkerError::Data(error.to_string()))?,
        });
    }
    let activation = pacs_db::activate_clinical_job(
        &state.pool,
        job_id,
        identity.institution_id,
        identity.user_id,
        &identity.username,
        job.mode,
        &job.reason,
        &activated,
    )
    .await;
    if let Err(error) = activation {
        for output in &activated {
            if let Err(cleanup_error) = store.remove_derived(&output.storage_path).await {
                tracing::error!(
                    job_id = %job_id,
                    path = %output.storage_path,
                    %cleanup_error,
                    "数据库激活失败后无法清理派生文件"
                );
            }
        }
        return Err(WorkerError::Db(error));
    }
    Ok(())
}

struct TransformedBytes {
    encoded: Vec<u8>,
    metadata: pacs_core::InstanceMetadata,
}

struct StagedOutput {
    source: TransformSource,
    derivation_source_version_id: i64,
    metadata: pacs_core::InstanceMetadata,
    staged: StagedFile,
}

fn transform_to_bytes(
    bytes: Vec<u8>,
    rules: &[TagRule],
    context: &TransformContext,
    apply_rules: bool,
) -> Result<TransformedBytes, WorkerError> {
    let mut object = dicom::object::from_reader(Cursor::new(bytes))
        .map_err(|error| WorkerError::Dicom(error.to_string()))?;
    let outcome = apply_transform(&mut object, rules, context, apply_rules)
        .map_err(|error| WorkerError::Transform(error.to_string()))?;
    let expected_pixel = outcome.pixel_sha256;
    let mut encoded = Vec::new();
    object
        .write_all(&mut encoded)
        .map_err(|error| WorkerError::Dicom(error.to_string()))?;
    let reparsed = dicom::object::from_reader(Cursor::new(&encoded))
        .map_err(|error| WorkerError::Dicom(error.to_string()))?;
    if pacs_core::pixel_data_sha256(&reparsed) != expected_pixel {
        return Err(WorkerError::PixelChanged);
    }
    let reparsed_metadata = pacs_core::extract_metadata(&reparsed)
        .map_err(|error| WorkerError::Transform(error.to_string()))?;
    if reparsed_metadata.study.uid != outcome.metadata.study.uid
        || reparsed_metadata.series.uid != outcome.metadata.series.uid
        || reparsed_metadata.instance.uid != outcome.metadata.instance.uid
    {
        return Err(WorkerError::Data(
            "派生文件往返解析后 UID 不一致".to_owned(),
        ));
    }
    Ok(TransformedBytes {
        encoded,
        metadata: reparsed_metadata,
    })
}

async fn read_verified(
    store: &Store,
    source: &TransformSource,
) -> Result<Vec<u8>, TransformApiError> {
    let bytes = store
        .read(&source.storage_path)
        .await
        .map_err(|error| TransformApiError::Internal(error.to_string()))?;
    let digest = Sha256::digest(&bytes);
    if digest.as_slice() != source.file_sha256.as_slice() {
        tracing::error!(
            logical_instance_id = %source.logical_instance_id,
            "转换前文件 SHA-256 与数据库不一致"
        );
        return Err(TransformApiError::Conflict(
            "存储完整性检查失败，任务已停止".to_owned(),
        ));
    }
    Ok(bytes)
}

async fn uid_aliases(
    state: &WebState,
    institution_id: i64,
    sources: &[TransformSource],
) -> Result<Vec<UidAlias>, TransformApiError> {
    let instance_ids: Vec<i64> = sources.iter().map(|source| source.instance_pk).collect();
    pacs_db::list_uid_aliases(&state.pool, institution_id, &instance_ids)
        .await
        .map_err(TransformApiError::db)
}

fn build_uid_map(sources: &[TransformSource], aliases: &[UidAlias]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut study_outputs = HashMap::new();
    let mut series_outputs = HashMap::new();
    let mut instance_outputs = HashMap::new();
    for source in sources {
        study_outputs
            .entry(source.study_pk)
            .or_insert_with(|| Uid::generate().into_string());
        series_outputs
            .entry(source.series_pk)
            .or_insert_with(|| Uid::generate().into_string());
        instance_outputs
            .entry(source.logical_instance_id)
            .or_insert_with(|| Uid::generate().into_string());
    }
    for alias in aliases {
        if let Some(uid) = study_outputs.get(&alias.study_pk) {
            map.insert(alias.study_instance_uid.clone(), uid.clone());
        }
        if let Some(uid) = series_outputs.get(&alias.series_pk) {
            map.insert(alias.series_instance_uid.clone(), uid.clone());
        }
        if let Some(uid) = instance_outputs.get(&alias.logical_instance_id) {
            map.insert(alias.sop_instance_uid.clone(), uid.clone());
        }
    }
    for source in sources {
        map.insert(
            source.study_instance_uid.clone(),
            study_outputs[&source.study_pk].clone(),
        );
        map.insert(
            source.series_instance_uid.clone(),
            series_outputs[&source.series_pk].clone(),
        );
        map.insert(
            source.sop_instance_uid.clone(),
            instance_outputs[&source.logical_instance_id].clone(),
        );
    }
    map
}

fn replace_with_historical_source(
    sources: &mut [TransformSource],
    historical: &VersionSource,
) -> Result<(), TransformApiError> {
    let source = sources
        .iter_mut()
        .find(|source| source.logical_instance_id == historical.logical_instance_id)
        .ok_or_else(|| TransformApiError::Conflict("历史修订不属于当前检查图".to_owned()))?;
    if source.instance_pk != historical.instance_pk {
        return Err(TransformApiError::Conflict(
            "历史修订的逻辑实例不一致".to_owned(),
        ));
    }
    source.storage_path.clone_from(&historical.storage_path);
    source.file_sha256.clone_from(&historical.file_sha256);
    source
        .study_instance_uid
        .clone_from(&historical.study_instance_uid);
    source
        .series_instance_uid
        .clone_from(&historical.series_instance_uid);
    source
        .sop_instance_uid
        .clone_from(&historical.sop_instance_uid);
    source
        .transfer_syntax_uid
        .clone_from(&historical.transfer_syntax_uid);
    Ok(())
}

async fn jobs(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
) -> Result<Json<Vec<pacs_db::JobRecord>>, TransformApiError> {
    require(&identity, Permission::EditDicomTags)?;
    Ok(Json(
        pacs_db::list_jobs(&state.pool, identity.institution_id, 100)
            .await
            .map_err(TransformApiError::db)?,
    ))
}

async fn job(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(job_id): Path<Uuid>,
) -> Result<Json<pacs_db::JobRecord>, TransformApiError> {
    require(&identity, Permission::EditDicomTags)?;
    Ok(Json(
        pacs_db::get_job(&state.pool, identity.institution_id, job_id)
            .await
            .map_err(TransformApiError::db)?,
    ))
}

async fn instance_revisions(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(logical_id): Path<Uuid>,
) -> Result<Json<Vec<pacs_db::RevisionRecord>>, TransformApiError> {
    require(&identity, Permission::ViewDicomRevisions)?;
    Ok(Json(
        pacs_db::list_revisions(&state.pool, identity.institution_id, logical_id)
            .await
            .map_err(TransformApiError::db)?,
    ))
}

async fn instance_revisions_by_sop(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(sop_uid): Path<String>,
) -> Result<Json<Vec<pacs_db::RevisionRecord>>, TransformApiError> {
    require(&identity, Permission::ViewDicomRevisions)?;
    Uid::parse(&sop_uid).map_err(|error| TransformApiError::BadRequest(error.to_string()))?;
    let logical_id = pacs_db::logical_instance_id_for_current_sop(
        &state.pool,
        identity.institution_id,
        &sop_uid,
    )
    .await
    .map_err(TransformApiError::db)?;
    Ok(Json(
        pacs_db::list_revisions(&state.pool, identity.institution_id, logical_id)
            .await
            .map_err(TransformApiError::db)?,
    ))
}

fn authorize_mode(identity: &Identity, mode: TransformMode) -> Result<(), TransformApiError> {
    match mode {
        TransformMode::ClinicalCorrection | TransformMode::Rollback => {
            require(identity, Permission::EditDicomTags)
        }
    }
}

fn require(identity: &Identity, permission: Permission) -> Result<(), TransformApiError> {
    if identity.role.can(permission) {
        Ok(())
    } else {
        Err(TransformApiError::Forbidden)
    }
}

fn require_any(identity: &Identity, permissions: &[Permission]) -> Result<(), TransformApiError> {
    if permissions
        .iter()
        .any(|permission| identity.role.can(*permission))
    {
        Ok(())
    } else {
        Err(TransformApiError::Forbidden)
    }
}

fn validate_reason(reason: &str) -> Result<(), TransformApiError> {
    let count = reason.trim().chars().count();
    if !(3..=MAX_REASON_CHARS).contains(&count) {
        return Err(TransformApiError::BadRequest(format!(
            "变更原因长度必须在 3..={MAX_REASON_CHARS} 个字符之间"
        )));
    }
    Ok(())
}

fn risk_name(risk: PixelRiskLevel) -> &'static str {
    match risk {
        PixelRiskLevel::Safe => "safe",
        PixelRiskLevel::ReviewRequired => "review_required",
        PixelRiskLevel::Blocking => "blocking",
    }
}

#[derive(Debug, thiserror::Error)]
enum WorkerError {
    #[error("任务已被其他工作器领取或暂不可运行")]
    NotRunnable,
    #[error("数据库错误: {0}")]
    Db(#[from] pacs_db::DbError),
    #[error("存储错误: {0}")]
    Store(#[from] pacs_store::StoreError),
    #[error("DICOM 解析或编码失败: {0}")]
    Dicom(String),
    #[error("DICOM 转换失败: {0}")]
    Transform(String),
    #[error("任务数据无效: {0}")]
    Data(String),
    #[error("后台任务执行失败: {0}")]
    Task(String),
    #[error("转换改变了 PixelData")]
    PixelChanged,
    #[error("存储未配置")]
    StorageUnavailable,
}

impl WorkerError {
    fn public_message(&self) -> &'static str {
        match self {
            Self::NotRunnable => "任务暂不可运行",
            Self::Db(pacs_db::DbError::Conflict(_)) => "基础修订已变化，请重新预览",
            Self::PixelChanged => "像素完整性校验失败",
            Self::StorageUnavailable => "存储未配置",
            _ => "DICOM 转换失败，请查看服务端日志",
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum TransformApiError {
    #[error("{0}")]
    BadRequest(String),
    #[error("权限不足")]
    Forbidden,
    #[error("资源不存在")]
    NotFound,
    #[error("{0}")]
    Conflict(String),
    #[error("影像存储未配置")]
    StorageUnavailable,
    #[error("内部错误")]
    Internal(String),
}

impl TransformApiError {
    fn db(error: pacs_db::DbError) -> Self {
        match error {
            pacs_db::DbError::NotFound => Self::NotFound,
            pacs_db::DbError::Conflict(message) => Self::Conflict(message),
            pacs_db::DbError::Invalid(message) => Self::BadRequest(message),
            other => {
                tracing::error!(%other, "DICOM 转换数据库操作失败");
                Self::Internal(other.to_string())
            }
        }
    }

    fn json(error: serde_json::Error) -> Self {
        tracing::error!(%error, "转换请求 JSON 序列化失败");
        Self::Internal(error.to_string())
    }

    fn worker(error: WorkerError) -> Self {
        tracing::error!(%error, "DICOM 转换预览失败");
        Self::BadRequest(error.public_message().to_owned())
    }
}

impl IntoResponse for TransformApiError {
    fn into_response(self) -> Response {
        let status = match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::StorageUnavailable | Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let message = match self {
            Self::Internal(_) => "内部错误".to_owned(),
            other => other.to_string(),
        };
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(role: Role) -> Identity {
        Identity {
            user_id: 1,
            institution_id: 1,
            username: "test".to_owned(),
            role,
        }
    }

    #[test]
    fn clinical_preview_permissions_are_enforced() {
        assert!(authorize_mode(&identity(Role::Admin), TransformMode::ClinicalCorrection).is_ok());
        assert!(
            authorize_mode(
                &identity(Role::Technician),
                TransformMode::ClinicalCorrection,
            )
            .is_ok()
        );
        assert!(
            authorize_mode(
                &identity(Role::Radiologist),
                TransformMode::ClinicalCorrection,
            )
            .is_err()
        );
        assert!(
            authorize_mode(&identity(Role::Viewer), TransformMode::ClinicalCorrection).is_err()
        );
    }

    #[test]
    fn change_reason_is_required_and_bounded() {
        assert!(validate_reason("修正姓名").is_ok());
        assert!(validate_reason("  ").is_err());
        assert!(validate_reason(&"x".repeat(MAX_REASON_CHARS + 1)).is_err());
    }
}
