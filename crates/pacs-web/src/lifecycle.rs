//! DICOM lifecycle policy, tier migration, legal hold and purge management.

use std::path::{Component, Path};
use std::sync::Arc;
use std::time::Duration as StdDuration;

use axum::extract::{Extension, Path as UrlPath, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use fs2::available_space;
use pacs_auth::service_accounts::{ApiScope, ServiceIdentity};
use pacs_auth::{AuthService, Identity, Permission};
use pacs_db::{BackgroundJob, JobKind, NewJob, StorageTier};
use pacs_store::StorageTier as StoreTier;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::WebState;

const DEFAULT_GRACE_HOURS: i64 = 24 * 7;
const MAX_GRACE_HOURS: i64 = 24 * 365;

pub fn routes(state: WebState, auth: Arc<AuthService>) -> Router {
    Router::new()
        .route("/lifecycle/summary", get(summary))
        .route("/lifecycle/jobs", get(list_jobs))
        .route(
            "/lifecycle/policies",
            get(list_policies).post(create_policy),
        )
        .route(
            "/lifecycle/policies/{id}",
            put(update_policy).delete(delete_policy),
        )
        .route("/lifecycle/policies/{id}/preview", post(preview_policy))
        .route("/lifecycle/policies/{id}/run", post(run_policy))
        .route("/lifecycle/studies", get(list_studies))
        .route("/lifecycle/studies/{study_uid}/move", post(move_study))
        .route(
            "/lifecycle/studies/{study_uid}/restore",
            post(restore_study),
        )
        .route("/lifecycle/studies/{study_uid}/holds", post(create_hold))
        .route("/lifecycle/holds", get(list_holds))
        .route("/lifecycle/holds/{id}", delete(release_hold))
        .route(
            "/lifecycle/purge-requests",
            get(list_purges).post(create_purge),
        )
        .route(
            "/lifecycle/purge-requests/{id}/approve",
            post(approve_purge),
        )
        .route("/lifecycle/purge-requests/{id}/reject", post(reject_purge))
        .route("/lifecycle/events", get(events))
        .with_state(state)
        .layer(axum::middleware::from_fn(move |request, next| {
            let auth = Arc::clone(&auth);
            async move { require_lifecycle(auth, request, next).await }
        }))
}

async fn require_lifecycle(auth: Arc<AuthService>, request: Request, next: Next) -> Response {
    let service = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split_once(' '))
        .is_some_and(|(scheme, token)| {
            scheme.eq_ignore_ascii_case("Bearer") && token.trim().starts_with("pacs_sk_")
        });
    if service {
        pacs_auth::service_accounts::require_api_scope(auth, ApiScope::Admin, request, next).await
    } else {
        pacs_auth::require(auth, Permission::DeleteImages, request, next).await
    }
}

fn institution(
    user: Option<&Extension<Identity>>,
    service: Option<&Extension<ServiceIdentity>>,
) -> Result<i64, LifecycleError> {
    user.map(|Extension(identity)| identity.institution_id)
        .or_else(|| service.map(|Extension(identity)| identity.institution_id))
        .ok_or(LifecycleError::Identity)
}

fn actor(user: Option<&Extension<Identity>>) -> Option<i64> {
    user.map(|Extension(identity)| identity.user_id)
}

#[derive(Debug, Deserialize)]
struct PolicyInput {
    name: String,
    #[serde(default = "default_priority")]
    priority: i32,
    #[serde(default)]
    enabled: bool,
    target_tier: StorageTier,
    #[serde(default)]
    modalities: Vec<String>,
    study_date_before: Option<NaiveDate>,
    last_accessed_before: Option<DateTime<Utc>>,
    #[serde(default = "empty_object")]
    tag_matches: Value,
    minimum_study_bytes: Option<i64>,
    minimum_storage_used_percent: Option<f64>,
}

fn default_priority() -> i32 {
    100
}
fn empty_object() -> Value {
    json!({})
}

