//! Postgres 访问层。
//!
//! 服务端独占数据库连接,客户端绝不直连(见实施计划"信任边界")。软件要分发到
//! 不同机器、不同账号,客户端一旦内嵌连接串就等于把库凭据发给每个用户 ——
//! 无法做权限控制、无法吊销、无法轮换。
//!
//! # 关于 SQL 的编译期校验
//!
//! 这里用运行期校验的 `sqlx::query`,而不是编译期校验的 `query!` 宏。
//! 宏要求编译时能连上数据库或提交 `.sqlx` 离线缓存,而 C-FIND 的查询是按
//! 匹配键动态拼的(阶段 4),本来就只能运行期构造 —— 最容易出错的那部分覆盖不到,
//! 却要为此在每次改表后多一道 `cargo sqlx prepare`。SQL 的正确性交给
//! 跑在真实数据库上的集成测试来保证。

pub mod find;
pub mod ingest;
pub mod retrieve;

use std::time::Duration;

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use thiserror::Error;

pub use find::{DEFAULT_LIMIT, FindResults, find};
pub use ingest::{Ingested, StorageRecord, ingest_instance};
pub use retrieve::{StoredInstance, find_instance, list_series_instances};

#[derive(Debug, Error)]
pub enum DbError {
    #[error("数据库操作失败")]
    Query(#[from] sqlx::Error),
    #[error("数据库迁移失败")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    /// 查询命中的记录数超过上限。
    ///
    /// 刻意报错而不是截断:截断会让对方以为"结果就这么多",
    /// 一次静默漏掉的检查比一次明确的失败危险得多。
    #[error("结果超过 {limit} 条,请收窄查询条件")]
    TooManyResults { limit: usize },
}

/// 连接数据库并建立连接池。
pub async fn connect(url: &str) -> Result<PgPool, DbError> {
    let pool = PgPoolOptions::new()
        .max_connections(16)
        .acquire_timeout(Duration::from_secs(5))
        .connect(url)
        .await?;
    Ok(pool)
}

/// 执行所有未应用的迁移。
///
/// 迁移在编译期嵌入二进制,部署时不用带 SQL 文件。
pub async fn migrate(pool: &PgPool) -> Result<(), DbError> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}
