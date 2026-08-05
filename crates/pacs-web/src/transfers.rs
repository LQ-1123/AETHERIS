//! Resumable bulk imports and auditable ZIP exports.

use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Extension, Path as UrlPath, Query, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use compress_tools::{ArchiveContents, ArchiveIterator};
use pacs_auth::service_accounts::{ApiScope, ServiceIdentity};
use pacs_auth::{AuthService, Identity, Permission};
use pacs_db::{BackgroundJob, ImportUpload, JobItemStatus, JobKind, NewJob, UploadStatus};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use uuid::Uuid;

use crate::WebState;

const CHUNK_LIMIT: usize = 8 * 1024 * 1024;
const FILE_LIMIT: i64 = 1024 * 1024 * 1024;
const ARCHIVE_ENTRY_LIMIT: usize = 100_000;
const ARCHIVE_TOTAL_LIMIT: u64 = 20 * 1024 * 1024 * 1024;
const ARCHIVE_RATIO_LIMIT: u64 = 100;
const RATIO_ALLOWANCE: u64 = 100 * 1024 * 1024;

pub fn transfer_routes(state: WebState, auth: Arc<AuthService>) -> Router {
    let import_auth = Arc::clone(&auth);
    let imports = Router::new()
        .route("/imports", post(create_import))
        .route("/imports/{job_id}/files", post(create_upload))
        .route("/imports/{job_id}/files/{upload_id}", put(upload_chunk))
        .route("/imports/{job_id}/complete", post(complete_import))
        .route(
            "/imports/{job_id}",
            get(get_transfer).delete(cancel_transfer),
        )
        .layer(axum::middleware::from_fn(move |request, next| {
            let auth = Arc::clone(&import_auth);
            async move {
                require_transfer(
                    auth,
                    ApiScope::Upload,
                    Permission::UploadImages,
                    request,
                    next,
                )
                .await
            }
        }));
    let exports = Router::new()
        .route("/exports", post(create_export))
        .route(
            "/exports/{job_id}",
            get(get_transfer).delete(cancel_transfer),
        )
        .route("/exports/{job_id}/download", get(download_export))
        .layer(axum::middleware::from_fn(move |request, next| {
            let auth = Arc::clone(&auth);
            async move {
                require_transfer(
                    auth,
                    ApiScope::Export,
                    Permission::ViewImages,
                    request,
                    next,
                )
                .await
            }
        }));
    imports.merge(exports).with_state(state)
}

async fn require_transfer(
    auth: Arc<AuthService>,
    scope: ApiScope,
    permission: Permission,
    request: Request,
    next: Next,
) -> Response {
    let service = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.strip_prefix("Bearer ")
                .is_some_and(|token| token.starts_with("pacs_sk_"))
        });
    if service {
        pacs_auth::service_accounts::require_api_scope(auth, scope, request, next).await
    } else {
        pacs_auth::require(auth, permission, request, next).await
    }
}

fn institution(
    user: Option<&Extension<Identity>>,
    service: Option<&Extension<ServiceIdentity>>,
) -> Result<i64, TransferError> {
    user.map(|v| v.institution_id)
        .or_else(|| service.map(|v| v.institution_id))
        .ok_or(TransferError::Identity)
}

#[derive(Deserialize)]
struct CreateImport {
    idempotency_key: Option<String>,
}

async fn create_import(
    State(state): State<WebState>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
    Json(input): Json<CreateImport>,
) -> Result<(StatusCode, Json<Value>), TransferError> {
    let institution_id = institution(user.as_ref(), service.as_ref())?;
    let id = Uuid::new_v4();
    let job = pacs_db::create_background_job(
        &state.pool,
        NewJob {
            id,
            institution_id,
            created_by: user.as_ref().map(|v| v.user_id),
            kind: JobKind::Import,
            idempotency_key: input.idempotency_key.as_deref(),
            payload: &json!({}),
            progress_total: 0,
            max_attempts: 3,
            available_at: Some(Utc::now() + Duration::days(36_500)),
        },
    )
    .await?;
    tokio::fs::create_dir_all(transfer_root(&state)?.join("uploads")).await?;
    Ok((StatusCode::CREATED, Json(json!({"job": job}))))
}