impl PolicyInput {
    fn normalized(&self) -> pacs_db::LifecyclePolicyInput<'_> {
        pacs_db::LifecyclePolicyInput {
            name: &self.name,
            priority: self.priority,
            enabled: self.enabled,
            target_tier: self.target_tier,
            modalities: &self.modalities,
            study_date_before: self.study_date_before,
            last_accessed_before: self.last_accessed_before,
            tag_matches: &self.tag_matches,
            minimum_study_bytes: self.minimum_study_bytes,
            minimum_storage_used_percent: self.minimum_storage_used_percent,
            definition_signature: &[],
        }
    }
}

fn policy_signature(input: &PolicyInput) -> Result<Vec<u8>, LifecycleError> {
    let value = json!({
        "name": input.name.trim(), "priority": input.priority, "target_tier": input.target_tier,
        "modalities": input.modalities, "study_date_before": input.study_date_before,
        "last_accessed_before": input.last_accessed_before, "tag_matches": input.tag_matches,
        "minimum_study_bytes": input.minimum_study_bytes,
        "minimum_storage_used_percent": input.minimum_storage_used_percent,
    });
    Ok(Sha256::digest(serde_json::to_vec(&value)?).to_vec())
}

async fn summary(
    State(state): State<WebState>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
) -> Result<Json<pacs_db::LifecycleSummary>, LifecycleError> {
    Ok(Json(
        pacs_db::lifecycle_summary(&state.pool, institution(user.as_ref(), service.as_ref())?)
            .await?,
    ))
}

async fn list_jobs(
    State(state): State<WebState>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
) -> Result<Json<Vec<BackgroundJob>>, LifecycleError> {
    Ok(Json(
        pacs_db::list_background_jobs(
            &state.pool,
            institution(user.as_ref(), service.as_ref())?,
            JobKind::Lifecycle,
            100,
        )
        .await?,
    ))
}

async fn list_policies(
    State(state): State<WebState>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
) -> Result<Json<Vec<pacs_db::LifecyclePolicy>>, LifecycleError> {
    Ok(Json(
        pacs_db::list_lifecycle_policies(
            &state.pool,
            institution(user.as_ref(), service.as_ref())?,
        )
        .await?,
    ))
}

async fn create_policy(
    State(state): State<WebState>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
    Json(input): Json<PolicyInput>,
) -> Result<(StatusCode, Json<pacs_db::LifecyclePolicy>), LifecycleError> {
    let signature = policy_signature(&input)?;
    let mut db_input = input.normalized();
    db_input.definition_signature = &signature;
    let policy = pacs_db::create_lifecycle_policy(
        &state.pool,
        institution(user.as_ref(), service.as_ref())?,
        actor(user.as_ref()),
        &db_input,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(policy)))
}

async fn update_policy(
    State(state): State<WebState>,
    UrlPath(id): UrlPath<Uuid>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
    Json(input): Json<PolicyInput>,
) -> Result<Json<pacs_db::LifecyclePolicy>, LifecycleError> {
    let signature = policy_signature(&input)?;
    let mut db_input = input.normalized();
    db_input.definition_signature = &signature;
    Ok(Json(
        pacs_db::update_lifecycle_policy(
            &state.pool,
            institution(user.as_ref(), service.as_ref())?,
            id,
            &db_input,
        )
        .await?,
    ))
}

async fn delete_policy(
    State(state): State<WebState>,
    UrlPath(id): UrlPath<Uuid>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
) -> Result<StatusCode, LifecycleError> {
    if pacs_db::delete_lifecycle_policy(
        &state.pool,
        institution(user.as_ref(), service.as_ref())?,
        id,
    )
    .await?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(LifecycleError::NotFound)
    }
}

async fn storage_used_percent(state: &WebState) -> Result<f64, LifecycleError> {
    let store = state.store.as_ref().ok_or(LifecycleError::Storage)?;
    let total = fs2::total_space(store.root())?;
    let available = available_space(store.root())?;
    if total == 0 {
        return Ok(0.0);
    }
    Ok(((total.saturating_sub(available)) as f64 / total as f64) * 100.0)
}

