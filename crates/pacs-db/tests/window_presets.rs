//! Account-scoped display-window preset persistence.

use pacs_db::{DbError, NewUserWindowPreset};
use sqlx::migrate::MigrateDatabase;
use sqlx::{PgPool, Postgres};
use uuid::Uuid;

async fn pool() -> Option<PgPool> {
    dotenvy::dotenv().ok();
    let Ok(url) = std::env::var("PACS_TEST_DATABASE_URL") else {
        assert!(
            std::env::var_os("CI").is_none(),
            "CI 必须设置 PACS_TEST_DATABASE_URL"
        );
        eprintln!("\n>>> 跳过用户窗预设数据库测试: 未设置 PACS_TEST_DATABASE_URL\n");
        return None;
    };
    if !Postgres::database_exists(&url).await.unwrap_or(false)
        && let Err(error) = Postgres::create_database(&url).await
    {
        assert!(
            Postgres::database_exists(&url).await.unwrap_or(false),
            "创建测试库失败: {error}"
        );
    }
    let pool = pacs_db::connect(&url).await.expect("应能连接测试库");
    pacs_db::migrate(&pool)
        .await
        .expect("用户窗预设迁移应能应用");
    Some(pool)
}

#[tokio::test]
async fn presets_are_personal_modality_scoped_and_case_insensitively_unique() {
    let Some(pool) = pool().await else { return };
    let suffix = Uuid::new_v4();
    let first_user: i64 = sqlx::query_scalar(
        "INSERT INTO users (institution_id, username, password_hash, role)
         VALUES (1, $1, 'unused', 'radiologist') RETURNING id",
    )
    .bind(format!("window-preset-a-{suffix}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let second_user: i64 = sqlx::query_scalar(
        "INSERT INTO users (institution_id, username, password_hash, role)
         VALUES (1, $1, 'unused', 'radiologist') RETURNING id",
    )
    .bind(format!("window-preset-b-{suffix}"))
    .fetch_one(&pool)
    .await
    .unwrap();

    let ct = pacs_db::create_user_window_preset(
        &pool,
        NewUserWindowPreset {
            institution_id: 1,
            user_id: first_user,
            modality: "CT",
            name: "My Lung",
            center: -600.0,
            width: 1500.0,
            function: "LINEAR",
        },
    )
    .await
    .unwrap();
    pacs_db::create_user_window_preset(
        &pool,
        NewUserWindowPreset {
            institution_id: 1,
            user_id: first_user,
            modality: "MR",
            name: "My Lung",
            center: 80.0,
            width: 160.0,
            function: "LINEAR_EXACT",
        },
    )
    .await
    .unwrap();

    assert_eq!(
        pacs_db::list_user_window_presets(&pool, 1, first_user)
            .await
            .unwrap()
            .len(),
        2
    );
    assert!(
        pacs_db::list_user_window_presets(&pool, 1, second_user)
            .await
            .unwrap()
            .is_empty()
    );

    let duplicate = pacs_db::create_user_window_preset(
        &pool,
        NewUserWindowPreset {
            institution_id: 1,
            user_id: first_user,
            modality: "CT",
            name: "my lung",
            center: -500.0,
            width: 1400.0,
            function: "LINEAR",
        },
    )
    .await;
    assert!(matches!(duplicate, Err(DbError::Conflict(_))));

    assert!(matches!(
        pacs_db::rename_user_window_preset(&pool, 1, second_user, ct.id, "Other").await,
        Err(DbError::NotFound)
    ));
    let renamed = pacs_db::rename_user_window_preset(&pool, 1, first_user, ct.id, "Chest")
        .await
        .unwrap();
    assert_eq!(renamed.name, "Chest");
    assert!(matches!(
        pacs_db::delete_user_window_preset(&pool, 1, second_user, ct.id).await,
        Err(DbError::NotFound)
    ));
    pacs_db::delete_user_window_preset(&pool, 1, first_user, ct.id)
        .await
        .unwrap();
}
