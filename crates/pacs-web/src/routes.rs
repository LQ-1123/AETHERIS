//! QIDO-RS 路由与响应编码。

use std::sync::Arc;

use axum::extract::{Extension, Path, Query as UrlQuery, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use dicom::core::{DataElement, PrimitiveValue, VR};
use dicom::dictionary_std::tags;
use dicom::object::InMemDicomObject;
use pacs_auth::{AuthService, Identity, Permission};
use pacs_core::Uid;
use pacs_core::query::QueryLevel;
use sqlx::PgPool;

use crate::qido::{self, QidoError};

/// DICOMweb 处理器共享的状态。
#[derive(Clone)]
pub struct WebState {
    pub pool: PgPool,
    /// 单次查询的结果上限。调用方给的 `limit` 会被压到这个值以内。
    ///
    /// 存在的理由和 `pacs_db::DEFAULT_LIMIT` 一样:不设上限的话,
    /// 一个无条件查询会把整个库拉进内存。
    pub max_results: usize,
    /// 影像存储。WADO-RS 需要,QIDO-RS 不需要。
    ///
    /// 做成 `Option` 是为了让只挂查询接口的部署(比如把取回放到另一个进程)
    /// 不必构造 `Store`。为 `None` 时取回接口回 500 并告警,
    /// 而不是静默返回 404 —— 后者会让"存储没配"看起来像"影像不存在"。
    pub store: Option<Arc<pacs_store::Store>>,
}

impl WebState {
    /// 只挂查询接口。
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            max_results: pacs_db::DEFAULT_LIMIT,
            store: None,
        }
    }

    /// 查询 + 取回。
    pub fn with_store(pool: PgPool, store: Arc<pacs_store::Store>) -> Self {
        Self {
            pool,
            max_results: pacs_db::DEFAULT_LIMIT,
            store: Some(store),
        }
    }
}

/// 挂载 DICOMweb 路由,整棵子树要求 `ViewImages` 权限。
///
/// 权限挂在路由树上而不是逐个 handler 写 —— 新增路由默认继承保护,
/// 漏写不会变成默认放行(见 `pacs_auth::middleware` 的模块文档)。
pub fn dicomweb_routes(state: WebState, auth: Arc<AuthService>) -> Router {
    Router::new()
        // —— QIDO-RS ——
        .route("/studies", get(search_studies))
        .route("/studies/{study_uid}/series", get(search_series))
        .route(
            "/studies/{study_uid}/series/{series_uid}/instances",
            get(search_instances),
        )
        // —— WADO-RS ——
        .route(
            "/studies/{study_uid}/series/{series_uid}/instances/{sop_uid}",
            get(crate::wado::retrieve_instance),
        )
        .route(
            "/studies/{study_uid}/series/{series_uid}/instances/{sop_uid}/metadata",
            get(crate::wado::retrieve_metadata),
        )
        .route(
            "/studies/{study_uid}/series/{series_uid}/instances/{sop_uid}/frames/{frames}",
            get(crate::wado::retrieve_frames),
        )
        // 先 with_state 再 layer:反过来会让状态类型在中间件那层被擦掉
        .with_state(state)
        .layer(axum::middleware::from_fn(move |request, next| {
            let auth = Arc::clone(&auth);
            async move { pacs_auth::require(auth, Permission::ViewImages, request, next).await }
        }))
}

/// `GET /studies`
async fn search_studies(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    UrlQuery(params): UrlQuery<Vec<(String, String)>>,
) -> Result<Response, ApiError> {
    execute(
        &state,
        identity.institution_id,
        QueryLevel::Study,
        params,
        &[],
    )
    .await
}

/// `GET /studies/{study_uid}/series`
async fn search_series(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(study_uid): Path<String>,
    UrlQuery(params): UrlQuery<Vec<(String, String)>>,
) -> Result<Response, ApiError> {
    let study = validated_uid(&study_uid, "StudyInstanceUID")?;
    execute(
        &state,
        identity.institution_id,
        QueryLevel::Series,
        params,
        &[(tags::STUDY_INSTANCE_UID, study)],
    )
    .await
}