#[derive(Deserialize)]
struct CreateUpload {
    relative_name: String,
    size: i64,
    sha256: Option<String>,
}

async fn create_upload(
    State(state): State<WebState>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
    UrlPath(job_id): UrlPath<Uuid>,
    Json(input): Json<CreateUpload>,
) -> Result<(StatusCode, Json<ImportUpload>), TransferError> {
    let institution_id = institution(user.as_ref(), service.as_ref())?;
    validate_relative_name(&input.relative_name)?;
    if input.size < 0 || input.size > FILE_LIMIT {
        return Err(TransferError::BadRequest("文件大小超出限制".to_owned()));
    }
    let expected_sha256 = input.sha256.as_deref().map(parse_sha256).transpose()?;
    let upload_id = Uuid::new_v4();
    let temp_name = format!("{upload_id}.part");
    let path = transfer_root(&state)?.join("uploads").join(&temp_name);
    let file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .await?;
    file.sync_all().await?;
    let upload = ImportUpload {
        id: upload_id,
        job_id,
        relative_name: input.relative_name,
        expected_size: input.size,
        expected_sha256,
        received_size: 0,
        temp_name,
        status: UploadStatus::Uploading,
        error_message: None,
    };
    match pacs_db::create_import_upload(&state.pool, institution_id, &upload).await {
        Ok(upload) => Ok((StatusCode::CREATED, Json(upload))),
        Err(error) => {
            let _ = tokio::fs::remove_file(path).await;
            Err(error.into())
        }
    }
}

#[derive(Deserialize)]
struct Offset {
    offset: i64,
}

async fn upload_chunk(
    State(state): State<WebState>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
    UrlPath((job_id, upload_id)): UrlPath<(Uuid, Uuid)>,
    Query(query): Query<Offset>,
    body: Bytes,
) -> Result<Json<ImportUpload>, TransferError> {
    if body.is_empty() || body.len() > CHUNK_LIMIT {
        return Err(TransferError::Payload);
    }
    let institution_id = institution(user.as_ref(), service.as_ref())?;
    let uploads = pacs_db::list_import_uploads(&state.pool, institution_id, job_id).await?;
    let upload = uploads
        .into_iter()
        .find(|v| v.id == upload_id)
        .ok_or(TransferError::NotFound)?;
    if upload.received_size != query.offset {
        return Err(TransferError::Conflict(format!(
            "期望偏移 {}",
            upload.received_size
        )));
    }
    let path = transfer_root(&state)?
        .join("uploads")
        .join(&upload.temp_name);
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .await?;
    file.seek(std::io::SeekFrom::Start(query.offset as u64))
        .await?;
    file.write_all(&body).await?;
    file.sync_data().await?;
    let advanced = pacs_db::advance_upload(
        &state.pool,
        institution_id,
        upload_id,
        query.offset,
        body.len() as i64,
    )
    .await?;
    Ok(Json(advanced))
}

async fn complete_import(
    State(state): State<WebState>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
    UrlPath(job_id): UrlPath<Uuid>,
) -> Result<(StatusCode, Json<Value>), TransferError> {
    let institution_id = institution(user.as_ref(), service.as_ref())?;
    let uploads = pacs_db::list_import_uploads(&state.pool, institution_id, job_id).await?;
    if uploads.is_empty() {
        return Err(TransferError::BadRequest("导入任务没有文件".to_owned()));
    }
    for upload in uploads {
        if upload.received_size != upload.expected_size {
            return Err(TransferError::Conflict(format!(
                "文件 {} 尚未上传完成",
                upload.relative_name
            )));
        }
        let path = transfer_root(&state)?
            .join("uploads")
            .join(&upload.temp_name);
        let digest: [u8; 32] = Sha256::digest(tokio::fs::read(&path).await?).into();
        if upload
            .expected_sha256
            .as_deref()
            .is_some_and(|expected| expected != digest)
        {
            pacs_db::mark_upload_failed(&state.pool, institution_id, upload.id, "SHA-256 不匹配")
                .await?;
            return Err(TransferError::Conflict(format!(
                "文件 {} 的 SHA-256 不匹配",
                upload.relative_name
            )));
        }
        pacs_db::mark_upload_ready(&state.pool, institution_id, upload.id).await?;
    }
    let job = pacs_db::release_background_job(&state.pool, institution_id, job_id).await?;
    Ok((StatusCode::ACCEPTED, Json(json!({"job": job}))))
}

