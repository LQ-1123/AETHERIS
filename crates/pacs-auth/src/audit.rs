//! 审计日志。
//!
//! 医疗合规的硬要求:谁在何时访问了哪个病人/检查。写数据库而不是只写文件 ——
//! 文件会轮转、会被删,也不方便按病人或时间段查询。
//!
//! # 失败时怎么办
//!
//! 审计写入失败**不阻断**业务操作,但一定会打错误日志。理由是取舍:
//! 让医生因为审计表写不进去而看不了片,风险高于少一条审计记录。
//! 但这个取舍必须是显式的、可见的 —— 所以失败路径上有 `tracing::error!`,
//! 而不是悄悄 `let _ =` 掉。

use serde_json::Value;
use sqlx::PgPool;

use crate::model::Role;

/// 审计动作。用固定枚举而不是自由字符串:
/// 合规检查要按动作类型统计,拼写不一致会让统计漏项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Login,
    Logout,
    TokenRefresh,
    PasswordChange,
    UserCreated,
    UserModified,
    UserDeactivated,
    /// 查询影像(QIDO-RS / C-FIND)。
    QueryImages,
    /// 取回影像(WADO-RS / C-MOVE / C-GET)。
    RetrieveImages,
    /// 上传影像(STOW-RS / C-STORE)。
    StoreImages,
    DeleteImages,
    ViewAuditLog,
    AnnotationCreated,
    AnnotationUpdated,
    AnnotationDeleted,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::Logout => "logout",
            Self::TokenRefresh => "token_refresh",
            Self::PasswordChange => "password_change",
            Self::UserCreated => "user_created",
            Self::UserModified => "user_modified",
            Self::UserDeactivated => "user_deactivated",
            Self::QueryImages => "query_images",
            Self::RetrieveImages => "retrieve_images",
            Self::StoreImages => "store_images",
            Self::DeleteImages => "delete_images",
            Self::ViewAuditLog => "view_audit_log",
            Self::AnnotationCreated => "annotation_created",
            Self::AnnotationUpdated => "annotation_updated",
            Self::AnnotationDeleted => "annotation_deleted",
        }
    }
}

/// 动作结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Success,
    /// 尝试了但失败(密码错、服务器出错)。
    Failure,
    /// 权限不足被拒。与 Failure 分开:大量 denied 是入侵信号,
    /// 而 failure 更多是用户手滑。
    Denied,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Denied => "denied",
        }
    }
}

/// 一条待写入的审计记录。
#[derive(Debug, Clone, Default)]
pub struct Entry {
    pub user_id: Option<i64>,
    /// 用户名快照。用户被删后外键会置空,但「是谁做的」必须留下来。
    pub username: Option<String>,
    pub role: Option<Role>,
    pub patient_id: Option<String>,
    pub study_instance_uid: Option<String>,
    pub series_instance_uid: Option<String>,
    pub sop_instance_uid: Option<String>,
    pub client_ip: Option<String>,
    pub user_agent: Option<String>,
    pub detail: Option<Value>,
}

impl Entry {
    pub fn for_user(user_id: i64, username: impl Into<String>, role: Role) -> Self {
        Self {
            user_id: Some(user_id),
            username: Some(username.into()),
            role: Some(role),
            ..Self::default()
        }
    }

    /// 登录失败等场景:还不知道是哪个用户,但用户名尝试值要记下来。
    pub fn for_attempted_username(username: impl Into<String>) -> Self {
        Self {
            username: Some(username.into()),
            ..Self::default()
        }
    }

    pub fn with_study(mut self, study_instance_uid: impl Into<String>) -> Self {
        self.study_instance_uid = Some(study_instance_uid.into());
        self
    }

    pub fn with_patient(mut self, patient_id: impl Into<String>) -> Self {
        self.patient_id = Some(patient_id.into());
        self
    }

    pub fn with_detail(mut self, detail: Value) -> Self {
        self.detail = Some(detail);
        self
    }
}

/// 写一条审计记录。
///
/// 失败只记日志不返回错误 —— 见模块文档里的取舍说明。
pub async fn record(pool: &PgPool, action: Action, outcome: Outcome, entry: Entry) {
    let mut detail = entry
        .detail
        .unwrap_or_else(|| Value::Object(Default::default()));
    // 角色不单独占列:它随时间会变,记录当时的值即可,放进 detail 更合适
    if let (Some(role), Value::Object(map)) = (entry.role, &mut detail) {
        map.insert("role".to_owned(), Value::String(role.to_string()));
    }

    let result = sqlx::query(
        r#"
        INSERT INTO audit_log (
            user_fk, username, action, outcome,
            patient_id, study_instance_uid, series_instance_uid, sop_instance_uid,
            client_ip, user_agent, detail
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        "#,
    )
    .bind(entry.user_id)
    .bind(&entry.username)
    .bind(action.as_str())
    .bind(outcome.as_str())
    .bind(&entry.patient_id)
    .bind(&entry.study_instance_uid)
    .bind(&entry.series_instance_uid)
    .bind(&entry.sop_instance_uid)
    .bind(entry.client_ip)
    .bind(&entry.user_agent)
    .bind(&detail)
    .execute(pool)
    .await;

    if let Err(error) = result {
        // 不能静默:审计写不进去本身就是需要人介入的事故
        tracing::error!(
            %error,
            action = action.as_str(),
            outcome = outcome.as_str(),
            username = entry.username.as_deref().unwrap_or("-"),
            "审计日志写入失败"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// outcome 会写进数据库的 CHECK 约束,两边必须一致。
    #[test]
    fn outcome_names_match_the_database_constraint() {
        let migration = include_str!("../../pacs-db/migrations/0002_accounts.sql");
        for outcome in [Outcome::Success, Outcome::Failure, Outcome::Denied] {
            assert!(
                migration.contains(&format!("'{}'", outcome.as_str())),
                "迁移里的 CHECK 约束缺少 outcome {}",
                outcome.as_str()
            );
        }
    }

    #[test]
    fn action_names_are_distinct() {
        let actions = [
            Action::Login,
            Action::Logout,
            Action::TokenRefresh,
            Action::PasswordChange,
            Action::UserCreated,
            Action::UserModified,
            Action::UserDeactivated,
            Action::QueryImages,
            Action::RetrieveImages,
            Action::StoreImages,
            Action::DeleteImages,
            Action::ViewAuditLog,
            Action::AnnotationCreated,
            Action::AnnotationUpdated,
            Action::AnnotationDeleted,
        ];
        let unique: std::collections::HashSet<_> = actions.iter().map(|a| a.as_str()).collect();
        assert_eq!(unique.len(), actions.len(), "动作名有重复,统计会串");
    }

    #[test]
    fn entry_builders_capture_identity() {
        let entry = Entry::for_user(7, "alice", Role::Radiologist).with_study("1.2.3");
        assert_eq!(entry.user_id, Some(7));
        assert_eq!(entry.username.as_deref(), Some("alice"));
        assert_eq!(entry.study_instance_uid.as_deref(), Some("1.2.3"));

        // 登录失败时没有 user_id,但用户名尝试值要留下
        let attempt = Entry::for_attempted_username("bob");
        assert_eq!(attempt.user_id, None);
        assert_eq!(attempt.username.as_deref(), Some("bob"));
    }
}