async fn preview_policy(
    State(state): State<WebState>,
    UrlPath(id): UrlPath<Uuid>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
) -> Result<Json<Value>, LifecycleError> {
    let institution_id = institution(user.as_ref(), service.as_ref())?;
    let policy = pacs_db::get_lifecycle_policy(&state.pool, institution_id, id).await?;
    let used_percent = storage_used_percent(&state).await?;
    let threshold_met = policy
        .minimum_storage_used_percent
        .is_none_or(|value| used_percent >= value);
    let matches = pacs_db::preview_lifecycle_policy(
        &state.pool,
        institution_id,
        &policy,
        threshold_met,
        10_000,
    )
    .await?;
    let matched_bytes: i64 = matches.iter().map(|study| study.storage_bytes).sum();
    let summary = json!({"matched_studies": matches.len(), "matched_bytes": matched_bytes,
        "storage_used_percent": used_percent, "storage_threshold_met": threshold_met,
        "sample": matches});
    let signature = policy_signature_from_policy(&policy)?;
    pacs_db::record_lifecycle_preview(&state.pool, institution_id, id, &signature, &summary)
        .await?;
    Ok(Json(summary))
}

fn policy_signature_from_policy(
    policy: &pacs_db::LifecyclePolicy,
) -> Result<Vec<u8>, LifecycleError> {
    let value = json!({"name": policy.name.trim(), "priority": policy.priority, "target_tier": policy.target_tier,
        "modalities": policy.modalities, "study_date_before": policy.study_date_before,
        "last_accessed_before": policy.last_accessed_before, "tag_matches": policy.tag_matches,
        "minimum_study_bytes": policy.minimum_study_bytes,
        "minimum_storage_used_percent": policy.minimum_storage_used_percent});
    Ok(Sha256::digest(serde_json::to_vec(&value)?).to_vec())
}

async fn run_policy(
    State(state): State<WebState>,
    UrlPath(id): UrlPath<Uuid>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
) -> Result<(StatusCode, Json<Value>), LifecycleError> {
    let institution_id = institution(user.as_ref(), service.as_ref())?;
    let policy = pacs_db::get_lifecycle_policy(&state.pool, institution_id, id).await?;
    if !policy.enabled || !policy.preview_current {
        return Err(LifecycleError::Conflict(
            "策略必须启用且已对当前定义完成预演".to_owned(),
        ));
    }
    let used_percent = storage_used_percent(&state).await?;
    let threshold_met = policy
        .minimum_storage_used_percent
        .is_none_or(|value| used_percent >= value);
    let matches = pacs_db::preview_lifecycle_policy(
        &state.pool,
        institution_id,
        &policy,
        threshold_met,
        10_000,
    )
    .await?;
    let studies: Vec<String> = matches
        .into_iter()
        .map(|study| study.study_instance_uid)
        .collect();
    let payload = json!({"operation":"move","target_tier":policy.target_tier,"study_uids":studies,"policy_id":id});
    let job = create_lifecycle_job(
        &state,
        institution_id,
        actor(user.as_ref()),
        payload,
        &studies,
        None,
    )
    .await?;
    pacs_db::mark_lifecycle_policy_run(&state.pool, institution_id, id).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"job":job,"matched_studies":studies.len()})),
    ))
}

#[derive(Debug, Deserialize)]
struct MoveInput {
    target_tier: StorageTier,
}

async fn move_study(
    State(state): State<WebState>,
    UrlPath(study_uid): UrlPath<String>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
    Json(input): Json<MoveInput>,
) -> Result<(StatusCode, Json<Value>), LifecycleError> {
    validate_uid(&study_uid)?;
    if input.target_tier == StorageTier::Hot {
        return Err(LifecycleError::BadRequest(
            "恢复请使用 restore 接口".to_owned(),
        ));
    }
    let institution_id = institution(user.as_ref(), service.as_ref())?;
    let payload =
        json!({"operation":"move","target_tier":input.target_tier,"study_uids":[study_uid]});
    let job = create_lifecycle_job(
        &state,
        institution_id,
        actor(user.as_ref()),
        payload,
        std::slice::from_ref(&study_uid),
        None,
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(json!({"job":job}))))
}