#[derive(Deserialize)]
struct CreateExport {
    study_instance_uid: String,
    series_instance_uid: Option<String>,
    idempotency_key: Option<String>,
}

async fn create_export(
    State(state): State<WebState>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
    Json(input): Json<CreateExport>,
) -> Result<(StatusCode, Json<Value>), TransferError> {
    let institution_id = institution(user.as_ref(), service.as_ref())?;
    pacs_core::Uid::parse(&input.study_instance_uid)
        .map_err(|e| TransferError::BadRequest(e.to_string()))?;
    if let Some(uid) = &input.series_instance_uid {
        pacs_core::Uid::parse(uid).map_err(|e| TransferError::BadRequest(e.to_string()))?;
    }
    let sources = pacs_db::list_export_sources(
        &state.pool,
        institution_id,
        &input.study_instance_uid,
        input.series_instance_uid.as_deref(),
    )
    .await?;
    if sources.is_empty() {
        return Err(TransferError::NotFound);
    }
    let payload = json!({"study_instance_uid": input.study_instance_uid, "series_instance_uid": input.series_instance_uid});
    let job = pacs_db::create_background_job(
        &state.pool,
        NewJob {
            id: Uuid::new_v4(),
            institution_id,
            created_by: user.as_ref().map(|v| v.user_id),
            kind: JobKind::Export,
            idempotency_key: input.idempotency_key.as_deref(),
            payload: &payload,
            progress_total: sources.len() as i64,
            max_attempts: 3,
            available_at: None,
        },
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(json!({"job": job}))))
}

async fn get_transfer(
    State(state): State<WebState>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
    UrlPath(job_id): UrlPath<Uuid>,
) -> Result<Json<Value>, TransferError> {
    let institution_id = institution(user.as_ref(), service.as_ref())?;
    let job = pacs_db::get_background_job(&state.pool, institution_id, job_id).await?;
    let items = pacs_db::list_background_job_items(&state.pool, institution_id, job_id).await?;
    let uploads = if job.kind == JobKind::Import {
        pacs_db::list_import_uploads(&state.pool, institution_id, job_id).await?
    } else {
        Vec::new()
    };
    Ok(Json(
        json!({"job": job, "items": items, "uploads": uploads}),
    ))
}

async fn cancel_transfer(
    State(state): State<WebState>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
    UrlPath(job_id): UrlPath<Uuid>,
) -> Result<Json<Value>, TransferError> {
    let job = pacs_db::request_job_cancellation(
        &state.pool,
        institution(user.as_ref(), service.as_ref())?,
        job_id,
    )
    .await?;
    Ok(Json(json!({"job": job})))
}

async fn download_export(
    State(state): State<WebState>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
    UrlPath(job_id): UrlPath<Uuid>,
) -> Result<Response, TransferError> {
    let artifact = pacs_db::find_export_artifact(
        &state.pool,
        institution(user.as_ref(), service.as_ref())?,
        job_id,
    )
    .await?;
    let path = transfer_root(&state)?.join(&artifact.relative_path);
    let bytes = tokio::fs::read(path).await?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/zip"),
            (
                header::CONTENT_DISPOSITION,
                &format!("attachment; filename=\"{}\"", artifact.download_name),
            ),
        ],
        bytes,
    )
        .into_response())
}

pub fn start_transfer_worker(state: WebState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let worker = Uuid::new_v4();
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            interval.tick().await;
            if let Err(error) = pacs_db::recover_background_jobs(&state.pool).await {
                tracing::error!(%error, "恢复传输任务失败");
            }
            for kind in [JobKind::Import, JobKind::Export] {
                match pacs_db::claim_background_job(&state.pool, kind, worker, Duration::minutes(5))
                    .await
                {
                    Ok(Some(job)) => process_job(&state, worker, job).await,
                    Ok(None) => {}
                    Err(error) => tracing::error!(%error, ?kind, "领取传输任务失败"),
                }
            }
            if let Ok(paths) = pacs_db::purge_expired_export_artifacts(&state.pool).await {
                for path in paths {
                    let _ = tokio::fs::remove_file(
                        transfer_root(&state).unwrap_or_default().join(path),
                    )
                    .await;
                }
            }
        }
    })
}

