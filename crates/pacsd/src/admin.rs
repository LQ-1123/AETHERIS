//! `pacsd admin` 子命令：创建管理员账号。
//!
//! 这是账号体系的引导入口 —— 管理员后台自己需要一个管理员才能登录,
//! 总得有个起点。命令行方式不留默认密码,也是管理员把自己锁在外面时的恢复途径。

use anyhow::{Context, Result};
use pacs_auth::{Role, normalize_username, password, repository};
use sqlx::PgPool;

pub async fn create_admin(pool: &PgPool, username: &str, password: &str) -> Result<()> {
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
            role: Role::Admin,
            // 命令行创建的是自己的账号,不需要强制改密
            must_change_password: false,
        },
    )
    .await?;

    println!("✓ 管理员账号已创建:");
    println!("  用户名: {}", user.username);
    println!("  ID:     {}", user.id);
    println!("  角色:   {}", user.role);
    Ok(())
}