async fn restore_study(
    State(state): State<WebState>,
    UrlPath(study_uid): UrlPath<String>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
) -> Result<(StatusCode, Json<Value>), LifecycleError> {
    validate_uid(&study_uid)?;
    let institution_id = institution(user.as_ref(), service.as_ref())?;
    let payload =
        json!({"operation":"move","target_tier":StorageTier::Hot,"study_uids":[study_uid]});
    let job = create_lifecycle_job(
        &state,
        institution_id,
        actor(user.as_ref()),
        payload,
        std::slice::from_ref(&study_uid),
        None,
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(json!({"job":job}))))
}

async fn create_lifecycle_job(
    state: &WebState,
    institution_id: i64,
    actor: Option<i64>,
    payload: Value,
    studies: &[String],
    idempotency_key: Option<&str>,
) -> Result<BackgroundJob, LifecycleError> {
    let job = pacs_db::create_background_job(
        &state.pool,
        NewJob {
            id: Uuid::new_v4(),
            institution_id,
            created_by: actor,
            kind: JobKind::Lifecycle,
            idempotency_key,
            payload: &payload,
            progress_total: studies.len() as i64,
            max_attempts: 3,
            available_at: None,
        },
    )
    .await?;
    for study_uid in studies {
        pacs_db::add_background_job_item(
            &state.pool,
            job.id,
            study_uid,
            &json!({"study_instance_uid":study_uid}),
        )
        .await?;
    }
    Ok(job)
}

async fn list_studies(
    State(state): State<WebState>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
) -> Result<Json<Vec<pacs_db::LifecycleStudy>>, LifecycleError> {
    Ok(Json(
        pacs_db::list_lifecycle_studies(
            &state.pool,
            institution(user.as_ref(), service.as_ref())?,
            500,
        )
        .await?,
    ))
}

async fn create_hold(
    State(state): State<WebState>,
    UrlPath(study_uid): UrlPath<String>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
    Json(input): Json<HoldInput>,
) -> Result<(StatusCode, Json<pacs_db::LegalHold>), LifecycleError> {
    validate_uid(&study_uid)?;
    Ok((
        StatusCode::CREATED,
        Json(
            pacs_db::create_legal_hold(
                &state.pool,
                institution(user.as_ref(), service.as_ref())?,
                &study_uid,
                &input.reason,
                input.expires_at,
                actor(user.as_ref()),
            )
            .await?,
        ),
    ))
}

#[derive(Debug, Deserialize)]
struct HoldInput {
    reason: String,
    expires_at: Option<DateTime<Utc>>,
}

async fn list_holds(
    State(state): State<WebState>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
) -> Result<Json<Vec<pacs_db::LegalHold>>, LifecycleError> {
    Ok(Json(
        pacs_db::list_legal_holds(&state.pool, institution(user.as_ref(), service.as_ref())?)
            .await?,
    ))
}

async fn release_hold(
    State(state): State<WebState>,
    UrlPath(id): UrlPath<Uuid>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
) -> Result<Json<pacs_db::LegalHold>, LifecycleError> {
    Ok(Json(
        pacs_db::release_legal_hold(
            &state.pool,
            institution(user.as_ref(), service.as_ref())?,
            id,
            actor(user.as_ref()),
        )
        .await?,
    ))
}

async fn create_purge(
    State(state): State<WebState>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
    Json(input): Json<PurgeInputWithStudy>,
) -> Result<(StatusCode, Json<pacs_db::PurgeRequest>), LifecycleError> {
    let study_uid = &input.study_instance_uid;
    validate_uid(study_uid)?;
    Ok((
        StatusCode::CREATED,
        Json(
            pacs_db::create_purge_request(
                &state.pool,
                institution(user.as_ref(), service.as_ref())?,
                study_uid,
                &input.reason,
                actor(user.as_ref()),
            )
            .await?,
        ),
    ))
}

#[derive(Debug, Deserialize)]
struct PurgeInputWithStudy {
    study_instance_uid: String,
    reason: String,
}

