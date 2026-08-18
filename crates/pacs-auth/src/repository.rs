//! 账号与令牌的数据库访问。

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use thiserror::Error;

use crate::model::{PasswordResetRequest, Permission, Role, User};

#[derive(Debug, Error)]
pub enum RepoError {
    #[error("数据库操作失败")]
    Query(#[from] sqlx::Error),
    #[error("用户名 {username:?} 已存在")]
    UsernameTaken { username: String },
    #[error("数据库里的角色 {role:?} 无法识别 —— 迁移与代码不一致")]
    UnknownRole { role: String },
}

/// 新建账号所需的字段。密码哈希由调用方算好传进来 ——
/// 仓储层不碰明文密码,连经手的机会都不给。
#[derive(Debug)]
pub struct NewUser<'a> {
    pub username: &'a str,
    pub display_name: Option<&'a str>,
    pub password_hash: &'a str,
    pub role: Role,
    /// 管理员代建的账号应当置 true,强制用户首次登录时自己设密码。
    pub must_change_password: bool,
}

type UserRow = (
    i64,
    i64,
    String,
    Option<String>,
    String,
    bool,
    bool,
    Option<DateTime<Utc>>,
    DateTime<Utc>,
);

fn user_from_row(row: UserRow) -> Result<User, RepoError> {
    let role = Role::parse(&row.4).ok_or(RepoError::UnknownRole {
        role: row.4.clone(),
    })?;
    Ok(User {
        id: row.0,
        institution_id: row.1,
        username: row.2,
        display_name: row.3,
        role,
        is_active: row.5,
        must_change_password: row.6,
        last_login_at: row.7,
        created_at: row.8,
    })
}

pub async fn create_user(pool: &PgPool, new: NewUser<'_>) -> Result<User, RepoError> {
    create_user_for_institution(pool, 1, new).await
}

pub async fn create_user_for_institution(
    pool: &PgPool,
    institution_id: i64,
    new: NewUser<'_>,
) -> Result<User, RepoError> {
    let row: UserRow = sqlx::query_as(
        "INSERT INTO users (institution_id, username, display_name, password_hash, role, must_change_password)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING id, institution_id, username, display_name, role,
                   is_active, must_change_password, last_login_at, created_at",
    )
    .bind(institution_id)
    .bind(new.username)
    .bind(new.display_name)
    .bind(new.password_hash)
    .bind(new.role.as_str())
    .bind(new.must_change_password)
    .fetch_one(pool)
    .await
    .map_err(|error| {
        if is_unique_violation(&error) {
            RepoError::UsernameTaken {
                username: new.username.to_owned(),
            }
        } else {
            RepoError::Query(error)
        }
    })?;
    user_from_row(row)
}

pub async fn list_users_for_institution(
    pool: &PgPool,
    institution_id: i64,
) -> Result<Vec<User>, RepoError> {
    let rows: Vec<UserRow> = sqlx::query_as(
        "SELECT id, institution_id, username, display_name, role,
                is_active, must_change_password, last_login_at, created_at
         FROM users WHERE institution_id=$1 ORDER BY username",
    )
    .bind(institution_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(user_from_row).collect()
}

/// 用户是否持有一个显式正向权限授予。
pub async fn has_permission_grant(
    pool: &PgPool,
    user_id: i64,
    permission: Permission,
) -> Result<bool, RepoError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM user_permission_grants WHERE user_fk=$1 AND permission=$2)",
    )
    .bind(user_id)
    .bind(permission.as_str())
    .fetch_one(pool)
    .await?)
}

/// 列出一个机构内用户的显式权限授予。
pub async fn list_permission_grants(
    pool: &PgPool,
    institution_id: i64,
    user_id: i64,
) -> Result<Vec<String>, RepoError> {
    Ok(sqlx::query_scalar(
        r#"SELECT g.permission FROM user_permission_grants g
           JOIN users u ON u.id=g.user_fk
           WHERE g.user_fk=$1 AND u.institution_id=$2
           ORDER BY g.permission"#,
    )
    .bind(user_id)
    .bind(institution_id)
    .fetch_all(pool)
    .await?)
}