/// `GET /studies/{study_uid}/series/{series_uid}/instances`
async fn search_instances(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path((study_uid, series_uid)): Path<(String, String)>,
    UrlQuery(params): UrlQuery<Vec<(String, String)>>,
) -> Result<Response, ApiError> {
    let study = validated_uid(&study_uid, "StudyInstanceUID")?;
    let series = validated_uid(&series_uid, "SeriesInstanceUID")?;
    execute(
        &state,
        identity.institution_id,
        QueryLevel::Image,
        params,
        &[
            (tags::STUDY_INSTANCE_UID, study),
            (tags::SERIES_INSTANCE_UID, series),
        ],
    )
    .await
}

/// 路径里的 UID 走和 C-STORE 落盘同一套校验。
///
/// UID 会进 SQL 参数(不是拼接,所以没有注入风险),但非法 UID 必然查不到东西 ——
/// 提前回 400 比让调用方对着空结果猜要好。校验规则复用 [`Uid`],
/// 和存储层保持一致。
fn validated_uid(raw: &str, field: &'static str) -> Result<Uid, ApiError> {
    Uid::parse(raw).map_err(|source| ApiError::InvalidUid { field, source })
}

/// 三条路由的公共执行路径。
///
/// `path_constraints` 是 URL 路径里的 UID —— 它们**覆盖**同名查询参数:
/// 路径是资源标识,查询参数是过滤条件,两者冲突时路径说了算。
/// 若把两个条件并存(`AND`),`/studies/A/series?StudyInstanceUID=B` 会变成
/// 永远查不到的矛盾条件,而调用方收到空结果却看不出哪里错了。
async fn execute(
    state: &WebState,
    institution_id: i64,
    level: QueryLevel,
    params: Vec<(String, String)>,
    path_constraints: &[(dicom::core::Tag, Uid)],
) -> Result<Response, ApiError> {
    let request = qido::parse(level, &params)?;

    // 路径 UID 覆盖同名查询参数
    let mut query = request.query;
    for (tag, uid) in path_constraints {
        query.keys.insert(
            *tag,
            pacs_core::query::MatchKey::Single(uid.as_str().to_owned()),
        );
    }

    // `limit` 和 `find` 的上限是两件不同的事,不能混:
    //   * `find` 的上限是**安全阀** —— 超了报错,防止无条件查询把整库拉进内存;
    //   * QIDO-RS 的 `limit` 是**分页** —— 意思是"最多给我这么多",超出的部分
    //     应当被截掉,不是让请求失败。
    // 把 `limit` 传给 `find` 当上限的话,`?limit=2` 在有三条结果时会回 413,
    // 分页就彻底失效了。
    let results =
        pacs_db::find_for_institution(&state.pool, &query, state.max_results, institution_id)
            .await
            .map_err(|error| match error {
                pacs_db::DbError::TooManyResults { limit } => ApiError::TooManyResults { limit },
                other => {
                    tracing::error!(%other, "QIDO-RS 查询失败");
                    ApiError::Internal
                }
            })?;

    // 分页在本地切。`find` 没有 offset,而加上它会改动阶段 4 已验收的接口;
    // 结果集已被 max_results 封顶,内存是有界的。真到了这个切法成为瓶颈的时候
    // (单层级几千条以上),再把 LIMIT/OFFSET 下推到 SQL。
    let effective_limit = request.limit.unwrap_or(state.max_results);

    let page: Vec<&InMemDicomObject> = results
        .identifiers
        .iter()
        .skip(request.offset)
        .take(effective_limit)
        .collect();

    // 空结果回 204:调用方不必解析响应体就知道没有命中(PS3.18 §6.7.1.2)
    if page.is_empty() {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }

    let body = encode_json(&page);
    let mut response = (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/dicom+json")],
        body,
    )
        .into_response();

    // 未支持的参数和匹配键都通过 Warning 头告知 —— 静默忽略会让调用方
    // 以为过滤生效了,而实际返回得更多。
    let mut warnings: Vec<String> = request.unsupported_params.clone();
    warnings.extend(
        results
            .unsupported_keys
            .iter()
            .map(|tag| format!("{tag:04X?}")),
    );
    if !warnings.is_empty() {
        tracing::info!(?warnings, level = level.as_str(), "QIDO-RS 有未支持的键");
        if let Ok(value) = header::HeaderValue::from_str(&format!(
            "299 - \"以下参数或匹配键未被支持,已忽略: {}\"",
            warnings.join(", ")
        )) {
            response.headers_mut().insert(header::WARNING, value);
        }
    }

    Ok(response)
}