async fn process_job(state: &WebState, worker: Uuid, job: BackgroundJob) {
    let result = match job.kind {
        JobKind::Import => run_import(state, worker, &job).await,
        JobKind::Export => run_export(state, worker, &job).await,
        _ => return,
    };
    match result {
        Ok(value) => {
            if let Err(error) =
                pacs_db::complete_background_job(&state.pool, job.id, worker, &value).await
            {
                tracing::error!(%error, job_id=%job.id, "完成传输任务失败");
            }
        }
        Err(error) => {
            tracing::error!(%error, job_id=%job.id, "传输任务失败");
            let _ =
                pacs_db::fail_background_job(&state.pool, job.id, worker, &error.to_string(), None)
                    .await;
        }
    }
}

async fn run_import(
    state: &WebState,
    worker: Uuid,
    job: &BackgroundJob,
) -> Result<Value, TransferError> {
    let store = state.store.as_ref().ok_or(TransferError::Storage)?;
    let uploads = pacs_db::list_import_uploads(&state.pool, job.institution_id, job.id).await?;
    let total = uploads.len() as i64;
    let mut outcomes = Vec::new();
    for (index, upload) in uploads.into_iter().enumerate() {
        if pacs_db::get_background_job(&state.pool, job.institution_id, job.id)
            .await?
            .cancel_requested
        {
            break;
        }
        let path = transfer_root(state)?
            .join("uploads")
            .join(&upload.temp_name);
        let extracted = if is_archive(&path).await? {
            Some(extract_archive(path.clone()).await?)
        } else {
            None
        };
        let entries = extracted.as_ref().map_or_else(
            || vec![(upload.relative_name.clone(), path.clone())],
            |archive| archive.entries.clone(),
        );
        for (name, entry_path) in entries {
            let bytes = tokio::fs::read(entry_path).await?;
            let item_key = format!("{}:{name}", upload.id);
            pacs_db::add_background_job_item(
                &state.pool,
                job.id,
                &item_key,
                &json!({"name": name}),
            )
            .await?;
            pacs_db::start_background_job_item(&state.pool, job.id, &item_key).await?;
            let outcome = crate::ingest::ingest_dicom_from(
                store,
                &state.pool,
                job.institution_id,
                &bytes,
                Some("FILE_IMPORT"),
            )
            .await;
            let status = match outcome.disposition {
                crate::ingest::IngestDisposition::Created
                | crate::ingest::IngestDisposition::Duplicate => JobItemStatus::Succeeded,
                crate::ingest::IngestDisposition::Conflict => JobItemStatus::Conflict,
                crate::ingest::IngestDisposition::Invalid => JobItemStatus::Skipped,
                crate::ingest::IngestDisposition::Failed => JobItemStatus::Failed,
            };
            let value = serde_json::to_value(&outcome).map_err(TransferError::Json)?;
            pacs_db::finish_background_job_item(
                &state.pool,
                job.id,
                &item_key,
                status,
                &value,
                outcome.error.as_deref(),
            )
            .await?;
            outcomes.push(value);
        }
        let _ = tokio::fs::remove_file(path).await;
        pacs_db::update_background_job_progress(
            &state.pool,
            job.id,
            worker,
            index as i64 + 1,
            total,
        )
        .await?;
    }
    let count = |name: &str| outcomes.iter().filter(|v| v["disposition"] == name).count();
    Ok(
        json!({"created": count("created"), "duplicates": count("duplicate"), "conflicts": count("conflict"), "invalid": count("invalid"), "failed": count("failed")}),
    )
}

struct ExtractedArchive {
    _directory: tempfile::TempDir,
    entries: Vec<(String, PathBuf)>,
}

