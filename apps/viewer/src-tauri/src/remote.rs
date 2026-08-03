//! PACS HTTPS 会话、工作列表和远程序列下载。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use reqwest::{Client, Response, StatusCode, Url};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct RemoteState {
    session: Arc<Mutex<Option<Session>>>,
    download_active: Arc<AtomicBool>,
    download_cancelled: Arc<AtomicBool>,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatientSummary {
    pub id: i64,
    pub patient_id: String,
    pub name: Option<String>,
    pub birth_date: Option<String>,
    pub sex: Option<String>,
    pub study_count: i64,
    pub series_count: i64,
    pub instance_count: i64,
    pub latest_study_date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudySummary {
    pub study_uid: String,
    pub study_date: Option<String>,
    pub study_time: Option<String>,
    pub accession_number: Option<String>,
    pub description: Option<String>,
    pub modalities: Vec<String>,
    pub series_count: i32,
    pub instance_count: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesSummary {
    pub series_uid: String,
    pub series_number: Option<i32>,
    pub modality: Option<String>,
    pub description: Option<String>,
    pub body_part_examined: Option<String>,
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
        }
    }

    pub async fn login(
        &self,
        server_url: &str,
        ca_cert_path: &Path,
        username: &str,
        password: &str,
    ) -> Result<RemoteUser, RemoteError> {
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

    async fn authorized_get(&self, url: Url) -> Result<Response, RemoteError> {
        let mut guard = self.session.lock().await;
        let session = guard.as_mut().ok_or(RemoteError::NotLoggedIn)?;
        let mut response = session
            .client
            .get(url.clone())
            .bearer_auth(&session.access_token)
            .send()
            .await
            .map_err(request_error)?;
        if response.status() == StatusCode::UNAUTHORIZED {
            if let Err(error) = refresh(session).await {
                *guard = None;
                return Err(error);
            }
            response = session
                .client
                .get(url)
                .bearer_auth(&session.access_token)
                .send()
                .await
                .map_err(request_error)?;
            if response.status() == StatusCode::UNAUTHORIZED {
                *guard = None;
                return Err(RemoteError::NotLoggedIn);
            }
        }
        ensure_success(response).await
    }
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