async fn approve_purge(
    State(state): State<WebState>,
    UrlPath(id): UrlPath<Uuid>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
    Json(input): Json<GraceInput>,
) -> Result<Json<pacs_db::PurgeRequest>, LifecycleError> {
    let hours = input.grace_hours.unwrap_or(DEFAULT_GRACE_HOURS);
    if !(0..=MAX_GRACE_HOURS).contains(&hours) {
        return Err(LifecycleError::BadRequest("宽限期超出范围".to_owned()));
    }
    Ok(Json(
        pacs_db::approve_purge_request(
            &state.pool,
            institution(user.as_ref(), service.as_ref())?,
            id,
            Utc::now() + Duration::hours(hours),
            actor(user.as_ref()),
        )
        .await?,
    ))
}

#[derive(Debug, Deserialize)]
struct GraceInput {
    grace_hours: Option<i64>,
}

async fn reject_purge(
    State(state): State<WebState>,
    UrlPath(id): UrlPath<Uuid>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
) -> Result<Json<pacs_db::PurgeRequest>, LifecycleError> {
    Ok(Json(
        pacs_db::reject_purge_request(
            &state.pool,
            institution(user.as_ref(), service.as_ref())?,
            id,
            actor(user.as_ref()),
        )
        .await?,
    ))
}

async fn list_purges(
    State(state): State<WebState>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
) -> Result<Json<Vec<pacs_db::PurgeRequest>>, LifecycleError> {
    Ok(Json(
        pacs_db::list_purge_requests(&state.pool, institution(user.as_ref(), service.as_ref())?)
            .await?,
    ))
}

async fn events(
    State(state): State<WebState>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
) -> Result<Json<Vec<pacs_db::LifecycleEvent>>, LifecycleError> {
    Ok(Json(
        pacs_db::list_lifecycle_events(
            &state.pool,
            institution(user.as_ref(), service.as_ref())?,
            200,
        )
        .await?,
    ))
}

fn validate_uid(value: &str) -> Result<(), LifecycleError> {
    pacs_core::Uid::parse(value)
        .map(|_| ())
        .map_err(|error| LifecycleError::BadRequest(error.to_string()))
}

pub fn start_lifecycle_worker(state: WebState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let worker = Uuid::new_v4();
        let mut interval = tokio::time::interval(StdDuration::from_secs(1));
        let mut schedule_interval = tokio::time::interval(StdDuration::from_secs(60));
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = schedule_interval.tick() => {
                    if let Err(error) = schedule_due_policies(&state).await {
                        tracing::error!(%error, "自动调度生命周期策略失败");
                    }
                }
            }
            if let Err(error) = pacs_db::recover_background_jobs(&state.pool).await {
                tracing::error!(%error, "恢复生命周期任务失败");
            }
            match pacs_db::claim_background_job(
                &state.pool,
                JobKind::Lifecycle,
                worker,
                Duration::minutes(10),
            )
            .await
            {
                Ok(Some(job)) => process_job(&state, worker, job).await,
                Ok(None) => {}
                Err(error) => tracing::error!(%error, "领取生命周期任务失败"),
            }
        }
    })
}

async fn process_job(state: &WebState, worker: Uuid, job: BackgroundJob) {
    let result = if job.payload["operation"] == "purge" {
        run_purge(state, worker, &job).await
    } else {
        run_move(state, worker, &job).await
    };
    match result {
        Ok(value) => {
            let _ = pacs_db::complete_background_job(&state.pool, job.id, worker, &value).await;
        }
        Err(error) => {
            tracing::error!(%error, job_id=%job.id, "生命周期任务失败");
            if job.payload["operation"] == "purge"
                && let Ok(request_id) =
                    serde_json::from_value::<Uuid>(job.payload["request_id"].clone())
            {
                let _ = pacs_db::record_purge_error(
                    &state.pool,
                    job.institution_id,
                    request_id,
                    &error.to_string(),
                )
                .await;
            }
            let _ = pacs_db::fail_background_job(
                &state.pool,
                job.id,
                worker,
                &error.to_string(),
                Some(Utc::now() + Duration::seconds(10)),
            )
            .await;
        }
    }
}