/// 原子替换用户的显式权限授予。当前仅开放报告审核权限。
pub async fn replace_permission_grants(
    pool: &PgPool,
    institution_id: i64,
    user_id: i64,
    permissions: &[String],
    granted_by: i64,
) -> Result<Vec<String>, RepoError> {
    let mut tx = pool.begin().await?;
    let target_role: Option<String> =
        sqlx::query_scalar("SELECT role FROM users WHERE id=$1 AND institution_id=$2 FOR UPDATE")
            .bind(user_id)
            .bind(institution_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(target_role) = target_role else {
        return Ok(Vec::new());
    };
    sqlx::query("DELETE FROM user_permission_grants WHERE user_fk=$1")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    for permission in permissions {
        if permission == Permission::ReviewReport.as_str()
            && matches!(target_role.as_str(), "admin" | "radiologist")
        {
            sqlx::query(
                r#"INSERT INTO user_permission_grants(user_fk,permission,granted_by)
                   VALUES($1,$2,$3)"#,
            )
            .bind(user_id)
            .bind(permission)
            .bind(granted_by)
            .execute(&mut *tx)
            .await?;
        }
    }
    let grants = sqlx::query_scalar(
        "SELECT permission FROM user_permission_grants WHERE user_fk=$1 ORDER BY permission",
    )
    .bind(user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(grants)
}

/// 带密码哈希的查询结果。
///
/// 必须把 10 列摊平写成一个元组,**不能写成 `(UserRow, String)`** ——
/// 嵌套元组会让 sqlx 以为第一列是 Postgres 的 `RECORD` 复合类型,
/// 于是拿整行去解 `INT8`,运行期报 `ColumnDecode`。
/// 这个错编译期看不出来:两种写法的类型都成立,只有真跑查询才暴露。
type UserWithHashRow = (
    i64,
    i64,
    String,
    Option<String>,
    String,
    bool,
    bool,
    Option<DateTime<Utc>>,
    DateTime<Utc>,
    String,
);

pub async fn find_by_username(
    pool: &PgPool,
    username: &str,
) -> Result<Option<(User, String)>, RepoError> {
    let row: Option<UserWithHashRow> = sqlx::query_as(
        "SELECT id, institution_id, username, display_name, role,
                is_active, must_change_password, last_login_at, created_at, password_hash
         FROM users WHERE username = $1",
    )
    .bind(username)
    .fetch_optional(pool)
    .await?;

    let Some((
        id,
        institution_id,
        name,
        display_name,
        role,
        is_active,
        must_change,
        last_login,
        created,
        hash,
    )) = row
    else {
        return Ok(None);
    };
    let user = user_from_row((
        id,
        institution_id,
        name,
        display_name,
        role,
        is_active,
        must_change,
        last_login,
        created,
    ))?;
    Ok(Some((user, hash)))
}

pub async fn find_by_id(pool: &PgPool, user_id: i64) -> Result<Option<User>, RepoError> {
    let row: Option<UserRow> = sqlx::query_as(
        "SELECT id, institution_id, username, display_name, role,
                is_active, must_change_password, last_login_at, created_at
         FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    row.map(user_from_row).transpose()
}

pub async fn list_users(pool: &PgPool) -> Result<Vec<User>, RepoError> {
    let rows: Vec<UserRow> = sqlx::query_as(
        "SELECT id, institution_id, username, display_name, role,
                is_active, must_change_password, last_login_at, created_at
         FROM users ORDER BY username",
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(user_from_row).collect()
}

/// 改密码。同时吊销该用户的全部 refresh token。
///
/// 改密后旧会话必须失效 —— 用户改密码的常见动机就是"怀疑密码泄露了",
/// 此时如果攻击者持有的会话还能继续用,改密码就白改了。
pub async fn set_password(
    pool: &PgPool,
    user_id: i64,
    password_hash: &str,
    must_change_password: bool,
) -> Result<(), RepoError> {
    let mut tx = pool.begin().await?;

    sqlx::query("UPDATE users SET password_hash = $2, must_change_password = $3 WHERE id = $1")
        .bind(user_id)
        .bind(password_hash)
        .bind(must_change_password)
        .execute(&mut *tx)
        .await?;

    sqlx::query(
        "UPDATE refresh_tokens SET revoked_at = now()
         WHERE user_fk = $1 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

/// 提交或更新一条待审核密码重置申请。
///
/// 账号不存在或已停用时返回 `None`，调用方仍应向外返回相同的已受理响应，
/// 避免公开接口被用来枚举账号。
pub async fn submit_password_reset_request(
    pool: &PgPool,
    username: &str,
    password_hash: &str,
) -> Result<Option<i64>, RepoError> {
    Ok(sqlx::query_scalar(
        r#"WITH target AS (
               SELECT id, institution_id
               FROM users
               WHERE username=$1 AND is_active
               ORDER BY id
               LIMIT 1
           )
           INSERT INTO password_reset_requests(institution_id,user_fk,password_hash)
           SELECT institution_id,id,$2 FROM target
           ON CONFLICT (user_fk) WHERE status='pending'
           DO UPDATE SET password_hash=EXCLUDED.password_hash,
                         requested_at=now(),reviewed_by=NULL,reviewed_at=NULL
           RETURNING id"#,
    )
    .bind(username)
    .bind(password_hash)
    .fetch_optional(pool)
    .await?)
}

type PasswordResetRow = (
    i64,
    i64,
    String,
    Option<String>,
    String,
    DateTime<Utc>,
    Option<i64>,
    Option<String>,
    Option<DateTime<Utc>>,
);

fn password_reset_from_row(row: PasswordResetRow) -> PasswordResetRequest {
    PasswordResetRequest {
        id: row.0,
        user_id: row.1,
        username: row.2,
        display_name: row.3,
        status: row.4,
        requested_at: row.5,
        reviewed_by: row.6,
        reviewer_name: row.7,
        reviewed_at: row.8,
    }
}

/// 列出机构内待管理员审核的密码重置申请。
pub async fn list_pending_password_reset_requests(
    pool: &PgPool,
    institution_id: i64,
) -> Result<Vec<PasswordResetRequest>, RepoError> {
    let rows: Vec<PasswordResetRow> = sqlx::query_as(
        r#"SELECT r.id,u.id,u.username,u.display_name,r.status,r.requested_at,
                  r.reviewed_by,reviewer.username,r.reviewed_at
           FROM password_reset_requests r
           JOIN users u ON u.id=r.user_fk
           LEFT JOIN users reviewer ON reviewer.id=r.reviewed_by
           WHERE r.institution_id=$1 AND r.status='pending'
           ORDER BY r.requested_at ASC"#,
    )
    .bind(institution_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(password_reset_from_row).collect())
}

/// 审核密码重置申请。批准时在同一事务内替换密码并吊销 refresh token；
/// 拒绝时只关闭申请，不触碰当前密码。
pub async fn review_password_reset_request(
    pool: &PgPool,
    institution_id: i64,
    request_id: i64,
    reviewer_id: i64,
    approve: bool,
) -> Result<Option<PasswordResetRequest>, RepoError> {
    let mut tx = pool.begin().await?;
    let pending: Option<(i64, String)> = sqlx::query_as(
        r#"SELECT r.user_fk,r.password_hash
           FROM password_reset_requests r
           JOIN users u ON u.id=r.user_fk
           WHERE r.id=$1 AND r.institution_id=$2 AND r.status='pending' AND u.is_active
           FOR UPDATE OF r,u"#,
    )
    .bind(request_id)
    .bind(institution_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((user_id, password_hash)) = pending else {
        tx.rollback().await?;
        return Ok(None);
    };

    let status = if approve { "approved" } else { "rejected" };
    if approve {
        sqlx::query("UPDATE users SET password_hash=$2,must_change_password=false WHERE id=$1")
            .bind(user_id)
            .bind(password_hash)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE refresh_tokens SET revoked_at=now() WHERE user_fk=$1 AND revoked_at IS NULL",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "UPDATE password_reset_requests SET status=$2,reviewed_by=$3,reviewed_at=now() WHERE id=$1",
    )
    .bind(request_id)
    .bind(status)
    .bind(reviewer_id)
    .execute(&mut *tx)
    .await?;

    let row: PasswordResetRow = sqlx::query_as(
        r#"SELECT r.id,u.id,u.username,u.display_name,r.status,r.requested_at,
                  r.reviewed_by,reviewer.username,r.reviewed_at
           FROM password_reset_requests r
           JOIN users u ON u.id=r.user_fk
           LEFT JOIN users reviewer ON reviewer.id=r.reviewed_by
           WHERE r.id=$1"#,
    )
    .bind(request_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(password_reset_from_row(row)))
}

/// 停用/启用账号。停用时一并吊销会话,否则已登录的人还能继续用到令牌过期。
pub async fn set_active(pool: &PgPool, user_id: i64, is_active: bool) -> Result<(), RepoError> {
    let mut tx = pool.begin().await?;

    sqlx::query("UPDATE users SET is_active = $2 WHERE id = $1")
        .bind(user_id)
        .bind(is_active)
        .execute(&mut *tx)
        .await?;

    if !is_active {
        sqlx::query(
            "UPDATE refresh_tokens SET revoked_at = now()
             WHERE user_fk = $1 AND revoked_at IS NULL",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

pub async fn touch_last_login(pool: &PgPool, user_id: i64) -> Result<(), RepoError> {
    sqlx::query("UPDATE users SET last_login_at = now() WHERE id = $1")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 库里的一条 refresh token 记录。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StoredRefreshToken {
    pub id: i64,
    pub user_fk: i64,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub replaced_by: Option<i64>,
}

impl StoredRefreshToken {
    /// 令牌当下是否可用。
    pub fn is_usable(&self, now: DateTime<Utc>) -> bool {
        self.revoked_at.is_none() && self.replaced_by.is_none() && self.expires_at > now
    }

    /// 是否是「已经被轮换掉却又被拿来用」。
    ///
    /// 合法客户端换到新令牌后就不会再用旧的,所以这种情况意味着旧令牌泄露了:
    /// 要么攻击者拿到了旧令牌,要么合法用户的新令牌被人抢先用掉。
    /// 两种都要求把整条链吊销。
    pub fn is_replayed(&self) -> bool {
        self.replaced_by.is_some()
    }
}

pub async fn store_refresh_token(
    pool: &PgPool,
    user_id: i64,
    token_hash: &[u8],
    expires_at: DateTime<Utc>,
    user_agent: Option<&str>,
    client_ip: Option<&str>,
) -> Result<i64, RepoError> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO refresh_tokens (user_fk, token_hash, expires_at, user_agent, client_ip)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING id",
    )
    .bind(user_id)
    .bind(token_hash)
    .bind(expires_at)
    .bind(user_agent)
    .bind(client_ip)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

pub async fn find_refresh_token(
    pool: &PgPool,
    token_hash: &[u8],
) -> Result<Option<StoredRefreshToken>, RepoError> {
    let row = sqlx::query_as(
        "SELECT id, user_fk, expires_at, revoked_at, replaced_by
         FROM refresh_tokens WHERE token_hash = $1",
    )
    .bind(token_hash)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// 轮换:吊销旧令牌、写入新令牌,并把两者串成链。
///
/// 在一个事务里完成 —— 中途失败会留下「旧的已吊销、新的没写进去」,
/// 用户会莫名其妙被登出。
pub async fn rotate_refresh_token(
    pool: &PgPool,
    old_token_id: i64,
    user_id: i64,
    new_token_hash: &[u8],
    expires_at: DateTime<Utc>,
    user_agent: Option<&str>,
    client_ip: Option<&str>,
) -> Result<i64, RepoError> {
    let mut tx = pool.begin().await?;

    let new_id: i64 = sqlx::query_scalar(
        "INSERT INTO refresh_tokens (user_fk, token_hash, expires_at, user_agent, client_ip)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING id",
    )
    .bind(user_id)
    .bind(new_token_hash)
    .bind(expires_at)
    .bind(user_agent)
    .bind(client_ip)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query("UPDATE refresh_tokens SET revoked_at = now(), replaced_by = $2 WHERE id = $1")
        .bind(old_token_id)
        .bind(new_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(new_id)
}

/// 吊销一条轮换链上的全部令牌。
///
/// 检测到重放时调用。只吊销被重放的那一条是不够的 —— 攻击者手里可能已经
/// 拿到了链上更新的令牌。用递归 CTE 从给定节点顺着 `replaced_by` 走到底。
pub async fn revoke_token_chain(pool: &PgPool, token_id: i64) -> Result<u64, RepoError> {
    let affected = sqlx::query(
        r#"
        WITH RECURSIVE chain AS (
            SELECT id, replaced_by FROM refresh_tokens WHERE id = $1
            UNION ALL
            SELECT t.id, t.replaced_by
            FROM refresh_tokens t
            JOIN chain c ON t.id = c.replaced_by
        )
        UPDATE refresh_tokens
        SET revoked_at = now()
        WHERE id IN (SELECT id FROM chain) AND revoked_at IS NULL
        "#,
    )
    .bind(token_id)
    .execute(pool)
    .await?;
    Ok(affected.rows_affected())
}

/// 吊销某个用户的全部会话(退出所有设备)。
pub async fn revoke_all_for_user(pool: &PgPool, user_id: i64) -> Result<u64, RepoError> {
    let affected = sqlx::query(
        "UPDATE refresh_tokens SET revoked_at = now()
         WHERE user_fk = $1 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(affected.rows_affected())
}

/// 清理已过期的令牌记录。过期的令牌不再有用,留着只会让表无限增长。
pub async fn purge_expired_tokens(pool: &PgPool) -> Result<u64, RepoError> {
    let affected = sqlx::query("DELETE FROM refresh_tokens WHERE expires_at < now()")
        .execute(pool)
        .await?;
    Ok(affected.rows_affected())
}

fn is_unique_violation(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(db) if db.is_unique_violation())
}