/// 编成 DICOM JSON Model(PS3.18 附录 F)的数组。
fn encode_json(identifiers: &[&InMemDicomObject]) -> String {
    let values: Vec<serde_json::Value> = identifiers
        .iter()
        .map(|object| {
            // QueryRetrieveLevel 是 DIMSE 的概念,QIDO-RS 的响应里不该出现:
            // 层级已经由 URL 路径表达了。
            let filtered: Vec<DataElement<_>> = object
                .iter()
                .filter(|element| element.header().tag != tags::QUERY_RETRIEVE_LEVEL)
                .cloned()
                .collect();
            let trimmed = InMemDicomObject::from_element_iter(filtered);
            dicom_json::to_value(trimmed).unwrap_or_else(|error| {
                tracing::error!(%error, "响应序列化失败,该条回空对象");
                serde_json::Value::Object(serde_json::Map::new())
            })
        })
        .collect();
    serde_json::Value::Array(values).to_string()
}

/// 让未使用的导入保持有意义:构造零长元素时会用到。
#[allow(dead_code)]
fn empty_element(tag: dicom::core::Tag, vr: VR) -> DataElement<InMemDicomObject> {
    DataElement::new(tag, vr, PrimitiveValue::Empty)
}

#[derive(Debug, thiserror::Error)]
enum ApiError {
    #[error("{field} 不是合法 UID:{source}")]
    InvalidUid {
        field: &'static str,
        #[source]
        source: pacs_core::UidError,
    },
    #[error(transparent)]
    Qido(#[from] QidoError),
    #[error("结果超过 {limit} 条")]
    TooManyResults { limit: usize },
    #[error("内部错误")]
    Internal,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            Self::InvalidUid { .. } | Self::Qido(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            // 413 而不是 400:请求本身是合法的,只是结果集太大。
            // 消息里给出上限,调用方才知道该怎么收窄。
            Self::TooManyResults { limit } => (
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("结果超过 {limit} 条,请用 limit 分页或收窄查询条件"),
            ),
            // 内部错误不回细节 —— 数据库结构、SQL 片段都可能藏在错误里。
            // 完整信息已在上游记进日志。
            Self::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "内部错误".to_owned()),
        };
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 路径里的 UID 要挡掉路径穿越 —— 和存储层同一套规则。
    #[test]
    fn path_uids_reject_traversal_attempts() {
        for bad in ["..", ".", "../../etc/passwd", "1.2/../3", ""] {
            assert!(
                validated_uid(bad, "StudyInstanceUID").is_err(),
                "应拒绝 {bad:?}"
            );
        }
        assert!(validated_uid("1.2.840.10008.1.1", "StudyInstanceUID").is_ok());
    }

    #[test]
    fn empty_result_set_encodes_as_an_empty_array() {
        assert_eq!(encode_json(&[]), "[]");
    }

    /// 响应里不该出现 QueryRetrieveLevel —— 那是 DIMSE 的概念。
    #[test]
    fn query_retrieve_level_is_stripped_from_responses() {
        let object = InMemDicomObject::from_element_iter([
            DataElement::new(
                tags::QUERY_RETRIEVE_LEVEL,
                VR::CS,
                PrimitiveValue::from("STUDY"),
            ),
            DataElement::new(
                tags::STUDY_INSTANCE_UID,
                VR::UI,
                PrimitiveValue::from("1.2.3"),
            ),
        ]);
        let json = encode_json(&[&object]);
        assert!(
            !json.contains("00080052"),
            "不该含 QueryRetrieveLevel:{json}"
        );
        assert!(json.contains("0020000D"), "应含 StudyInstanceUID:{json}");
    }
}