async fn run_move(
    state: &WebState,
    worker: Uuid,
    job: &BackgroundJob,
) -> Result<Value, LifecycleError> {
    let target: StorageTier = serde_json::from_value(job.payload["target_tier"].clone())
        .map_err(|_| LifecycleError::BadRequest("生命周期任务目标层级无效".to_owned()))?;
    let store = state.store.as_ref().ok_or(LifecycleError::Storage)?;
    let items = pacs_db::list_background_job_items(&state.pool, job.institution_id, job.id).await?;
    let mut moved = 0_i64;
    let mut skipped = 0_i64;
    for item in items {
        match item.status {
            pacs_db::JobItemStatus::Succeeded => {
                moved += 1;
                continue;
            }
            pacs_db::JobItemStatus::Skipped => {
                skipped += 1;
                continue;
            }
            pacs_db::JobItemStatus::Conflict | pacs_db::JobItemStatus::Cancelled => continue,
            status if retryable_move_item(status) => {}
            _ => continue,
        }
        pacs_db::start_background_job_item(&state.pool, job.id, &item.item_key).await?;
        let study_uid = item.item_key;
        let result = move_one(state, store, job, &study_uid, target).await;
        match result {
            Ok(true) => {
                moved += 1;
                pacs_db::finish_background_job_item(
                    &state.pool,
                    job.id,
                    &study_uid,
                    pacs_db::JobItemStatus::Succeeded,
                    &json!({"target_tier":target}),
                    None,
                )
                .await?;
            }
            Ok(false) => {
                skipped += 1;
                pacs_db::finish_background_job_item(
                    &state.pool,
                    job.id,
                    &study_uid,
                    pacs_db::JobItemStatus::Skipped,
                    &json!({"reason":"already_in_target_tier"}),
                    None,
                )
                .await?;
            }
            Err(error) => {
                let _ = pacs_db::finish_background_job_item(
                    &state.pool,
                    job.id,
                    &study_uid,
                    pacs_db::JobItemStatus::Failed,
                    &json!({}),
                    Some(&error.to_string()),
                )
                .await;
                return Err(error);
            }
        }
        pacs_db::update_background_job_progress(
            &state.pool,
            job.id,
            worker,
            moved + skipped,
            job.progress_total,
        )
        .await?;
    }
    Ok(json!({"moved":moved,"skipped":skipped,"target_tier":target}))
}

fn retryable_move_item(status: pacs_db::JobItemStatus) -> bool {
    matches!(
        status,
        pacs_db::JobItemStatus::Pending
            | pacs_db::JobItemStatus::Running
            | pacs_db::JobItemStatus::Failed
    )
}

async fn move_one(
    state: &WebState,
    store: &pacs_store::Store,
    job: &BackgroundJob,
    study_uid: &str,
    target: StorageTier,
) -> Result<bool, LifecycleError> {
    let (current, files) =
        pacs_db::lifecycle_files_for_study(&state.pool, job.institution_id, study_uid).await?;
    if current == target {
        for file in &files {
            for tier in [StorageTier::Hot, StorageTier::Cold, StorageTier::Quarantine] {
                if tier == target {
                    continue;
                }
                let source = store.tier_relative_path(&file.storage_path, to_store_tier(tier))?;
                store
                    .remove_after_verified_copy(&source, &file.storage_path, &file.file_sha256)
                    .await?;
            }
        }
        return Ok(false);
    }
    if files.is_empty() {
        return Err(LifecycleError::Conflict("Study 没有实例文件".to_owned()));
    }
    let target_store = to_store_tier(target);
    let mut updates = Vec::with_capacity(files.len());
    for file in &files {
        let copy = store
            .copy_to_tier(&file.storage_path, target_store, &file.file_sha256)
            .await?;
        updates.push(pacs_db::LifecyclePathUpdate {
            version_id: file.version_id,
            old_path: file.storage_path.clone(),
            new_path: copy.destination_relative_path,
        });
    }
    if let Err(error) = pacs_db::switch_study_storage_tier(
        &state.pool,
        job.institution_id,
        study_uid,
        current,
        target,
        &updates,
        job.id,
        job.created_by,
    )
    .await
    {
        return Err(error.into());
    }
    for (file, update) in files.iter().zip(&updates) {
        if let Err(error) = store
            .verify_sha256(&update.new_path, &file.file_sha256)
            .await
        {
            let reverse: Vec<_> = updates
                .iter()
                .map(|item| pacs_db::LifecyclePathUpdate {
                    version_id: item.version_id,
                    old_path: item.new_path.clone(),
                    new_path: item.old_path.clone(),
                })
                .collect();
            let _ = pacs_db::switch_study_storage_tier(
                &state.pool,
                job.institution_id,
                study_uid,
                target,
                current,
                &reverse,
                job.id,
                job.created_by,
            )
            .await;
            return Err(error.into());
        }
    }
    for (file, update) in files.iter().zip(&updates) {
        store
            .remove_after_verified_copy(&file.storage_path, &update.new_path, &file.file_sha256)
            .await?;
    }
    Ok(true)
}