async fn extract_archive(path: PathBuf) -> Result<ExtractedArchive, TransferError> {
    tokio::task::spawn_blocking(move || {
        let compressed = std::fs::metadata(&path)?.len();
        let file = std::fs::File::open(path)?;
        let iterator =
            ArchiveIterator::from_read(file).map_err(|e| TransferError::Archive(e.to_string()))?;
        let directory = tempfile::Builder::new()
            .prefix("remote-pacs-import-")
            .tempdir()?;
        let mut result = Vec::new();
        let mut current: Option<(String, std::fs::File, PathBuf, usize)> = None;
        let mut total = 0u64;
        for content in iterator {
            match content {
                ArchiveContents::StartOfEntry(name, stat) => {
                    validate_relative_name(&name)?;
                    let file_type = stat.st_mode & libc::S_IFMT;
                    if file_type == libc::S_IFDIR {
                        current = None;
                        continue;
                    }
                    if file_type != libc::S_IFREG {
                        return Err(TransferError::Archive(
                            "归档包含符号链接、硬链接或设备条目".to_owned(),
                        ));
                    }
                    if stat.st_nlink > 1 {
                        return Err(TransferError::Archive("归档包含硬链接条目".to_owned()));
                    }
                    if stat.st_size < 0 || stat.st_size > FILE_LIMIT {
                        return Err(TransferError::Archive("归档条目过大".to_owned()));
                    }
                    if result.len() >= ARCHIVE_ENTRY_LIMIT {
                        return Err(TransferError::Archive("归档条目过多".to_owned()));
                    }
                    let path = directory.path().join(Uuid::new_v4().to_string());
                    current = Some((name, std::fs::File::create(&path)?, path, 0));
                }
                ArchiveContents::DataChunk(chunk) => {
                    if let Some((_, file, _, entry_size)) = current.as_mut() {
                        total += chunk.len() as u64;
                        *entry_size += chunk.len();
                        if archive_limits_exceeded(*entry_size, total, compressed) {
                            return Err(TransferError::Archive("归档解压限制被触发".to_owned()));
                        }
                        file.write_all(&chunk)?;
                    }
                }
                ArchiveContents::EndOfEntry => {
                    if let Some((name, file, path, size)) = current.take()
                        && size != 0
                    {
                        file.sync_all()?;
                        result.push((name, path));
                    }
                }
                ArchiveContents::Err(error) => {
                    return Err(TransferError::Archive(error.to_string()));
                }
            }
        }
        Ok(ExtractedArchive {
            _directory: directory,
            entries: result,
        })
    })
    .await
    .map_err(|e| TransferError::Archive(e.to_string()))?
}

async fn is_archive(path: &Path) -> Result<bool, TransferError> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut bytes = [0u8; 8];
    let count = tokio::io::AsyncReadExt::read(&mut file, &mut bytes).await?;
    let bytes = &bytes[..count];
    Ok(bytes.starts_with(b"PK\x03\x04")
        || bytes.starts_with(b"PK\x05\x06")
        || bytes.starts_with(b"Rar!\x1a\x07"))
}

async fn run_export(
    state: &WebState,
    worker: Uuid,
    job: &BackgroundJob,
) -> Result<Value, TransferError> {
    let study = job.payload["study_instance_uid"]
        .as_str()
        .ok_or_else(|| TransferError::BadRequest("导出任务缺少 StudyInstanceUID".to_owned()))?;
    let series = job.payload["series_instance_uid"].as_str();
    let sources =
        pacs_db::list_export_sources(&state.pool, job.institution_id, study, series).await?;
    let store = state.store.as_ref().ok_or(TransferError::Storage)?;
    let mut resolved = Vec::new();
    for source in sources {
        resolved.push((
            source.clone(),
            store.resolve_for_read(&source.storage_path).await?,
        ));
    }
    let dir = transfer_root(state)?.join("exports");
    tokio::fs::create_dir_all(&dir).await?;
    let relative = format!("exports/{}.zip", job.id);
    let part = dir.join(format!("{}.part", job.id));
    let final_path = transfer_root(state)?.join(&relative);
    let worker_part = part.clone();
    let manifest = tokio::task::spawn_blocking(move || write_export_zip(&worker_part, &resolved))
        .await
        .map_err(|e| TransferError::Archive(e.to_string()))??;
    tokio::fs::rename(&part, &final_path).await?;
    let bytes = tokio::fs::read(&final_path).await?;
    let hash: [u8; 32] = Sha256::digest(&bytes).into();
    let name = if let Some(series) = series {
        format!("series-{series}.zip")
    } else {
        format!("study-{study}.zip")
    };
    let artifact = pacs_db::ExportArtifact {
        job_id: job.id,
        relative_path: relative,
        file_size: bytes.len() as i64,
        file_sha256: hash.to_vec(),
        download_name: name,
        expires_at: Utc::now() + Duration::hours(24),
    };
    pacs_db::save_export_artifact(&state.pool, &artifact).await?;
    pacs_db::update_background_job_progress(
        &state.pool,
        job.id,
        worker,
        job.progress_total,
        job.progress_total,
    )
    .await?;
    Ok(json!({"artifact": artifact, "manifest": manifest}))
}

