//! 登录路径的集成测试。跑在真实 Postgres 上。
//!
//! # 为什么必须连真库
//!
//! 这些测试是为一个溜过去的 bug 补的:`find_by_username` 曾把返回类型写成
//! `(UserRow, String)` —— 嵌套元组让 sqlx 以为第一列是 Postgres 的 `RECORD`
//! 复合类型,拿整行去解 `INT8`,运行期报 `ColumnDecode`。
//!
//! 关键在于**编译期完全看不出问题**:两种写法的 Rust 类型都成立。而当时
//! `pacs-auth` 一条连库测试都没有,单元测试只覆盖了哈希和令牌这些纯逻辑,
//! 于是"HTTP 登录从来没成功过"这件事一直没被发现 —— 建账号走的是另一条
//! SQL,它是对的。
//!
//! 教训:凡是行解码,单元测试给不了任何保证。

use pacs_auth::{AuthError, AuthService, Role, password, repository};
use sqlx::PgPool;
use sqlx::Postgres;
use sqlx::migrate::MigrateDatabase;

const TEST_SECRET: &[u8] = b"a-test-secret-that-is-long-enough-for-hs256";

async fn pool() -> Option<PgPool> {
    dotenvy::dotenv().ok();
    let Ok(url) = std::env::var("PACS_TEST_DATABASE_URL") else {
        assert!(
            std::env::var_os("CI").is_none(),
            "CI 环境必须设置 PACS_TEST_DATABASE_URL,登录路径测试不允许跳过"
        );
        eprintln!(
            "\n>>> 跳过登录测试:未设置 PACS_TEST_DATABASE_URL。\
             \n>>> 这些测试覆盖登录的行解码,本地请照 .env.example 配置后重跑。\n"
        );
        return None;
    };
    if !Postgres::database_exists(&url).await.unwrap_or(false)
        && let Err(error) = Postgres::create_database(&url).await
    {
        assert!(
            Postgres::database_exists(&url).await.unwrap_or(false),
            "创建测试库失败(连接账号需要 CREATEDB 权限):{error}"
        );
    }
    let pool = pacs_db::connect(&url).await.expect("应能连上测试库");
    pacs_db::migrate(&pool).await.expect("迁移应能应用");
    Some(pool)
}

/// 建一个用户名唯一的账号,返回 (用户名, 密码)。
async fn fresh_account(pool: &PgPool, role: Role) -> (String, String) {
    // 用户名规则:小写字母、数字、`.` `_` `-`,须以字母或数字开头。
    // UUID 里有连字符,取十六进制部分即可。
    let username = format!("t{}", uuid::Uuid::new_v4().simple());
    let password = format!("pw-{}-abcdefgh", uuid::Uuid::new_v4().simple());
    let hash = password::hash(&password).expect("应能哈希");

    repository::create_user(
        pool,
        repository::NewUser {
            username: &username,
            display_name: Some("集成测试账号"),
            password_hash: &hash,
            role,
            must_change_password: false,
        },
    )
    .await
    .expect("应能建账号");

    (username, password)
}

/// 正确的用户名密码应当登录成功,并拿到可用的令牌。
///
/// 这条是那个 `ColumnDecode` bug 的直接回归测试。
#[tokio::test]
async fn login_with_correct_credentials_succeeds() {
    let Some(pool) = pool().await else {
        return;
    };
    let service = AuthService::new(pool.clone(), TEST_SECRET).expect("应能构造");
    let (username, password) = fresh_account(&pool, Role::Radiologist).await;

    let (access, refresh, user) = service
        .login(&username, &password, None, None)
        .await
        .expect("正确凭据应能登录 —— 失败多半是行解码写错了");

    assert_eq!(user.username, username);
    assert_eq!(user.role, Role::Radiologist);
    assert!(!access.is_empty());
    assert!(!refresh.is_empty());

    // 签发的 access token 必须能被同一个服务验回来
    let claims = service.verify_access_token(&access).expect("应能验签");
    assert_eq!(claims.sub, user.id);
    assert_eq!(claims.username, username);
    assert_eq!(claims.role().unwrap(), Role::Radiologist);
}

/// 密码错误要回 InvalidCredentials,不是内部错误。
///
/// 这个区分有实际意义:内部错误会被监控当成故障告警,而密码输错是日常。
#[tokio::test]
async fn wrong_password_is_rejected_as_invalid_credentials() {
    let Some(pool) = pool().await else {
        return;
    };
    let service = AuthService::new(pool.clone(), TEST_SECRET).expect("应能构造");
    let (username, _password) = fresh_account(&pool, Role::Viewer).await;

    let error = service
        .login(&username, "definitely-the-wrong-password", None, None)
        .await
        .expect_err("错密码应当失败");
    assert!(
        matches!(error, AuthError::InvalidCredentials),
        "应是 InvalidCredentials,实际:{error:?}"
    );
}