async fn run_purge(
    state: &WebState,
    worker: Uuid,
    job: &BackgroundJob,
) -> Result<Value, LifecycleError> {
    let request_id: Uuid = serde_json::from_value(job.payload["request_id"].clone())
        .map_err(|_| LifecycleError::BadRequest("清除任务缺少申请 ID".to_owned()))?;
    let uid = pacs_db::begin_purge(&state.pool, job.institution_id, request_id).await?;
    let files = pacs_db::commit_purge_metadata(&state.pool, job.institution_id, request_id).await?;
    let store = state.store.as_ref().ok_or(LifecycleError::Storage)?;
    let mut deleted = 0_i64;
    for file in files {
        if file.deleted_at.is_some() {
            continue;
        }
        if file.storage_kind == "dicom" {
            store
                .remove_quarantined(&file.relative_path, &file.file_sha256)
                .await?;
        } else {
            remove_transfer_file(store, &file.relative_path, &file.file_sha256).await?;
        }
        pacs_db::mark_purge_file_deleted(
            &state.pool,
            request_id,
            &file.storage_kind,
            &file.relative_path,
        )
        .await?;
        deleted += 1;
    }
    pacs_db::finalize_purge(
        &state.pool,
        job.institution_id,
        request_id,
        job.id,
        job.created_by,
    )
    .await?;
    pacs_db::update_background_job_progress(
        &state.pool,
        job.id,
        worker,
        job.progress_total,
        job.progress_total,
    )
    .await?;
    Ok(json!({"study_instance_uid":uid,"deleted_files":deleted}))
}