fn write_export_zip(
    path: &Path,
    sources: &[(pacs_db::ExportSource, PathBuf)],
) -> Result<Value, TransferError> {
    let file = std::fs::File::create(path)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut entries = Vec::new();
    for (source, path) in sources {
        let entry_path = format!(
            "dicom/{}/{}/{}.dcm",
            source.study_uid, source.series_uid, source.sop_uid
        );
        let bytes = std::fs::read(path)?;
        let hash: [u8; 32] = Sha256::digest(&bytes).into();
        zip.start_file(&entry_path, options)
            .map_err(|e| TransferError::Archive(e.to_string()))?;
        zip.write_all(&bytes)?;
        entries.push(json!({"study_instance_uid": source.study_uid, "series_instance_uid": source.series_uid,
            "sop_instance_uid": source.sop_uid, "path": entry_path, "size": bytes.len(), "sha256": hex(&hash)}));
    }
    let manifest = json!({"version": 1, "instances": entries});
    zip.start_file("manifest.json", options)
        .map_err(|e| TransferError::Archive(e.to_string()))?;
    zip.write_all(&serde_json::to_vec_pretty(&manifest).map_err(TransferError::Json)?)?;
    let file = zip
        .finish()
        .map_err(|e| TransferError::Archive(e.to_string()))?;
    file.sync_all()?;
    Ok(manifest)
}

fn transfer_root(state: &WebState) -> Result<PathBuf, TransferError> {
    Ok(state
        .store
        .as_ref()
        .ok_or(TransferError::Storage)?
        .root()
        .join(".transfers"))
}
fn validate_relative_name(name: &str) -> Result<(), TransferError> {
    let path = Path::new(name);
    if name.trim().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(TransferError::BadRequest(
            "文件名必须是安全的相对路径".to_owned(),
        ));
    }
    Ok(())
}
fn parse_sha256(value: &str) -> Result<Vec<u8>, TransferError> {
    if value.len() != 64 {
        return Err(TransferError::BadRequest(
            "SHA-256 必须是 64 位十六进制".to_owned(),
        ));
    }
    (0..32)
        .map(|i| {
            u8::from_str_radix(&value[i * 2..i * 2 + 2], 16)
                .map_err(|_| TransferError::BadRequest("SHA-256 不是十六进制".to_owned()))
        })
        .collect()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|v| format!("{v:02x}")).collect()
}

fn archive_limits_exceeded(entry_bytes: usize, total_bytes: u64, compressed_bytes: u64) -> bool {
    entry_bytes > FILE_LIMIT as usize
        || total_bytes > ARCHIVE_TOTAL_LIMIT
        || total_bytes
            > compressed_bytes
                .saturating_mul(ARCHIVE_RATIO_LIMIT)
                .saturating_add(RATIO_ALLOWANCE)
}

