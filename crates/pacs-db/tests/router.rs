use pacs_core::fixture::{ct_instance, unique_uid};
use pacs_db::{RouteDestinationInput, RouteProtocol, RouteRuleInput, StorageRecord};
use pacs_store::{InstanceKey, Store};
use sqlx::migrate::MigrateDatabase;
use sqlx::{PgPool, Postgres};

async fn pool() -> Option<PgPool> {
    dotenvy::dotenv().ok();
    let Ok(url) = std::env::var("PACS_TEST_DATABASE_URL") else {
        assert!(
            std::env::var_os("CI").is_none(),
            "CI 必须设置 PACS_TEST_DATABASE_URL"
        );
        eprintln!("跳过 Router 数据库测试: 未设置 PACS_TEST_DATABASE_URL");
        return None;
    };
    if !Postgres::database_exists(&url).await.unwrap_or(false) {
        Postgres::create_database(&url).await.unwrap();
    }
    let pool = pacs_db::connect(&url).await.unwrap();
    pacs_db::migrate(&pool).await.expect("Router 迁移应能应用");
    Some(pool)
}

#[tokio::test]
async fn destinations_rules_and_deliveries_are_institution_scoped_and_idempotent() {
    let Some(pool) = pool().await else { return };
    sqlx::query(
        "DELETE FROM background_jobs j USING dicom_route_deliveries x, dicom_route_destinations d
         WHERE j.id=x.current_job_fk AND x.destination_fk=d.id AND d.name LIKE 'DCMTK-%'",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "DELETE FROM dicom_route_destinations WHERE institution_id=1 AND name LIKE 'DCMTK-%'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let institution_two: i64 = sqlx::query_scalar(
        "INSERT INTO institutions(code,name) VALUES($1,'Router Test') ON CONFLICT(code) DO UPDATE SET name=EXCLUDED.name RETURNING id",
    ).bind(format!("route-{}", uuid::Uuid::new_v4())).fetch_one(&pool).await.unwrap();

    let destination = pacs_db::create_route_destination(
        &pool,
        1,
        RouteDestinationInput {
            name: &format!("DCMTK-{}", uuid::Uuid::new_v4()),
            protocol: RouteProtocol::Dimse,
            enabled: true,
            host: Some("127.0.0.1"),
            port: Some(11112),
            called_ae_title: Some("STORESCP"),
            calling_ae_title: Some("REMOTE_PACS"),
            use_tls: false,
            stow_url: None,
            auth_token: None,
            ca_pem: None,
        },
    )
    .await
    .unwrap();
    pacs_db::create_route_destination(
        &pool,
        institution_two,
        RouteDestinationInput {
            name: "OTHER",
            protocol: RouteProtocol::Stow,
            enabled: true,
            host: None,
            port: None,
            called_ae_title: None,
            calling_ae_title: None,
            use_tls: false,
            stow_url: Some("https://example.invalid/stow"),
            auth_token: None,
            ca_pem: None,
        },
    )
    .await
    .unwrap();
    assert!(
        pacs_db::list_route_destinations(&pool, 1)
            .await
            .unwrap()
            .iter()
            .all(|v| v.institution_id == 1)
    );

    let rule = pacs_db::create_route_rule(
        &pool,
        1,
        RouteRuleInput {
            destination_id: destination.id,
            name: &format!("CT-{}", uuid::Uuid::new_v4()),
            priority: 10,
            enabled: true,
            source_ae_title: Some("MODALITY_AE"),
            modality: Some("CT"),
            body_part_examined: Some("CHEST"),
            study_description: Some("CHEST"),
            series_description: None,
            tag_matches: &serde_json::json!({}),
        },
    )
    .await
    .unwrap();

    let (study, series, sop) = (unique_uid(), unique_uid(), unique_uid());
    let object = ct_instance(&study, &series, &sop);
    let mut bytes = Vec::new();
    object.write_all(&mut bytes).unwrap();
    let metadata = pacs_core::extract_metadata(&object).unwrap();
    let storage = tempfile::tempdir().unwrap();
    let store = Store::open(storage.path()).await.unwrap();
    let stored = store
        .store(
            InstanceKey {
                study: &metadata.study.uid,
                series: &metadata.series.uid,
                sop: &metadata.instance.uid,
            },
            &bytes,
        )
        .await
        .unwrap();
    pacs_db::ingest_instance_for_institution(
        &pool,
        &metadata,
        StorageRecord {
            relative_path: &stored.relative_path,
            size: stored.size,
            sha256: &stored.sha256,
        },
        1,
    )
    .await
    .unwrap();
    let source = pacs_db::route_source_by_sop(&pool, 1, &sop).await.unwrap();
    let matched = pacs_db::matching_route_rules(&pool, &source, Some("MODALITY_AE"))
        .await
        .unwrap();
    assert_eq!(
        matched.iter().map(|v| v.id).collect::<Vec<_>>(),
        vec![rule.id]
    );
    assert!(
        pacs_db::matching_route_rules(&pool, &source, Some("OTHER"))
            .await
            .unwrap()
            .is_empty()
    );
    let first =
        pacs_db::enqueue_route_delivery(&pool, &source, destination.id, Some(rule.id), None)
            .await
            .unwrap();
    let duplicate =
        pacs_db::enqueue_route_delivery(&pool, &source, destination.id, Some(rule.id), None)
            .await
            .unwrap();
    assert!(first.is_some());
    assert!(duplicate.is_none());
    let deliveries = pacs_db::list_route_deliveries(&pool, 1, 100).await.unwrap();
    assert!(deliveries.iter().any(|value| value.sop_instance_uid == sop));
    assert!(
        pacs_db::list_route_deliveries(&pool, institution_two, 100)
            .await
            .unwrap()
            .iter()
            .all(|value| value.sop_instance_uid != sop)
    );
    pacs_db::request_job_cancellation(&pool, 1, first.unwrap())
        .await
        .unwrap();
    pacs_db::delete_route_destination(&pool, 1, destination.id)
        .await
        .unwrap();
}

#[tokio::test]
async fn observed_peers_track_associations_and_remain_institution_scoped() {
    let Some(pool) = pool().await else { return };
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let calling_ae = format!("OBS_{}", &suffix[..8]).to_uppercase();
    let remote_host = format!("test-{}.invalid", &suffix[..8]);

    pacs_db::observe_dicom_association_opened(&pool, 1, &calling_ae, &remote_host)
        .await
        .unwrap();
    pacs_db::observe_dicom_association_opened(&pool, 1, &calling_ae, &remote_host)
        .await
        .unwrap();

    let peers = pacs_db::list_observed_dicom_peers(&pool, 1, 500)
        .await
        .unwrap();
    let peer = peers
        .iter()
        .find(|peer| peer.calling_ae_title == calling_ae && peer.remote_host == remote_host)
        .unwrap();
    assert_eq!(peer.status, "connected");
    assert_eq!(peer.active_associations, 2);
    assert_eq!(peer.association_count, 2);

    pacs_db::observe_dicom_association_closed(&pool, 1, &calling_ae, &remote_host)
        .await
        .unwrap();
    pacs_db::observe_dicom_association_closed(&pool, 1, &calling_ae, &remote_host)
        .await
        .unwrap();
    let peers = pacs_db::list_observed_dicom_peers(&pool, 1, 500)
        .await
        .unwrap();
    let peer = peers
        .iter()
        .find(|peer| peer.calling_ae_title == calling_ae && peer.remote_host == remote_host)
        .unwrap();
    assert_eq!(peer.status, "recent");
    assert_eq!(peer.active_associations, 0);
    assert!(peer.last_disconnected_at.is_some());

    let institution_two: i64 = sqlx::query_scalar(
        "INSERT INTO institutions(code,name) VALUES($1,'Observed Peer Test') RETURNING id",
    )
    .bind(format!("peer-{suffix}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        pacs_db::list_observed_dicom_peers(&pool, institution_two, 500)
            .await
            .unwrap()
            .iter()
            .all(|peer| peer.calling_ae_title != calling_ae)
    );

    pacs_db::observe_dicom_association_opened(&pool, 1, &calling_ae, &remote_host)
        .await
        .unwrap();
    pacs_db::reset_observed_dicom_associations(&pool)
        .await
        .unwrap();
    let peers = pacs_db::list_observed_dicom_peers(&pool, 1, 500)
        .await
        .unwrap();
    assert_eq!(
        peers
            .iter()
            .find(|peer| peer.calling_ae_title == calling_ae && peer.remote_host == remote_host)
            .unwrap()
            .active_associations,
        0
    );
}