async fn remove_transfer_file(
    store: &pacs_store::Store,
    relative: &str,
    expected: &[u8],
) -> Result<(), LifecycleError> {
    let path = Path::new(relative);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(LifecycleError::BadRequest("导出缓存路径无效".to_owned()));
    }
    let relative_store = Path::new(".transfers")
        .join(path)
        .to_string_lossy()
        .into_owned();
    let absolute = match store.resolve_for_read(&relative_store).await {
        Ok(path) => path,
        Err(pacs_store::StoreError::NotFound { .. }) => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let bytes = tokio::fs::read(&absolute).await?;
    let actual: [u8; 32] = Sha256::digest(&bytes).into();
    if actual.as_slice() != expected {
        return Err(LifecycleError::Conflict(
            "导出缓存 SHA-256 校验失败".to_owned(),
        ));
    }
    match tokio::fs::remove_file(&absolute).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn to_store_tier(tier: StorageTier) -> StoreTier {
    match tier {
        StorageTier::Hot => StoreTier::Hot,
        StorageTier::Cold => StoreTier::Cold,
        StorageTier::Quarantine => StoreTier::Quarantine,
    }
}

async fn schedule_due_policies(state: &WebState) -> Result<(), LifecycleError> {
    let policies =
        pacs_db::list_due_lifecycle_policies(&state.pool, Utc::now() - Duration::hours(1)).await?;
    if policies.is_empty() {
        return Ok(());
    }
    let used_percent = storage_used_percent(state).await?;
    let window = Utc::now().timestamp() / 3600;
    for policy in policies {
        let threshold_met = policy
            .minimum_storage_used_percent
            .is_none_or(|value| used_percent >= value);
        let matches = pacs_db::preview_lifecycle_policy(
            &state.pool,
            policy.institution_id,
            &policy,
            threshold_met,
            10_000,
        )
        .await?;
        let studies: Vec<String> = matches
            .into_iter()
            .map(|study| study.study_instance_uid)
            .collect();
        let payload = json!({"operation":"move","target_tier":policy.target_tier,
            "study_uids":studies,"policy_id":policy.id,"scheduled":true});
        let key = format!("policy:{}:{window}", policy.id);
        create_lifecycle_job(
            state,
            policy.institution_id,
            None,
            payload,
            &studies,
            Some(&key),
        )
        .await?;
        pacs_db::mark_lifecycle_policy_run(&state.pool, policy.institution_id, policy.id).await?;
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
enum LifecycleError {
    #[error("请求参数错误: {0}")]
    BadRequest(String),
    #[error("并发冲突: {0}")]
    Conflict(String),
    #[error("资源不存在")]
    NotFound,
    #[error("存储未配置")]
    Storage,
    #[error("认证身份缺失")]
    Identity,
    #[error(transparent)]
    Db(#[from] pacs_db::DbError),
    #[error(transparent)]
    Store(#[from] pacs_store::StoreError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl IntoResponse for LifecycleError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::BadRequest(_) | Self::Db(pacs_db::DbError::Invalid(_)) => StatusCode::BAD_REQUEST,
            Self::Conflict(_) | Self::Db(pacs_db::DbError::Conflict(_)) => StatusCode::CONFLICT,
            Self::Db(pacs_db::DbError::Query(sqlx::Error::Database(error)))
                if error.is_unique_violation() =>
            {
                StatusCode::CONFLICT
            }
            Self::NotFound | Self::Db(pacs_db::DbError::NotFound) => StatusCode::NOT_FOUND,
            Self::Store(pacs_store::StoreError::ContentConflict { .. })
            | Self::Store(pacs_store::StoreError::DestinationExists { .. }) => StatusCode::CONFLICT,
            Self::Identity => StatusCode::UNAUTHORIZED,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(json!({"error":self.to_string()}))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_signature_changes_when_a_match_condition_changes() {
        let a = PolicyInput {
            name: "old".to_owned(),
            priority: 100,
            enabled: false,
            target_tier: StorageTier::Cold,
            modalities: vec!["CT".to_owned()],
            study_date_before: None,
            last_accessed_before: None,
            tag_matches: json!({}),
            minimum_study_bytes: None,
            minimum_storage_used_percent: None,
        };
        let mut b = PolicyInput {
            name: "old".to_owned(),
            priority: 100,
            enabled: false,
            target_tier: StorageTier::Cold,
            modalities: vec!["MR".to_owned()],
            study_date_before: None,
            last_accessed_before: None,
            tag_matches: json!({}),
            minimum_study_bytes: None,
            minimum_storage_used_percent: None,
        };
        assert_ne!(policy_signature(&a).unwrap(), policy_signature(&b).unwrap());
        b.modalities = a.modalities.clone();
        assert_eq!(policy_signature(&a).unwrap(), policy_signature(&b).unwrap());
    }

    #[test]
    fn purge_grace_window_has_a_bounded_default() {
        const {
            assert!(DEFAULT_GRACE_HOURS > 0 && DEFAULT_GRACE_HOURS < MAX_GRACE_HOURS);
        }
    }

    #[test]
    fn lifecycle_retries_only_unfinished_items() {
        assert!(retryable_move_item(pacs_db::JobItemStatus::Pending));
        assert!(retryable_move_item(pacs_db::JobItemStatus::Running));
        assert!(retryable_move_item(pacs_db::JobItemStatus::Failed));
        assert!(!retryable_move_item(pacs_db::JobItemStatus::Succeeded));
        assert!(!retryable_move_item(pacs_db::JobItemStatus::Skipped));
        assert!(!retryable_move_item(pacs_db::JobItemStatus::Conflict));
        assert!(!retryable_move_item(pacs_db::JobItemStatus::Cancelled));
    }
}