#[derive(Debug, thiserror::Error)]
enum TransferError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Conflict(String),
    #[error("资源不存在")]
    NotFound,
    #[error("请求分块为空或超过 8 MiB")]
    Payload,
    #[error("影像存储未配置")]
    Storage,
    #[error("认证身份缺失")]
    Identity,
    #[error("归档处理失败: {0}")]
    Archive(String),
    #[error(transparent)]
    Db(#[from] pacs_db::DbError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Store(#[from] pacs_store::StoreError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl IntoResponse for TransferError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::BadRequest(_) | Self::Archive(_) => StatusCode::BAD_REQUEST,
            Self::Conflict(_) | Self::Db(pacs_db::DbError::Conflict(_)) => StatusCode::CONFLICT,
            Self::NotFound | Self::Db(pacs_db::DbError::NotFound) => StatusCode::NOT_FOUND,
            Self::Payload => StatusCode::PAYLOAD_TOO_LARGE,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(json!({"error": self.to_string()}))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_archive_paths() {
        for name in [
            "",
            "/absolute.dcm",
            "../escape.dcm",
            "a/../../escape.dcm",
            "./file.dcm",
        ] {
            assert!(validate_relative_name(name).is_err(), "应拒绝 {name:?}");
        }
        assert!(validate_relative_name("patient/study/image.dcm").is_ok());
    }

    #[test]
    fn parses_sha256_strictly() {
        assert_eq!(parse_sha256(&"ab".repeat(32)).unwrap(), vec![0xab; 32]);
        assert!(parse_sha256("ab").is_err());
        assert!(parse_sha256(&"zz".repeat(32)).is_err());
    }

    #[test]
    fn enforces_archive_size_and_ratio_limits() {
        assert!(archive_limits_exceeded(FILE_LIMIT as usize + 1, 1, 1));
        assert!(archive_limits_exceeded(1, ARCHIVE_TOTAL_LIMIT + 1, 1));
        assert!(archive_limits_exceeded(
            1,
            RATIO_ALLOWANCE + ARCHIVE_RATIO_LIMIT + 1,
            1
        ));
        assert!(!archive_limits_exceeded(1024, 1024, 1024));
    }

    #[tokio::test]
    async fn reads_valid_zip_and_rejects_corrupt_archive() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("valid.zip");
        {
            let file = std::fs::File::create(&path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            writer
                .start_file("nested/image.dcm", zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"dicom bytes").unwrap();
            writer.finish().unwrap();
        }
        let archive = extract_archive(path).await.unwrap();
        assert_eq!(archive.entries[0].0, "nested/image.dcm");
        assert_eq!(
            std::fs::read(&archive.entries[0].1).unwrap(),
            b"dicom bytes"
        );

        let corrupt = directory.path().join("corrupt.zip");
        std::fs::write(&corrupt, b"PK\x03\x04broken").unwrap();
        assert!(extract_archive(corrupt).await.is_err());
    }

    #[tokio::test]
    async fn rejects_encrypted_zip_without_accepting_a_password() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("encrypted.dcm");
        let archive = directory.path().join("encrypted.zip");
        std::fs::write(&source, b"secret dicom").unwrap();
        let status = std::process::Command::new("/usr/bin/zip")
            .current_dir(directory.path())
            .args(["-q", "-P", "secret", "encrypted.zip", "encrypted.dcm"])
            .status()
            .unwrap();
        assert!(status.success());
        assert!(extract_archive(archive).await.is_err());
    }

    #[tokio::test]
    async fn export_manifest_contains_hash_and_can_be_read_as_an_import_archive() {
        let directory = tempfile::tempdir().unwrap();
        let source_path = directory.path().join("source.dcm");
        let output = directory.path().join("export.zip");
        std::fs::write(&source_path, b"exported dicom bytes").unwrap();
        let source = pacs_db::ExportSource {
            study_uid: "1.2.3".to_owned(),
            series_uid: "1.2.3.4".to_owned(),
            sop_uid: "1.2.3.4.5".to_owned(),
            storage_path: "unused".to_owned(),
            file_size: 20,
            file_sha256: Vec::new(),
        };
        let manifest = write_export_zip(&output, &[(source, source_path)]).unwrap();
        assert_eq!(
            manifest["instances"][0]["path"],
            "dicom/1.2.3/1.2.3.4/1.2.3.4.5.dcm"
        );
        assert_eq!(
            manifest["instances"][0]["sha256"],
            hex(&Sha256::digest(b"exported dicom bytes"))
        );
        let archive = extract_archive(output).await.unwrap();
        assert!(
            archive
                .entries
                .iter()
                .any(|(name, path)| name == "manifest.json"
                    && std::fs::metadata(path).unwrap().len() > 0)
        );
        assert!(
            archive
                .entries
                .iter()
                .any(|(name, path)| name.ends_with(".dcm")
                    && std::fs::read(path).unwrap() == b"exported dicom bytes")
        );
    }
}
