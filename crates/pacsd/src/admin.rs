//! `pacsd admin` 与 `pacsd user create` 子命令的账号创建实现。
//!
//! 这是账号体系的引导入口 —— 管理员后台自己需要一个管理员才能登录,
//! 总得有个起点。命令行方式不留默认密码,也是管理员把自己锁在外面时的恢复途径。

use anyhow::{Context, Result};
use pacs_auth::{Role, normalize_username, password, repository};
use sqlx::PgPool;

pub async fn create_admin(pool: &PgPool, username: &str, password: &str) -> Result<()> {
    create_user(pool, username, password, Role::Admin).await
}

pub async fn create_user(pool: &PgPool, username: &str, password: &str, role: Role) -> Result<()> {
    let normalized =
        normalize_username(username).with_context(|| format!("用户名 {username:?} 不符合规则"))?;

    password::check_strength(password, &normalized).context("密码强度不足")?;

    let hash = password::hash(password).context("密码哈希失败")?;

    let user = repository::create_user(
        pool,
        repository::NewUser {
            username: &normalized,
            display_name: None,
            password_hash: &hash,
            role,
            // change-password HTTP 入口完成前不能强制首次改密,否则账号无法登录。
            must_change_password: false,
        },
    )
    .await?;

    println!("✓ 用户账号已创建:");
    println!("  用户名: {}", user.username);
    println!("  ID:     {}", user.id);
    println!("  角色:   {}", user.role);
    Ok(())
}
