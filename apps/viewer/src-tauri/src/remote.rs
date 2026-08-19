//! PACS HTTPS 会话、工作列表和远程序列下载。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use reqwest::{Client, Method, Response, StatusCode, Url};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Clone)]
pub struct RemoteState {
    session: Arc<Mutex<Option<Session>>>,
    download_active: Arc<AtomicBool>,
    download_cancelled: Arc<AtomicBool>,
    transfer_job: Arc<Mutex<Option<Uuid>>>,
}

struct Session {
    client: Client,
    base_url: Url,
    access_token: String,
    refresh_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteUser {
    pub id: i64,
    pub username: String,
    pub display_name: Option<String>,
    pub role: String,
    pub institution_id: i64,
    pub institution_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserWindowPreset {
    pub id: i64,
    pub modality: String,
    pub name: String,
    pub center: f64,
    pub width: f64,
    pub function: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportTemplate {
    pub id: String,
    pub name: String,
    pub modality: String,
    pub body_part: Option<String>,
    pub version: i32,
    pub structure: serde_json::Value,
    pub builtin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticReport {
    pub id: String,
    pub study_uid: String,
    pub author_id: i64,
    pub author_name: String,
    pub reviewer_id: Option<i64>,
    pub reviewer_name: Option<String>,
    pub status: String,
    pub findings: String,
    pub impression: String,
    pub recommendation: Option<String>,
    pub revision: i32,
    pub access_incomplete: bool,
    pub is_positive: bool,
    pub template_payload: Option<serde_json::Value>,
    pub submitted_at: Option<String>,
    pub reviewed_at: Option<String>,
    pub review_comment: Option<String>,
    pub reviewer_modified: bool,
    pub review_required: bool,
    pub can_review: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportVersion {
    pub id: String,
    pub report_id: String,
    pub version_number: i32,
    pub findings: String,
    pub impression: String,
    pub recommendation: Option<String>,
    pub covered_series_uids: Vec<String>,
    pub access_incomplete: bool,
    pub is_positive: bool,
    pub amendment_reason: Option<String>,
    pub signed_by: i64,
    pub signed_at: String,
    pub reviewed_by: Option<i64>,
    pub reviewed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportReviewEvent {
    pub id: i64,
    pub report_id: String,
    pub actor_id: i64,
    pub actor_name: String,
    pub action: String,
    pub comment: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClinicalWorkItem {
    pub id: String,
    pub series_uid: String,
    pub study_uid: String,
    pub patient_id: String,
    pub patient_name: Option<String>,
    pub modality: Option<String>,
    pub series_description: Option<String>,
    pub device_name: String,
    pub received_date: String,
    pub status: String,
    pub assignee_id: Option<i64>,
    pub assignee_name: Option<String>,
    pub revision: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExamRequest {
    pub id: String,
    pub patient_id: String,
    pub patient_name: String,
    pub patient_birth_date: Option<String>,
    pub patient_sex: Option<String>,
    pub modality: String,
    pub body_part: String,
    pub request_type: String,
    pub clinical_indication: String,
    pub requested_by_id: i64,
    pub requested_by_name: String,
    pub requested_at: String,
    pub scheduled_at: Option<String>,
    pub status: String,
    pub study_uid: Option<String>,
    pub study_date: Option<String>,
    pub study_description: Option<String>,
    pub revision: i32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExamRequestStudyCandidate {
    pub study_uid: String,
    pub patient_id: String,
    pub patient_name: Option<String>,
    pub study_date: Option<String>,
    pub modalities: Vec<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadRow {
    pub user_id: i64,
    pub username: String,
    pub display_name: Option<String>,
    pub role: String,
    pub draft_reports: i64,
    pub submitted_reports: i64,
    pub under_review_reports: i64,
    pub signed_status_reports: i64,
    pub signed_reports: i64,
    pub reviews_completed: i64,
    pub reviewer_modifications: i64,
    pub exam_requests_created: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DicomDevice {
    pub id: String,
    pub name: String,
    pub calling_ae_title: String,
    pub source_ip: String,
    pub modality_hint: Option<String>,
    pub status: String,
    pub approved_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesSourceEntry {
    pub series_uid: String,
    pub study_uid: String,
    pub patient_id: String,
    pub patient_name: Option<String>,
    pub modality: Option<String>,
    pub description: Option<String>,
    pub instance_count: i64,
    pub source_status: String,
    pub device_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminUser {
    pub id: i64,
    pub username: String,
    pub display_name: Option<String>,
    pub role: String,
    pub is_active: bool,
    pub must_change_password: bool,
    pub last_login_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasswordResetRequest {
    pub id: i64,
    pub user_id: i64,
    pub username: String,
    pub display_name: Option<String>,
    pub status: String,
    pub requested_at: String,
    pub reviewed_by: Option<i64>,
    pub reviewer_name: Option<String>,
    pub reviewed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatientSummary {
    pub id: i64,
    pub patient_id: String,
    pub issuer_of_patient_id: Option<String>,
    pub name: Option<String>,
    pub birth_date: Option<String>,
    pub sex: Option<String>,
    pub study_count: i64,
    pub series_count: i64,
    pub instance_count: i64,
    pub latest_study_date: Option<String>,
    pub pending_studies: i64,
    pub writing_studies: i64,
    pub locked_studies: i64,
    pub signed_studies: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudySummary {
    pub study_uid: String,
    pub study_date: Option<String>,
    pub study_time: Option<String>,
    pub accession_number: Option<String>,
    pub study_id: Option<String>,
    pub description: Option<String>,
    pub referring_physician: Option<String>,
    pub modalities: Vec<String>,
    pub series_count: i32,
    pub instance_count: i32,
    pub report_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueStudyRow {
    pub patient_key: i64,
    pub study_uid: String,
    pub patient_id: String,
    pub patient_name: Option<String>,
    pub patient_sex: Option<String>,
    pub patient_birth_date: Option<String>,
    pub study_date: Option<String>,
    pub study_time: Option<String>,
    pub modalities: Vec<String>,
    pub description: Option<String>,
    pub body_parts: Vec<String>,
    pub report_status: String,
    pub has_exam_request: bool,
    pub institution_name: Option<String>,
    pub series_count: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesSummary {
    pub series_uid: String,
    pub series_number: Option<i32>,
    pub modality: Option<String>,
    pub description: Option<String>,
    pub body_part_examined: Option<String>,
    pub protocol_name: Option<String>,
    pub instance_count: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadProgress {
    pub downloaded: usize,
    pub total: usize,
}

#[derive(Debug, Deserialize)]
struct LoginResponse {
    access_token: String,
    refresh_token: String,
    user: RemoteUser,
}

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    access_token: String,
    refresh_token: String,
}

#[derive(Debug, Deserialize)]
struct ApiErrorBody {
    error: String,
}

#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    #[error("PACS 地址无效: {0}")]
    InvalidUrl(String),
    #[error("只能连接 HTTPS PACS 地址")]
    HttpsRequired,
    #[error("无法读取 CA 证书 {path}: {source}")]
    CertificateIo {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("CA 证书格式无效: {0}")]
    InvalidCertificate(String),
    #[error("尚未登录 PACS")]
    NotLoggedIn,
    #[error("PACS 请求失败: {0}")]
    Request(String),
    #[error("PACS 返回 {status}: {message}")]
    Http { status: u16, message: String },
    #[error("PACS 响应格式无效: {0}")]
    InvalidResponse(String),
    #[error("已有序列正在下载")]
    DownloadInProgress,
    #[error("已取消下载")]
    Cancelled,
}

impl RemoteState {
    pub fn new() -> Self {
        Self {
            session: Arc::new(Mutex::new(None)),
            download_active: Arc::new(AtomicBool::new(false)),
            download_cancelled: Arc::new(AtomicBool::new(false)),
            transfer_job: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn login(
        &self,
        server_url: &str,
        ca_cert_path: &Path,
        username: &str,
        password: &str,
    ) -> Result<RemoteUser, RemoteError> {
        let (client, base_url) = client_for(server_url, ca_cert_path)?;

        let response = client
            .post(endpoint(&base_url, "auth/login")?)
            .json(&serde_json::json!({ "username": username, "password": password }))
            .send()
            .await
            .map_err(request_error)?;
        let response = ensure_success(response).await?;
        let login: LoginResponse = response
            .json()
            .await
            .map_err(|error| RemoteError::InvalidResponse(error.to_string()))?;
        let user = login.user.clone();
        *self.session.lock().await = Some(Session {
            client,
            base_url,
            access_token: login.access_token,
            refresh_token: login.refresh_token,
        });
        Ok(user)
    }

    /// 未登录用户提交密码重置申请。新密码只发往服务端，管理员看不到明文。
    pub async fn request_password_reset(
        &self,
        server_url: &str,
        ca_cert_path: &Path,
        username: &str,
        new_password: &str,
    ) -> Result<(), RemoteError> {
        let (client, base_url) = client_for(server_url, ca_cert_path)?;
        let response = client
            .post(endpoint(&base_url, "auth/password-reset-requests")?)
            .json(&serde_json::json!({
                "username": username,
                "new_password": new_password,
            }))
            .send()
            .await
            .map_err(request_error)?;
        ensure_success(response).await?;
        Ok(())
    }

    pub async fn logout(&self) -> Result<(), RemoteError> {
        self.cancel_download();
        let session = self.session.lock().await.take();
        let Some(session) = session else {
            return Ok(());
        };
        let response = session
            .client
            .post(endpoint(&session.base_url, "auth/logout")?)
            .json(&serde_json::json!({ "refresh_token": session.refresh_token }))
            .send()
            .await
            .map_err(request_error)?;
        ensure_success(response).await?;
        Ok(())
    }

    pub async fn list_window_presets(&self) -> Result<Vec<UserWindowPreset>, RemoteError> {
        let url = self.session_url("api/window-presets").await?;
        self.get_json(url).await
    }

    pub async fn create_window_preset(
        &self,
        modality: &str,
        name: &str,
        center: f64,
        width: f64,
        function: &str,
    ) -> Result<UserWindowPreset, RemoteError> {
        let url = self.session_url("api/window-presets").await?;
        self.authorized_json(
            Method::POST,
            url,
            Some(serde_json::json!({
                "modality": modality,
                "name": name,
                "center": center,
                "width": width,
                "function": function,
            })),
        )
        .await
    }

    pub async fn rename_window_preset(
        &self,
        preset_id: i64,
        name: &str,
    ) -> Result<UserWindowPreset, RemoteError> {
        let url = self
            .session_url(&format!("api/window-presets/{preset_id}"))
            .await?;
        self.authorized_json(
            Method::PATCH,
            url,
            Some(serde_json::json!({ "name": name })),
        )
        .await
    }

    pub async fn delete_window_preset(&self, preset_id: i64) -> Result<(), RemoteError> {
        let url = self
            .session_url(&format!("api/window-presets/{preset_id}"))
            .await?;
        self.authorized_request(Method::DELETE, url, None).await?;
        Ok(())
    }

    pub async fn list_report_templates(
        &self,
        modality: Option<&str>,
    ) -> Result<Vec<ReportTemplate>, RemoteError> {
        let mut url = self.session_url("api/v1/report-templates").await?;
        if let Some(modality) = modality {
            url.query_pairs_mut().append_pair("modality", modality);
        }
        self.get_json(url).await
    }

    pub async fn list_reports(
        &self,
        study_uid: &str,
    ) -> Result<Vec<DiagnosticReport>, RemoteError> {
        let mut url = self.session_url("api/v1/reports").await?;
        url.query_pairs_mut().append_pair("study_uid", study_uid);
        self.get_json(url).await
    }

    pub async fn create_report(
        &self,
        study_uid: &str,
        series_uids: Vec<String>,
        template_payload: Option<serde_json::Value>,
        is_positive: bool,
    ) -> Result<DiagnosticReport, RemoteError> {
        let url = self.session_url("api/v1/reports").await?;
        self.authorized_json(
            Method::POST,
            url,
            Some(serde_json::json!({
                "study_uid": study_uid,
                "covered_series_uids": series_uids,
                "template_payload": template_payload,
                "is_positive": is_positive,
            })),
        )
        .await
    }

    pub async fn update_report_draft(
        &self,
        report_id: &str,
        revision: i32,
        findings: &str,
        impression: &str,
        recommendation: Option<&str>,
        template_payload: Option<serde_json::Value>,
        is_positive: bool,
        clear_template_payload: bool,
    ) -> Result<DiagnosticReport, RemoteError> {
        let url = self
            .session_url(&format!("api/v1/reports/{report_id}/draft"))
            .await?;
        self.authorized_json(
            Method::PUT,
            url,
            Some(serde_json::json!({
                "revision": revision,
                "findings": findings,
                "impression": impression,
                "recommendation": recommendation,
                "template_payload": template_payload,
                "is_positive": is_positive,
                "clear_template_payload": clear_template_payload,
            })),
        )
        .await
    }

    pub async fn sign_report(&self, report_id: &str, revision: i32) -> Result<(), RemoteError> {
        let url = self
            .session_url(&format!("api/v1/reports/{report_id}/sign"))
            .await?;
        self.authorized_request(
            Method::POST,
            url,
            Some(serde_json::json!({ "revision": revision })),
        )
        .await?;
        Ok(())
    }

    pub async fn submit_report(&self, report_id: &str, revision: i32) -> Result<(), RemoteError> {
        let url = self
            .session_url(&format!("api/v1/reports/{report_id}/submit"))
            .await?;
        self.authorized_request(
            Method::POST,
            url,
            Some(serde_json::json!({ "revision": revision })),
        )
        .await?;
        Ok(())
    }

    pub async fn start_report_review(
        &self,
        report_id: &str,
        revision: i32,
    ) -> Result<(), RemoteError> {
        let url = self
            .session_url(&format!("api/v1/reports/{report_id}/review/start"))
            .await?;
        self.authorized_request(
            Method::POST,
            url,
            Some(serde_json::json!({ "revision": revision })),
        )
        .await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn approve_report(
        &self,
        report_id: &str,
        revision: i32,
        modified: bool,
        findings: Option<&str>,
        impression: Option<&str>,
        recommendation: Option<&str>,
        review_comment: Option<&str>,
    ) -> Result<(), RemoteError> {
        let url = self
            .session_url(&format!("api/v1/reports/{report_id}/review/approve"))
            .await?;
        let content = modified.then(|| {
            serde_json::json!({
                "findings": findings,
                "impression": impression,
                "recommendation": recommendation,
            })
        });
        self.authorized_request(
            Method::POST,
            url,
            Some(serde_json::json!({
                "revision": revision,
                "modified": modified,
                "content": content,
                "review_comment": review_comment,
            })),
        )
        .await?;
        Ok(())
    }

    pub async fn list_report_review_events(
        &self,
        report_id: &str,
    ) -> Result<Vec<ReportReviewEvent>, RemoteError> {
        let url = self
            .session_url(&format!("api/v1/reports/{report_id}/review-events"))
            .await?;
        self.get_json(url).await
    }

    pub async fn begin_report_amendment(
        &self,
        report_id: &str,
        reason: &str,
    ) -> Result<DiagnosticReport, RemoteError> {
        let url = self
            .session_url(&format!("api/v1/reports/{report_id}/amendments"))
            .await?;
        self.authorized_json(
            Method::POST,
            url,
            Some(serde_json::json!({ "reason": reason })),
        )
        .await
    }

    pub async fn list_report_versions(
        &self,
        report_id: &str,
    ) -> Result<Vec<ReportVersion>, RemoteError> {
        let url = self
            .session_url(&format!("api/v1/reports/{report_id}/versions"))
            .await?;
        self.get_json(url).await
    }

    pub async fn list_worklist(
        &self,
        status: Option<&str>,
    ) -> Result<Vec<ClinicalWorkItem>, RemoteError> {
        let mut url = self.session_url("api/v1/worklist").await?;
        if let Some(status) = status {
            url.query_pairs_mut().append_pair("status", status);
        }
        self.get_json(url).await
    }

    pub async fn list_exam_requests(
        &self,
        status: Option<&str>,
        limit: u32,
        offset: u64,
    ) -> Result<Vec<ExamRequest>, RemoteError> {
        let mut url = self.session_url("api/v1/exam-requests").await?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs
                .append_pair("limit", &limit.to_string())
                .append_pair("offset", &offset.to_string());
            if let Some(status) = status.filter(|value| !value.is_empty()) {
                pairs.append_pair("status", status);
            }
        }
        self.get_json(url).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_exam_request(
        &self,
        patient_id: &str,
        patient_name: &str,
        patient_birth_date: Option<&str>,
        patient_sex: Option<&str>,
        modality: &str,
        body_part: &str,
        request_type: &str,
        clinical_indication: &str,
        scheduled_at: Option<&str>,
    ) -> Result<ExamRequest, RemoteError> {
        let url = self.session_url("api/v1/exam-requests").await?;
        self.authorized_json(
            Method::POST,
            url,
            Some(serde_json::json!({
                "patient_id": patient_id,
                "patient_name": patient_name,
                "patient_birth_date": patient_birth_date,
                "patient_sex": patient_sex,
                "modality": modality,
                "body_part": body_part,
                "request_type": request_type,
                "clinical_indication": clinical_indication,
                "scheduled_at": scheduled_at,
            })),
        )
        .await
    }

    pub async fn create_exam_request_for_study(
        &self,
        study_uid: &str,
        modality: &str,
        body_part: &str,
        request_type: &str,
        clinical_indication: &str,
        scheduled_at: Option<&str>,
    ) -> Result<ExamRequest, RemoteError> {
        validate_uid(study_uid)?;
        let url = self
            .session_url(&format!("api/v1/exam-requests/study/{study_uid}"))
            .await?;
        self.authorized_json(
            Method::POST,
            url,
            Some(serde_json::json!({
                "modality": modality,
                "body_part": body_part,
                "request_type": request_type,
                "clinical_indication": clinical_indication,
                "scheduled_at": scheduled_at,
            })),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_exam_request(
        &self,
        request_id: &str,
        revision: i32,
        patient_id: &str,
        patient_name: &str,
        patient_birth_date: Option<&str>,
        patient_sex: Option<&str>,
        modality: &str,
        body_part: &str,
        request_type: &str,
        clinical_indication: &str,
        scheduled_at: Option<&str>,
    ) -> Result<ExamRequest, RemoteError> {
        let url = self
            .session_url(&format!("api/v1/exam-requests/{request_id}"))
            .await?;
        self.authorized_json(
            Method::PUT,
            url,
            Some(serde_json::json!({
                "revision": revision,
                "patient_id": patient_id,
                "patient_name": patient_name,
                "patient_birth_date": patient_birth_date,
                "patient_sex": patient_sex,
                "modality": modality,
                "body_part": body_part,
                "request_type": request_type,
                "clinical_indication": clinical_indication,
                "scheduled_at": scheduled_at,
            })),
        )
        .await
    }

    pub async fn bind_exam_request(
        &self,
        request_id: &str,
        study_uid: &str,
        revision: i32,
    ) -> Result<ExamRequest, RemoteError> {
        validate_uid(study_uid)?;
        let url = self
            .session_url(&format!("api/v1/exam-requests/{request_id}/bind"))
            .await?;
        self.authorized_json(
            Method::POST,
            url,
            Some(serde_json::json!({"study_uid":study_uid,"revision":revision})),
        )
        .await
    }

    pub async fn exam_request_for_study(
        &self,
        study_uid: &str,
    ) -> Result<Option<ExamRequest>, RemoteError> {
        validate_uid(study_uid)?;
        let url = self
            .session_url(&format!("api/v1/exam-requests/study/{study_uid}"))
            .await?;
        self.get_json(url).await
    }

    pub async fn list_exam_request_study_candidates(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<ExamRequestStudyCandidate>, RemoteError> {
        let mut url = self
            .session_url("api/v1/exam-requests/study-candidates")
            .await?;
        url.query_pairs_mut()
            .append_pair("query", query)
            .append_pair("limit", &limit.to_string());
        self.get_json(url).await
    }

    pub async fn workload_report(
        &self,
        date_from: &str,
        date_to: &str,
    ) -> Result<Vec<WorkloadRow>, RemoteError> {
        let mut url = self.session_url("api/v1/workload").await?;
        url.query_pairs_mut()
            .append_pair("date_from", date_from)
            .append_pair("date_to", date_to);
        self.get_json(url).await
    }

    pub async fn work_item_for_series(
        &self,
        series_uid: &str,
    ) -> Result<ClinicalWorkItem, RemoteError> {
        let url = self
            .session_url(&format!("api/v1/worklist/series/{series_uid}"))
            .await?;
        self.get_json(url).await
    }

    pub async fn study_work_items(
        &self,
        study_uid: &str,
    ) -> Result<Vec<ClinicalWorkItem>, RemoteError> {
        let url = self
            .session_url(&format!("api/v1/worklist/study/{study_uid}"))
            .await?;
        self.get_json(url).await
    }

    pub async fn claim_study(&self, study_uid: &str) -> Result<usize, RemoteError> {
        let url = self
            .session_url(&format!("api/v1/worklist/study/{study_uid}/claim"))
            .await?;
        self.authorized_json(Method::POST, url, None).await
    }

    pub async fn release_study(&self, study_uid: &str) -> Result<(), RemoteError> {
        let url = self
            .session_url(&format!("api/v1/worklist/study/{study_uid}/release"))
            .await?;
        self.authorized_request(Method::POST, url, None).await?;
        Ok(())
    }

    pub async fn claim_work_item(&self, work_id: &str, revision: i32) -> Result<(), RemoteError> {
        let url = self
            .session_url(&format!("api/v1/worklist/{work_id}/claim"))
            .await?;
        self.authorized_request(
            Method::POST,
            url,
            Some(serde_json::json!({ "revision": revision })),
        )
        .await?;
        Ok(())
    }

    pub async fn release_work_item(&self, work_id: &str, revision: i32) -> Result<(), RemoteError> {
        let url = self
            .session_url(&format!("api/v1/worklist/{work_id}/release"))
            .await?;
        self.authorized_request(
            Method::POST,
            url,
            Some(serde_json::json!({ "revision": revision })),
        )
        .await?;
        Ok(())
    }

    pub async fn register_device(
        &self,
        name: &str,
        calling_ae_title: &str,
        source_ip: &str,
        modality_hint: Option<&str>,
    ) -> Result<DicomDevice, RemoteError> {
        let url = self.session_url("api/v1/devices").await?;
        self.authorized_json(
            Method::POST,
            url,
            Some(serde_json::json!({
                "name": name,
                "calling_ae_title": calling_ae_title,
                "source_ip": source_ip,
                "modality_hint": modality_hint,
            })),
        )
        .await
    }

    pub async fn list_devices(
        &self,
        status: Option<&str>,
    ) -> Result<Vec<DicomDevice>, RemoteError> {
        let mut url = self.session_url("api/v1/devices").await?;
        if let Some(status) = status {
            url.query_pairs_mut().append_pair("status", status);
        }
        self.get_json(url).await
    }

    pub async fn approve_device(
        &self,
        device_id: &str,
        name: &str,
        modality_hint: Option<&str>,
    ) -> Result<DicomDevice, RemoteError> {
        let url = self
            .session_url(&format!("api/v1/devices/{device_id}/approve"))
            .await?;
        self.authorized_json(
            Method::POST,
            url,
            Some(serde_json::json!({
                "name": name,
                "modality_hint": modality_hint,
            })),
        )
        .await
    }

    pub async fn set_device_status(
        &self,
        device_id: &str,
        status: &str,
    ) -> Result<(), RemoteError> {
        let url = self
            .session_url(&format!("api/v1/devices/{device_id}"))
            .await?;
        self.authorized_request(
            Method::PATCH,
            url,
            Some(serde_json::json!({ "status": status })),
        )
        .await?;
        Ok(())
    }

    pub async fn list_series_sources(
        &self,
        unattributed: bool,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<SeriesSourceEntry>, RemoteError> {
        let mut url = self.session_url("api/v1/series-sources").await?;
        url.query_pairs_mut()
            .append_pair("unattributed", &unattributed.to_string())
            .append_pair("limit", &limit.to_string())
            .append_pair("offset", &offset.to_string());
        self.get_json(url).await
    }

    pub async fn resolve_series_source(
        &self,
        series_uid: &str,
        device_id: &str,
    ) -> Result<(), RemoteError> {
        let url = self
            .session_url(&format!("api/v1/series/{series_uid}/resolve-source"))
            .await?;
        self.authorized_request(
            Method::POST,
            url,
            Some(serde_json::json!({ "device_id": device_id })),
        )
        .await?;
        Ok(())
    }

    pub async fn list_users(&self) -> Result<Vec<AdminUser>, RemoteError> {
        let url = self.session_url("api/v1/users").await?;
        self.get_json(url).await
    }

    pub async fn create_user(
        &self,
        username: &str,
        display_name: Option<&str>,
        role: &str,
        temporary_password: &str,
    ) -> Result<AdminUser, RemoteError> {
        let url = self.session_url("api/v1/users").await?;
        self.authorized_json(
            Method::POST,
            url,
            Some(serde_json::json!({
                "username": username,
                "display_name": display_name,
                "role": role,
                "temporary_password": temporary_password,
            })),
        )
        .await
    }

    pub async fn update_user(
        &self,
        user_id: i64,
        display_name: Option<&str>,
        role: Option<&str>,
        is_active: Option<bool>,
    ) -> Result<AdminUser, RemoteError> {
        let url = self.session_url(&format!("api/v1/users/{user_id}")).await?;
        self.authorized_json(
            Method::PATCH,
            url,
            Some(serde_json::json!({
                "display_name": display_name,
                "role": role,
                "is_active": is_active,
            })),
        )
        .await
    }

    pub async fn list_password_reset_requests(
        &self,
    ) -> Result<Vec<PasswordResetRequest>, RemoteError> {
        let url = self.session_url("api/v1/password-reset-requests").await?;
        self.get_json(url).await
    }

    pub async fn review_password_reset_request(
        &self,
        request_id: i64,
        approve: bool,
    ) -> Result<PasswordResetRequest, RemoteError> {
        let decision = if approve { "approve" } else { "reject" };
        let url = self
            .session_url(&format!(
                "api/v1/password-reset-requests/{request_id}/{decision}"
            ))
            .await?;
        self.authorized_json(Method::POST, url, None).await
    }

    pub async fn list_user_permissions(&self, user_id: i64) -> Result<Vec<String>, RemoteError> {
        let url = self
            .session_url(&format!("api/v1/users/{user_id}/permissions"))
            .await?;
        self.get_json(url).await
    }

    pub async fn replace_user_permissions(
        &self,
        user_id: i64,
        permissions: Vec<String>,
    ) -> Result<Vec<String>, RemoteError> {
        let url = self
            .session_url(&format!("api/v1/users/{user_id}/permissions"))
            .await?;
        self.authorized_json(
            Method::PUT,
            url,
            Some(serde_json::json!({ "permissions": permissions })),
        )
        .await
    }

    pub async fn list_user_device_grants(&self, user_id: i64) -> Result<Vec<String>, RemoteError> {
        let url = self
            .session_url(&format!("api/v1/users/{user_id}/device-grants"))
            .await?;
        self.get_json(url).await
    }

    pub async fn replace_user_device_grants(
        &self,
        user_id: i64,
        device_ids: Vec<String>,
    ) -> Result<Vec<String>, RemoteError> {
        let url = self
            .session_url(&format!("api/v1/users/{user_id}/device-grants"))
            .await?;
        self.authorized_json(
            Method::PUT,
            url,
            Some(serde_json::json!({ "device_ids": device_ids })),
        )
        .await
    }

    pub async fn list_patients(
        &self,
        query: &str,
        limit: u32,
        offset: u64,
    ) -> Result<Vec<PatientSummary>, RemoteError> {
        let mut url = self.session_url("api/patients").await?;
        url.query_pairs_mut()
            .append_pair("query", query)
            .append_pair("limit", &limit.to_string())
            .append_pair("offset", &offset.to_string());
        self.get_json(url).await
    }

    pub async fn list_queue_studies(
        &self,
        query: &str,
        modality: Option<&str>,
        body_part: Option<&str>,
        report_status: Option<&str>,
        institution: Option<&str>,
        date_from: Option<&str>,
        date_to: Option<&str>,
        sort: &str,
        order: &str,
        limit: u32,
        offset: u64,
    ) -> Result<Vec<QueueStudyRow>, RemoteError> {
        let mut url = self.session_url("api/queue/studies").await?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs
                .append_pair("query", query)
                .append_pair("sort", sort)
                .append_pair("order", order)
                .append_pair("limit", &limit.to_string())
                .append_pair("offset", &offset.to_string());
            for (key, value) in [
                ("modality", modality),
                ("body_part", body_part),
                ("report_status", report_status),
                ("institution", institution),
                ("date_from", date_from),
                ("date_to", date_to),
            ] {
                if let Some(value) = value.filter(|value| !value.is_empty()) {
                    pairs.append_pair(key, value);
                }
            }
        }
        self.get_json(url).await
    }

    pub async fn list_patient_studies(
        &self,
        patient_id: i64,
    ) -> Result<Vec<StudySummary>, RemoteError> {
        let url = self
            .session_url(&format!("api/patients/{patient_id}/studies"))
            .await?;
        self.get_json(url).await
    }

    pub async fn list_study_series(
        &self,
        study_uid: &str,
    ) -> Result<Vec<SeriesSummary>, RemoteError> {
        validate_uid(study_uid)?;
        let url = self
            .session_url(&format!("api/studies/{study_uid}/series"))
            .await?;
        self.get_json(url).await
    }

    pub async fn list_shared_annotations(
        &self,
        study_uid: &str,
        series_uid: &str,
        since: Option<&str>,
    ) -> Result<serde_json::Value, RemoteError> {
        validate_uid(study_uid)?;
        validate_uid(series_uid)?;
        let mut url = self
            .session_url(&format!(
                "api/studies/{study_uid}/series/{series_uid}/annotations"
            ))
            .await?;
        if let Some(since) = since {
            url.query_pairs_mut().append_pair("since", since);
        }
        self.get_json(url).await
    }

    pub async fn create_shared_annotation(
        &self,
        study_uid: &str,
        series_uid: &str,
        annotation: serde_json::Value,
    ) -> Result<serde_json::Value, RemoteError> {
        validate_uid(study_uid)?;
        validate_uid(series_uid)?;
        let url = self
            .session_url(&format!(
                "api/studies/{study_uid}/series/{series_uid}/annotations"
            ))
            .await?;
        self.authorized_json(Method::POST, url, Some(annotation))
            .await
    }

    pub async fn update_shared_annotation(
        &self,
        study_uid: &str,
        series_uid: &str,
        annotation_id: &str,
        expected_revision: i64,
        geometry: serde_json::Value,
        deleted: bool,
    ) -> Result<serde_json::Value, RemoteError> {
        validate_uid(study_uid)?;
        validate_uid(series_uid)?;
        uuid::Uuid::parse_str(annotation_id)
            .map_err(|_| RemoteError::InvalidResponse("标注 ID 不是有效 UUID".to_owned()))?;
        let url = self
            .session_url(&format!(
                "api/studies/{study_uid}/series/{series_uid}/annotations/{annotation_id}"
            ))
            .await?;
        self.authorized_json(
            Method::PATCH,
            url,
            Some(serde_json::json!({
                "expected_revision": expected_revision,
                "geometry": geometry,
                "deleted": deleted,
            })),
        )
        .await
    }

    pub async fn list_segmentation_projects(
        &self,
        study_uid: &str,
        series_uid: &str,
    ) -> Result<serde_json::Value, RemoteError> {
        validate_uid(study_uid)?;
        validate_uid(series_uid)?;
        let url = self
            .session_url(&format!(
                "api/studies/{study_uid}/series/{series_uid}/segmentations"
            ))
            .await?;
        self.get_json(url).await
    }

    pub async fn create_segmentation_project(
        &self,
        study_uid: &str,
        series_uid: &str,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, RemoteError> {
        validate_uid(study_uid)?;
        validate_uid(series_uid)?;
        let url = self
            .session_url(&format!(
                "api/studies/{study_uid}/series/{series_uid}/segmentations"
            ))
            .await?;
        self.authorized_json(Method::POST, url, Some(input)).await
    }

    pub async fn delete_segmentation_project(
        &self,
        study_uid: &str,
        series_uid: &str,
        project_id: &str,
    ) -> Result<(), RemoteError> {
        validate_uid(study_uid)?;
        validate_uid(series_uid)?;
        uuid::Uuid::parse_str(project_id)
            .map_err(|_| RemoteError::InvalidResponse("分割项目 ID 无效".to_owned()))?;
        let url = self
            .session_url(&format!(
                "api/studies/{study_uid}/series/{series_uid}/segmentations/{project_id}"
            ))
            .await?;
        self.authorized_request(Method::DELETE, url, None).await?;
        Ok(())
    }

    pub async fn list_segmentation_segments(
        &self,
        study_uid: &str,
        series_uid: &str,
        project_id: &str,
        tag: Option<&str>,
    ) -> Result<serde_json::Value, RemoteError> {
        validate_uid(study_uid)?;
        validate_uid(series_uid)?;
        uuid::Uuid::parse_str(project_id)
            .map_err(|_| RemoteError::InvalidResponse("分割项目 ID 无效".to_owned()))?;
        let mut url = self
            .session_url(&format!(
                "api/studies/{study_uid}/series/{series_uid}/segmentations/{project_id}/segments"
            ))
            .await?;
        if let Some(tag) = tag.filter(|tag| !tag.trim().is_empty()) {
            url.query_pairs_mut().append_pair("tag", tag.trim());
        }
        self.get_json(url).await
    }

    pub async fn update_segmentation_segment_tags(
        &self,
        study_uid: &str,
        series_uid: &str,
        project_id: &str,
        segment_id: &str,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, RemoteError> {
        validate_uid(study_uid)?;
        validate_uid(series_uid)?;
        uuid::Uuid::parse_str(project_id)
            .map_err(|_| RemoteError::InvalidResponse("分割项目 ID 无效".to_owned()))?;
        uuid::Uuid::parse_str(segment_id)
            .map_err(|_| RemoteError::InvalidResponse("Segment ID 无效".to_owned()))?;
        let url = self
            .session_url(&format!(
                "api/studies/{study_uid}/series/{series_uid}/segmentations/{project_id}/segments/{segment_id}"
            ))
            .await?;
        self.authorized_json(Method::PATCH, url, Some(input)).await
    }

    pub async fn list_segmentation_masks(
        &self,
        study_uid: &str,
        series_uid: &str,
        project_id: &str,
        sop_instance_uid: &str,
        frame_number: i32,
    ) -> Result<serde_json::Value, RemoteError> {
        validate_uid(study_uid)?;
        validate_uid(series_uid)?;
        validate_uid(sop_instance_uid)?;
        uuid::Uuid::parse_str(project_id)
            .map_err(|_| RemoteError::InvalidResponse("分割项目 ID 无效".to_owned()))?;
        let mut url = self
            .session_url(&format!(
                "api/studies/{study_uid}/series/{series_uid}/segmentations/{project_id}/masks"
            ))
            .await?;
        url.query_pairs_mut()
            .append_pair("sop_instance_uid", sop_instance_uid)
            .append_pair("frame_number", &frame_number.to_string());
        self.get_json(url).await
    }

    pub async fn upsert_segmentation_mask(
        &self,
        study_uid: &str,
        series_uid: &str,
        project_id: &str,
        segment_id: &str,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, RemoteError> {
        validate_uid(study_uid)?;
        validate_uid(series_uid)?;
        uuid::Uuid::parse_str(project_id)
            .map_err(|_| RemoteError::InvalidResponse("分割项目 ID 无效".to_owned()))?;
        uuid::Uuid::parse_str(segment_id)
            .map_err(|_| RemoteError::InvalidResponse("Segment ID 无效".to_owned()))?;
        let url = self
            .session_url(&format!(
                "api/studies/{study_uid}/series/{series_uid}/segmentations/{project_id}/segments/{segment_id}/mask"
            ))
            .await?;
        self.authorized_json(Method::PUT, url, Some(input)).await
    }

    pub async fn list_segmentation_volume(
        &self,
        study_uid: &str,
        series_uid: &str,
        project_id: &str,
        segment_id: &str,
    ) -> Result<serde_json::Value, RemoteError> {
        validate_uid(study_uid)?;
        validate_uid(series_uid)?;
        uuid::Uuid::parse_str(project_id)
            .map_err(|_| RemoteError::InvalidResponse("分割项目 ID 无效".to_owned()))?;
        uuid::Uuid::parse_str(segment_id)
            .map_err(|_| RemoteError::InvalidResponse("Segment ID 无效".to_owned()))?;
        let url = self
            .session_url(&format!(
                "api/studies/{study_uid}/series/{series_uid}/segmentations/{project_id}/segments/{segment_id}/masks"
            ))
            .await?;
        self.get_json(url).await
    }

    pub async fn upsert_segmentation_masks(
        &self,
        study_uid: &str,
        series_uid: &str,
        project_id: &str,
        segment_id: &str,
        updates: serde_json::Value,
    ) -> Result<serde_json::Value, RemoteError> {
        validate_uid(study_uid)?;
        validate_uid(series_uid)?;
        uuid::Uuid::parse_str(project_id)
            .map_err(|_| RemoteError::InvalidResponse("分割项目 ID 无效".to_owned()))?;
        uuid::Uuid::parse_str(segment_id)
            .map_err(|_| RemoteError::InvalidResponse("Segment ID 无效".to_owned()))?;
        let url = self
            .session_url(&format!(
                "api/studies/{study_uid}/series/{series_uid}/segmentations/{project_id}/segments/{segment_id}/masks"
            ))
            .await?;
        self.authorized_json(
            Method::PUT,
            url,
            Some(serde_json::json!({ "updates": updates })),
        )
        .await
    }

    pub async fn transform_schema(&self) -> Result<serde_json::Value, RemoteError> {
        let url = self.session_url("api/dicom/schema").await?;
        self.get_json(url).await
    }

    pub async fn preview_clinical_transform(
        &self,
        target_type: &str,
        target_key: &str,
        rules: serde_json::Value,
        reason: &str,
    ) -> Result<serde_json::Value, RemoteError> {
        let url = self
            .session_url("api/dicom/transformations/preview")
            .await?;
        self.authorized_json(
            Method::POST,
            url,
            Some(serde_json::json!({
                "mode": "clinical_correction",
                "target": { "target_type": target_type, "key": target_key },
                "rules": rules,
                "reason": reason
            })),
        )
        .await
    }

    pub async fn confirm_transform(
        &self,
        job_id: &str,
        confirmation_token: &str,
    ) -> Result<serde_json::Value, RemoteError> {
        let url = self.session_url("api/dicom/transformations").await?;
        self.authorized_json(
            Method::POST,
            url,
            Some(serde_json::json!({
                "job_id": job_id,
                "confirmation_token": confirmation_token
            })),
        )
        .await
    }

    pub async fn transform_jobs(&self) -> Result<serde_json::Value, RemoteError> {
        let url = self.session_url("api/dicom/transformations").await?;
        self.get_json(url).await
    }

    pub async fn instance_revisions_by_sop(
        &self,
        sop_uid: &str,
    ) -> Result<serde_json::Value, RemoteError> {
        validate_uid(sop_uid)?;
        let url = self
            .session_url(&format!("api/dicom/instances/by-sop/{sop_uid}/revisions"))
            .await?;
        self.get_json(url).await
    }

    pub async fn preview_rollback(
        &self,
        logical_id: &str,
        version_id: i64,
        reason: &str,
    ) -> Result<serde_json::Value, RemoteError> {
        let url = self
            .session_url(&format!("api/dicom/instances/{logical_id}/rollback"))
            .await?;
        self.authorized_json(
            Method::POST,
            url,
            Some(serde_json::json!({
                "version_id": version_id,
                "reason": reason
            })),
        )
        .await
    }

    pub async fn list_instance_uids(
        &self,
        study_uid: &str,
        series_uid: &str,
    ) -> Result<Vec<String>, RemoteError> {
        validate_uid(study_uid)?;
        validate_uid(series_uid)?;
        let mut url = self
            .session_url(&format!(
                "dicomweb/studies/{study_uid}/series/{series_uid}/instances"
            ))
            .await?;
        url.query_pairs_mut().append_pair("limit", "10000");
        let response = self.authorized_get(url).await?;
        if response.status() == StatusCode::NO_CONTENT {
            return Ok(Vec::new());
        }
        let value: serde_json::Value = response
            .json()
            .await
            .map_err(|error| RemoteError::InvalidResponse(error.to_string()))?;
        let entries = value
            .as_array()
            .ok_or_else(|| RemoteError::InvalidResponse("实例查询响应不是数组".to_owned()))?;
        entries
            .iter()
            .map(|entry| {
                entry
                    .get("00080018")
                    .and_then(|element| element.get("Value"))
                    .and_then(|values| values.get(0))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        RemoteError::InvalidResponse("实例查询响应缺少 SOPInstanceUID".to_owned())
                    })
            })
            .collect()
    }

    pub async fn download_instance(
        &self,
        study_uid: &str,
        series_uid: &str,
        sop_uid: &str,
    ) -> Result<Vec<u8>, RemoteError> {
        validate_uid(sop_uid)?;
        let url = self
            .session_url(&format!(
                "dicomweb/studies/{study_uid}/series/{series_uid}/instances/{sop_uid}"
            ))
            .await?;
        let response = self.authorized_get(url).await?;
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(request_error)
    }

    pub async fn create_export(
        &self,
        study: &str,
        series: Option<&str>,
    ) -> Result<Uuid, RemoteError> {
        let url = self.session_url("api/v1/exports").await?;
        let value: serde_json::Value = self
            .authorized_json(
                Method::POST,
                url,
                Some(
                    serde_json::json!({"study_instance_uid": study, "series_instance_uid": series}),
                ),
            )
            .await?;
        job_id(&value)
    }

    pub async fn transfer_status(
        &self,
        kind: &str,
        job: Uuid,
    ) -> Result<serde_json::Value, RemoteError> {
        let url = self.session_url(&format!("api/v1/{kind}/{job}")).await?;
        self.get_json(url).await
    }

    pub async fn download_export(&self, job: Uuid) -> Result<Vec<u8>, RemoteError> {
        let url = self
            .session_url(&format!("api/v1/exports/{job}/download"))
            .await?;
        let response = self.authorized_get(url).await?;
        response
            .bytes()
            .await
            .map(|v| v.to_vec())
            .map_err(request_error)
    }

    pub async fn router_get(&self, path: &str) -> Result<serde_json::Value, RemoteError> {
        let url = self.session_url(&format!("api/v1/router/{path}")).await?;
        self.get_json(url).await
    }

    pub async fn router_write(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, RemoteError> {
        let url = self.session_url(&format!("api/v1/router/{path}")).await?;
        self.authorized_json(method, url, body).await
    }

    pub async fn router_delete(&self, path: &str) -> Result<(), RemoteError> {
        let url = self.session_url(&format!("api/v1/router/{path}")).await?;
        self.authorized_request(Method::DELETE, url, None).await?;
        Ok(())
    }

    pub async fn lifecycle_get(&self, path: &str) -> Result<serde_json::Value, RemoteError> {
        let url = self
            .session_url(&format!("api/v1/lifecycle/{path}"))
            .await?;
        self.get_json(url).await
    }

    pub async fn lifecycle_write(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, RemoteError> {
        let url = self
            .session_url(&format!("api/v1/lifecycle/{path}"))
            .await?;
        self.authorized_json(method, url, body).await
    }

    pub async fn lifecycle_delete(&self, path: &str) -> Result<(), RemoteError> {
        let url = self
            .session_url(&format!("api/v1/lifecycle/{path}"))
            .await?;
        self.authorized_request(Method::DELETE, url, None).await?;
        Ok(())
    }

    pub async fn begin_transfer(&self, job: Uuid) {
        *self.transfer_job.lock().await = Some(job);
    }
    pub async fn end_transfer(&self) {
        *self.transfer_job.lock().await = None;
    }
    pub async fn cancel_transfer(&self, kind: &str) -> Result<(), RemoteError> {
        let Some(job) = *self.transfer_job.lock().await else {
            return Ok(());
        };
        let url = self.session_url(&format!("api/v1/{kind}/{job}")).await?;
        let _: serde_json::Value = self.authorized_json(Method::DELETE, url, None).await?;
        Ok(())
    }

    pub fn begin_download(&self) -> Result<DownloadGuard, RemoteError> {
        self.download_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| RemoteError::DownloadInProgress)?;
        self.download_cancelled.store(false, Ordering::Release);
        Ok(DownloadGuard(self.clone()))
    }

    pub fn cancel_download(&self) {
        self.download_cancelled.store(true, Ordering::Release);
    }

    pub fn check_cancelled(&self) -> Result<(), RemoteError> {
        if self.download_cancelled.load(Ordering::Acquire) {
            Err(RemoteError::Cancelled)
        } else {
            Ok(())
        }
    }

    async fn session_url(&self, path: &str) -> Result<Url, RemoteError> {
        let guard = self.session.lock().await;
        let session = guard.as_ref().ok_or(RemoteError::NotLoggedIn)?;
        endpoint(&session.base_url, path)
    }

    async fn get_json<T: DeserializeOwned>(&self, url: Url) -> Result<T, RemoteError> {
        let response = self.authorized_get(url).await?;
        response
            .json()
            .await
            .map_err(|error| RemoteError::InvalidResponse(error.to_string()))
    }

    async fn authorized_json<T: DeserializeOwned>(
        &self,
        method: Method,
        url: Url,
        body: Option<serde_json::Value>,
    ) -> Result<T, RemoteError> {
        let response = self.authorized_request(method, url, body).await?;
        response
            .json()
            .await
            .map_err(|error| RemoteError::InvalidResponse(error.to_string()))
    }

    async fn authorized_get(&self, url: Url) -> Result<Response, RemoteError> {
        self.authorized_request(Method::GET, url, None).await
    }

    async fn authorized_request(
        &self,
        method: Method,
        url: Url,
        body: Option<serde_json::Value>,
    ) -> Result<Response, RemoteError> {
        let mut guard = self.session.lock().await;
        let session = guard.as_mut().ok_or(RemoteError::NotLoggedIn)?;
        let send = |session: &Session| {
            let mut request = session
                .client
                .request(method.clone(), url.clone())
                .bearer_auth(&session.access_token);
            if let Some(body) = body.clone() {
                request = request.json(&body);
            }
            request.send()
        };
        let mut response = send(session).await.map_err(request_error)?;
        if response.status() == StatusCode::UNAUTHORIZED {
            if let Err(error) = refresh(session).await {
                *guard = None;
                return Err(error);
            }
            response = send(session).await.map_err(request_error)?;
            if response.status() == StatusCode::UNAUTHORIZED {
                *guard = None;
                return Err(RemoteError::NotLoggedIn);
            }
        }
        ensure_success(response).await
    }
}

fn job_id(value: &serde_json::Value) -> Result<Uuid, RemoteError> {
    value
        .pointer("/job/id")
        .and_then(serde_json::Value::as_str)
        .and_then(|v| Uuid::parse_str(v).ok())
        .ok_or_else(|| RemoteError::InvalidResponse("响应缺少任务 ID".to_owned()))
}

pub struct DownloadGuard(RemoteState);

impl Drop for DownloadGuard {
    fn drop(&mut self) {
        self.0.download_active.store(false, Ordering::Release);
        self.0.download_cancelled.store(false, Ordering::Release);
    }
}

async fn refresh(session: &mut Session) -> Result<(), RemoteError> {
    let response = session
        .client
        .post(endpoint(&session.base_url, "auth/refresh")?)
        .json(&serde_json::json!({ "refresh_token": session.refresh_token }))
        .send()
        .await
        .map_err(request_error)?;
    let response = ensure_success(response).await?;
    let refreshed: RefreshResponse = response
        .json()
        .await
        .map_err(|error| RemoteError::InvalidResponse(error.to_string()))?;
    session.access_token = refreshed.access_token;
    session.refresh_token = refreshed.refresh_token;
    Ok(())
}

fn normalized_base_url(raw: &str) -> Result<Url, RemoteError> {
    let mut url =
        Url::parse(raw.trim()).map_err(|error| RemoteError::InvalidUrl(error.to_string()))?;
    if url.scheme() != "https" {
        return Err(RemoteError::HttpsRequired);
    }
    if url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() {
        return Err(RemoteError::InvalidUrl(
            "地址必须包含主机且不能包含凭据".to_owned(),
        ));
    }
    url.set_query(None);
    url.set_fragment(None);
    url.set_path("/");
    Ok(url)
}

fn client_for(server_url: &str, ca_cert_path: &Path) -> Result<(Client, Url), RemoteError> {
    let base_url = normalized_base_url(server_url)?;
    let certificate_bytes =
        std::fs::read(ca_cert_path).map_err(|source| RemoteError::CertificateIo {
            path: ca_cert_path.to_owned(),
            source,
        })?;
    let certificate = reqwest::Certificate::from_pem(&certificate_bytes)
        .map_err(|error| RemoteError::InvalidCertificate(error.to_string()))?;
    let client = Client::builder()
        .https_only(true)
        .add_root_certificate(certificate)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|error| RemoteError::Request(error.to_string()))?;
    Ok((client, base_url))
}

fn endpoint(base: &Url, path: &str) -> Result<Url, RemoteError> {
    base.join(path)
        .map_err(|error| RemoteError::InvalidUrl(error.to_string()))
}

fn validate_uid(uid: &str) -> Result<(), RemoteError> {
    if uid.is_empty()
        || uid.len() > 64
        || uid.starts_with('.')
        || uid.ends_with('.')
        || uid.split('.').any(|part| part.is_empty())
        || !uid
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return Err(RemoteError::InvalidResponse(
            "服务端返回了无效的 DICOM UID".to_owned(),
        ));
    }
    Ok(())
}

fn request_error(error: reqwest::Error) -> RemoteError {
    RemoteError::Request(error.to_string())
}

async fn ensure_success(response: Response) -> Result<Response, RemoteError> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    let message = serde_json::from_str::<ApiErrorBody>(&text)
        .map(|body| body.error)
        .unwrap_or_else(|_| {
            if text.trim().is_empty() {
                "请求失败".to_owned()
            } else {
                text
            }
        });
    Err(RemoteError::Http { status, message })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_server_urls_are_accepted() {
        assert!(matches!(
            normalized_base_url("http://127.0.0.1:8443"),
            Err(RemoteError::HttpsRequired)
        ));
        let url = normalized_base_url("https://127.0.0.1:8443/something?ignored=1").unwrap();
        assert_eq!(url.as_str(), "https://127.0.0.1:8443/");
    }

    #[test]
    fn dicom_uids_are_validated_before_building_paths() {
        assert!(validate_uid("1.2.840.10008.1.1").is_ok());
        for invalid in ["", "..", "1.2/3", ".1.2", "1.2.", "1..2"] {
            assert!(validate_uid(invalid).is_err(), "应拒绝 {invalid:?}");
        }
    }
}