/// 不存在的用户名同样回 InvalidCredentials,不泄露"这个账号不存在"。
#[tokio::test]
async fn unknown_username_does_not_leak_account_existence() {
    let Some(pool) = pool().await else {
        return;
    };
    let service = AuthService::new(pool, TEST_SECRET).expect("应能构造");

    let error = service
        .login("no.such.user.at.all", "whatever-password-here", None, None)
        .await
        .expect_err("不存在的账号应当失败");
    assert!(
        matches!(error, AuthError::InvalidCredentials),
        "不该区分「账号不存在」和「密码错误」,实际:{error:?}"
    );
}

/// 用户名大小写不敏感:`Alice` 和 `alice` 是同一个账号。
#[tokio::test]
async fn username_is_case_insensitive_at_login() {
    let Some(pool) = pool().await else {
        return;
    };
    let service = AuthService::new(pool.clone(), TEST_SECRET).expect("应能构造");
    let (username, password) = fresh_account(&pool, Role::Viewer).await;

    let upper = username.to_uppercase();
    assert_ne!(upper, username, "测试前提:用户名含字母");
    service
        .login(&upper, &password, None, None)
        .await
        .expect("大写用户名应当能登录 —— 否则同一个人会有两个账号");
}

/// refresh 能换到新令牌,且旧的 refresh token 立刻失效(轮换)。
#[tokio::test]
async fn refresh_rotates_the_token() {
    let Some(pool) = pool().await else {
        return;
    };
    let service = AuthService::new(pool.clone(), TEST_SECRET).expect("应能构造");
    let (username, password) = fresh_account(&pool, Role::Technician).await;

    let (_access, first_refresh, _user) = service
        .login(&username, &password, None, None)
        .await
        .expect("应能登录");

    let (_new_access, second_refresh, _) = service
        .refresh(&first_refresh, None, None)
        .await
        .expect("应能续期");
    assert_ne!(first_refresh, second_refresh, "续期应当轮换令牌");

    // 旧令牌再用一次要被识别为重放
    let replay = service.refresh(&first_refresh, None, None).await;
    assert!(
        matches!(
            replay,
            Err(AuthError::TokenReplayed) | Err(AuthError::TokenRevoked)
        ),
        "已轮换的旧令牌必须失效,实际:{replay:?}"
    );
}

/// 停用的账号不能登录。
#[tokio::test]
async fn disabled_account_cannot_log_in() {
    let Some(pool) = pool().await else {
        return;
    };
    let service = AuthService::new(pool.clone(), TEST_SECRET).expect("应能构造");
    let (username, password) = fresh_account(&pool, Role::Viewer).await;

    sqlx::query("UPDATE users SET is_active = false WHERE username = $1")
        .bind(&username)
        .execute(&pool)
        .await
        .expect("应能停用账号");

    let error = service
        .login(&username, &password, None, None)
        .await
        .expect_err("停用账号应当无法登录");
    assert!(
        matches!(error, AuthError::AccountDisabled),
        "应是 AccountDisabled,实际:{error:?}"
    );
}

/// `find_by_username` 要能同时取回用户和密码哈希 —— 直接盯住出过 bug 的那个函数。
#[tokio::test]
async fn find_by_username_decodes_both_the_user_and_the_hash() {
    let Some(pool) = pool().await else {
        return;
    };
    let (username, password) = fresh_account(&pool, Role::Admin).await;

    let found = repository::find_by_username(&pool, &username)
        .await
        .expect("查询本身不该失败 —— 失败说明行解码又写错了")
        .expect("刚建的账号应当查得到");

    let (user, hash) = found;
    assert_eq!(user.username, username);
    assert_eq!(user.role, Role::Admin);
    assert!(user.is_active);
    // 取回的哈希必须真的能验密码,不能是别的列错位读进来的
    assert!(
        password::verify(&password, &hash).unwrap_or(false),
        "取回的哈希应能验通原密码 —— 验不过说明列错位了"
    );

    // 不存在的账号回 None,不是错误
    assert!(
        repository::find_by_username(&pool, "no.such.user.at.all")
            .await
            .expect("查不到不是错误")
            .is_none()
    );
}
