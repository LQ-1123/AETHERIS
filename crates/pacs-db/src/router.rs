//! Persistence for institution-scoped DICOM routing.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::DbError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteProtocol {
    Dimse,
    Stow,
}

impl RouteProtocol {
    fn as_str(self) -> &'static str {
        match self {
            Self::Dimse => "dimse",
            Self::Stow => "stow",
        }
    }

    fn parse(value: &str) -> Result<Self, DbError> {
        match value {
            "dimse" => Ok(Self::Dimse),
            "stow" => Ok(Self::Stow),
            _ => Err(DbError::Invalid(format!("未知路由协议 {value:?}"))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteDestination {
    pub id: Uuid,
    pub institution_id: i64,
    pub name: String,
    pub protocol: RouteProtocol,
    pub enabled: bool,
    pub host: Option<String>,
    pub port: Option<i32>,
    pub called_ae_title: Option<String>,
    pub calling_ae_title: Option<String>,
    pub use_tls: bool,
    pub stow_url: Option<String>,
    #[serde(skip_serializing)]
    pub auth_token: Option<String>,
    #[serde(skip_serializing)]
    pub ca_pem: Option<String>,
    pub has_auth_token: bool,
    pub has_ca_certificate: bool,
    pub status: String,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_latency_ms: Option<i64>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct RouteDestinationInput<'a> {
    pub name: &'a str,
    pub protocol: RouteProtocol,
    pub enabled: bool,
    pub host: Option<&'a str>,
    pub port: Option<i32>,
    pub called_ae_title: Option<&'a str>,
    pub calling_ae_title: Option<&'a str>,
    pub use_tls: bool,
    pub stow_url: Option<&'a str>,
    pub auth_token: Option<&'a str>,
    pub ca_pem: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteRule {
    pub id: Uuid,
    pub institution_id: i64,
    pub destination_id: Uuid,
    pub destination_name: String,
    pub name: String,
    pub priority: i32,
    pub enabled: bool,
    pub source_ae_title: Option<String>,
    pub modality: Option<String>,
    pub body_part_examined: Option<String>,
    pub study_description: Option<String>,
    pub series_description: Option<String>,
    pub tag_matches: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct RouteRuleInput<'a> {
    pub destination_id: Uuid,
    pub name: &'a str,
    pub priority: i32,
    pub enabled: bool,
    pub source_ae_title: Option<&'a str>,
    pub modality: Option<&'a str>,
    pub body_part_examined: Option<&'a str>,
    pub study_description: Option<&'a str>,
    pub series_description: Option<&'a str>,
    pub tag_matches: &'a Value,
}

#[derive(Debug, Clone)]
pub struct RouteSource {
    pub institution_id: i64,
    pub version_id: i64,
    pub study_uid: String,
    pub series_uid: String,
    pub sop_uid: String,
    pub sop_class_uid: String,
    pub transfer_syntax_uid: String,
    pub storage_path: String,
    pub modality: Option<String>,
    pub body_part_examined: Option<String>,
    pub study_description: Option<String>,
    pub series_description: Option<String>,
    pub attributes: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteDelivery {
    pub id: Uuid,
    pub destination_id: Uuid,
    pub destination_name: String,
    pub rule_id: Option<Uuid>,
    pub sop_instance_uid: String,
    pub job_id: Option<Uuid>,
    pub status: String,
    pub attempts: i32,
    pub last_error: Option<String>,
    pub delivered_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservedDicomPeer {
    pub id: i64,
    pub institution_id: i64,
    pub calling_ae_title: String,
    pub remote_host: String,
    pub status: String,
    pub active_associations: i32,
    pub association_count: i64,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub last_disconnected_at: Option<DateTime<Utc>>,
}

pub async fn observe_dicom_association_opened(
    pool: &PgPool,
    institution_id: i64,
    calling_ae_title: &str,
    remote_host: &str,
) -> Result<(), DbError> {
    sqlx::query(
        r#"INSERT INTO dicom_observed_peers
           (institution_id,calling_ae_title,remote_host,active_associations,association_count)
           VALUES ($1,$2,$3,1,1)
           ON CONFLICT (institution_id,calling_ae_title,remote_host) DO UPDATE SET
             active_associations=dicom_observed_peers.active_associations+1,
             association_count=dicom_observed_peers.association_count+1,
             last_seen_at=now()"#,
    )
    .bind(institution_id)
    .bind(calling_ae_title.trim())
    .bind(remote_host.trim())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn observe_dicom_association_closed(
    pool: &PgPool,
    institution_id: i64,
    calling_ae_title: &str,
    remote_host: &str,
) -> Result<(), DbError> {
    sqlx::query(
        r#"UPDATE dicom_observed_peers SET
             active_associations=GREATEST(active_associations-1,0),
             last_seen_at=now(),last_disconnected_at=now()
           WHERE institution_id=$1 AND calling_ae_title=$2 AND remote_host=$3"#,
    )
    .bind(institution_id)
    .bind(calling_ae_title.trim())
    .bind(remote_host.trim())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn reset_observed_dicom_associations(pool: &PgPool) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE dicom_observed_peers SET active_associations=0 WHERE active_associations<>0",
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_observed_dicom_peers(
    pool: &PgPool,
    institution_id: i64,
    limit: i64,
) -> Result<Vec<ObservedDicomPeer>, DbError> {
    let rows = sqlx::query(
        r#"SELECT *,CASE
             WHEN active_associations>0 THEN 'connected'
             WHEN last_seen_at>=now()-interval '5 minutes' THEN 'recent'
             ELSE 'offline'
           END AS status
           FROM dicom_observed_peers
           WHERE institution_id=$1
           ORDER BY last_seen_at DESC,id
           LIMIT $2"#,
    )
    .bind(institution_id)
    .bind(limit.clamp(1, 500))
    .fetch_all(pool)
    .await?;
    rows.iter().map(decode_observed_peer).collect()
}

pub async fn list_route_destinations(
    pool: &PgPool,
    institution_id: i64,
) -> Result<Vec<RouteDestination>, DbError> {
    let rows = sqlx::query(
        "SELECT * FROM dicom_route_destinations WHERE institution_id = $1 ORDER BY name, id",
    )
    .bind(institution_id)
    .fetch_all(pool)
    .await?;
    rows.iter().map(decode_destination).collect()
}

pub async fn get_route_destination(
    pool: &PgPool,
    institution_id: i64,
    id: Uuid,
) -> Result<RouteDestination, DbError> {
    let row =
        sqlx::query("SELECT * FROM dicom_route_destinations WHERE institution_id = $1 AND id = $2")
            .bind(institution_id)
            .bind(id)
            .fetch_optional(pool)
            .await?
            .ok_or(DbError::NotFound)?;
    decode_destination(&row)
}

pub async fn create_route_destination(
    pool: &PgPool,
    institution_id: i64,
    input: RouteDestinationInput<'_>,
) -> Result<RouteDestination, DbError> {
    validate_destination(&input)?;
    let row = sqlx::query(
        r#"INSERT INTO dicom_route_destinations
           (id,institution_id,name,protocol,enabled,host,port,called_ae_title,calling_ae_title,use_tls,stow_url,auth_token,ca_pem)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) RETURNING *"#,
    )
    .bind(Uuid::new_v4())
    .bind(institution_id)
    .bind(input.name.trim())
    .bind(input.protocol.as_str())
    .bind(input.enabled)
    .bind(trim(input.host))
    .bind(input.port)
    .bind(trim(input.called_ae_title))
    .bind(trim(input.calling_ae_title))
    .bind(input.use_tls)
    .bind(trim(input.stow_url))
    .bind(trim(input.auth_token))
    .bind(trim(input.ca_pem))
    .fetch_one(pool)
    .await?;
    decode_destination(&row)
}

pub async fn update_route_destination(
    pool: &PgPool,
    institution_id: i64,
    id: Uuid,
    input: RouteDestinationInput<'_>,
) -> Result<RouteDestination, DbError> {
    validate_destination(&input)?;
    let row = sqlx::query(
        r#"UPDATE dicom_route_destinations SET name=$3,protocol=$4,enabled=$5,host=$6,port=$7,
           called_ae_title=$8,calling_ae_title=$9,use_tls=$10,stow_url=$11,
           auth_token=CASE WHEN $4='dimse' THEN NULL ELSE COALESCE($12,auth_token) END,
           ca_pem=COALESCE($13,ca_pem),
           status='unknown',last_error=NULL
           WHERE institution_id=$1 AND id=$2 RETURNING *"#,
    )
    .bind(institution_id)
    .bind(id)
    .bind(input.name.trim())
    .bind(input.protocol.as_str())
    .bind(input.enabled)
    .bind(trim(input.host))
    .bind(input.port)
    .bind(trim(input.called_ae_title))
    .bind(trim(input.calling_ae_title))
    .bind(input.use_tls)
    .bind(trim(input.stow_url))
    .bind(trim(input.auth_token))
    .bind(trim(input.ca_pem))
    .fetch_optional(pool)
    .await?
    .ok_or(DbError::NotFound)?;
    decode_destination(&row)
}

pub async fn delete_route_destination(
    pool: &PgPool,
    institution_id: i64,
    id: Uuid,
) -> Result<bool, DbError> {
    Ok(
        sqlx::query("DELETE FROM dicom_route_destinations WHERE institution_id=$1 AND id=$2")
            .bind(institution_id)
            .bind(id)
            .execute(pool)
            .await?
            .rows_affected()
            == 1,
    )
}

pub async fn record_destination_health(
    pool: &PgPool,
    institution_id: i64,
    id: Uuid,
    online: bool,
    latency_ms: i64,
    error: Option<&str>,
) -> Result<RouteDestination, DbError> {
    let row = sqlx::query(
        r#"UPDATE dicom_route_destinations SET status=$3,last_checked_at=now(),
           last_success_at=CASE WHEN $4 THEN now() ELSE last_success_at END,last_latency_ms=$5,last_error=$6
           WHERE institution_id=$1 AND id=$2 RETURNING *"#,
    )
    .bind(institution_id)
    .bind(id)
    .bind(if online { "online" } else { "offline" })
    .bind(online)
    .bind(latency_ms.max(0))
    .bind(error)
    .fetch_optional(pool)
    .await?
    .ok_or(DbError::NotFound)?;
    decode_destination(&row)
}

pub async fn list_route_rules(
    pool: &PgPool,
    institution_id: i64,
) -> Result<Vec<RouteRule>, DbError> {
    let rows = sqlx::query(
        r#"SELECT r.*,d.name AS destination_name FROM dicom_route_rules r
           JOIN dicom_route_destinations d ON d.id=r.destination_fk
           WHERE r.institution_id=$1 ORDER BY r.priority,r.name,r.id"#,
    )
    .bind(institution_id)
    .fetch_all(pool)
    .await?;
    rows.iter().map(decode_rule).collect()
}

pub async fn create_route_rule(
    pool: &PgPool,
    institution_id: i64,
    input: RouteRuleInput<'_>,
) -> Result<RouteRule, DbError> {
    validate_rule(&input)?;
    let row = sqlx::query(
        r#"WITH inserted AS (
           INSERT INTO dicom_route_rules (id,institution_id,destination_fk,name,priority,enabled,source_ae_title,modality,body_part_examined,study_description,series_description,tag_matches)
           SELECT $1,$2,d.id,$4,$5,$6,$7,$8,$9,$10,$11,$12 FROM dicom_route_destinations d
           WHERE d.id=$3 AND d.institution_id=$2 RETURNING *)
           SELECT inserted.*,d.name AS destination_name FROM inserted
           JOIN dicom_route_destinations d ON d.id=inserted.destination_fk"#,
    )
    .bind(Uuid::new_v4()).bind(institution_id).bind(input.destination_id)
    .bind(input.name.trim()).bind(input.priority).bind(input.enabled)
    .bind(trim(input.source_ae_title)).bind(trim(input.modality))
    .bind(trim(input.body_part_examined)).bind(trim(input.study_description))
    .bind(trim(input.series_description)).bind(input.tag_matches)
    .fetch_optional(pool).await?.ok_or(DbError::NotFound)?;
    decode_rule(&row)
}

pub async fn update_route_rule(
    pool: &PgPool,
    institution_id: i64,
    id: Uuid,
    input: RouteRuleInput<'_>,
) -> Result<RouteRule, DbError> {
    validate_rule(&input)?;
    let row = sqlx::query(
        r#"WITH updated AS (
           UPDATE dicom_route_rules r SET destination_fk=$3,name=$4,priority=$5,enabled=$6,
           source_ae_title=$7,modality=$8,body_part_examined=$9,study_description=$10,
           series_description=$11,tag_matches=$12 FROM dicom_route_destinations target
           WHERE r.institution_id=$1 AND r.id=$2 AND target.id=$3 AND target.institution_id=$1 RETURNING r.*)
           SELECT updated.*,d.name AS destination_name FROM updated
           JOIN dicom_route_destinations d ON d.id=updated.destination_fk"#,
    )
    .bind(institution_id).bind(id).bind(input.destination_id).bind(input.name.trim())
    .bind(input.priority).bind(input.enabled).bind(trim(input.source_ae_title))
    .bind(trim(input.modality)).bind(trim(input.body_part_examined))
    .bind(trim(input.study_description)).bind(trim(input.series_description)).bind(input.tag_matches)
    .fetch_optional(pool).await?.ok_or(DbError::NotFound)?;
    decode_rule(&row)
}

pub async fn delete_route_rule(
    pool: &PgPool,
    institution_id: i64,
    id: Uuid,
) -> Result<bool, DbError> {
    Ok(
        sqlx::query("DELETE FROM dicom_route_rules WHERE institution_id=$1 AND id=$2")
            .bind(institution_id)
            .bind(id)
            .execute(pool)
            .await?
            .rows_affected()
            == 1,
    )
}

pub async fn route_source_by_sop(
    pool: &PgPool,
    institution_id: i64,
    sop_uid: &str,
) -> Result<RouteSource, DbError> {
    let row = sqlx::query(
        r#"SELECT st.institution_id,v.id AS version_id,v.study_instance_uid,v.series_instance_uid,
           v.sop_instance_uid,COALESCE(i.sop_class_uid,'') AS sop_class_uid,v.transfer_syntax_uid,
           v.storage_path,se.modality,se.body_part_examined,st.description AS study_description,
           se.description AS series_description,
           (COALESCE(st.attributes,'{}') || COALESCE(se.attributes,'{}') || COALESCE(i.attributes,'{}')) AS attributes
           FROM instances i JOIN dicom_instance_versions v ON v.id=i.current_version_id
           JOIN series se ON se.id=i.series_fk JOIN studies st ON st.id=se.study_fk
           WHERE st.institution_id=$1 AND v.sop_instance_uid=$2"#,
    ).bind(institution_id).bind(sop_uid).fetch_optional(pool).await?.ok_or(DbError::NotFound)?;
    decode_source(&row)
}

pub async fn route_sources_for_scope(
    pool: &PgPool,
    institution_id: i64,
    study_uid: &str,
    series_uid: Option<&str>,
) -> Result<Vec<RouteSource>, DbError> {
    let rows = sqlx::query(
        r#"SELECT st.institution_id,v.id AS version_id,v.study_instance_uid,v.series_instance_uid,
           v.sop_instance_uid,COALESCE(i.sop_class_uid,'') AS sop_class_uid,v.transfer_syntax_uid,
           v.storage_path,se.modality,se.body_part_examined,st.description AS study_description,
           se.description AS series_description,
           (COALESCE(st.attributes,'{}') || COALESCE(se.attributes,'{}') || COALESCE(i.attributes,'{}')) AS attributes
           FROM instances i JOIN dicom_instance_versions v ON v.id=i.current_version_id
           JOIN series se ON se.id=i.series_fk JOIN studies st ON st.id=se.study_fk
           WHERE st.institution_id=$1 AND st.study_instance_uid=$2
             AND ($3::text IS NULL OR se.series_instance_uid=$3)
           ORDER BY se.series_instance_uid,i.instance_number NULLS LAST,v.sop_instance_uid"#,
    ).bind(institution_id).bind(study_uid).bind(series_uid).fetch_all(pool).await?;
    rows.iter().map(decode_source).collect()
}

pub async fn matching_route_rules(
    pool: &PgPool,
    source: &RouteSource,
    source_ae_title: Option<&str>,
) -> Result<Vec<RouteRule>, DbError> {
    let rules = list_route_rules(pool, source.institution_id).await?;
    Ok(rules
        .into_iter()
        .filter(|rule| rule.enabled && rule_matches(rule, source, source_ae_title))
        .collect())
}

fn rule_matches(rule: &RouteRule, source: &RouteSource, source_ae_title: Option<&str>) -> bool {
    matches_optional(&rule.source_ae_title, source_ae_title)
        && matches_optional(&rule.modality, source.modality.as_deref())
        && matches_optional(
            &rule.body_part_examined,
            source.body_part_examined.as_deref(),
        )
        && contains_optional(&rule.study_description, source.study_description.as_deref())
        && contains_optional(
            &rule.series_description,
            source.series_description.as_deref(),
        )
        && json_contains(&source.attributes, &rule.tag_matches)
}

pub async fn enqueue_route_delivery(
    pool: &PgPool,
    source: &RouteSource,
    destination_id: Uuid,
    rule_id: Option<Uuid>,
    created_by: Option<i64>,
) -> Result<Option<Uuid>, DbError> {
    let mut tx = pool.begin().await?;
    let delivery_id = Uuid::new_v4();
    let inserted: Option<Uuid> = sqlx::query_scalar(
        r#"INSERT INTO dicom_route_deliveries
           (id,institution_id,destination_fk,rule_fk,version_fk,sop_instance_uid)
           SELECT $1,$2,d.id,$4,$5,$6 FROM dicom_route_destinations d
           WHERE d.id=$3 AND d.institution_id=$2 AND d.enabled
           ON CONFLICT (destination_fk,version_fk) DO NOTHING RETURNING id"#,
    )
    .bind(delivery_id)
    .bind(source.institution_id)
    .bind(destination_id)
    .bind(rule_id)
    .bind(source.version_id)
    .bind(&source.sop_uid)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(delivery_id) = inserted else {
        tx.commit().await?;
        return Ok(None);
    };
    let job_id = Uuid::new_v4();
    let payload = serde_json::json!({"delivery_id":delivery_id,"destination_id":destination_id,
        "version_id":source.version_id,"sop_instance_uid":source.sop_uid});
    sqlx::query(
        r#"INSERT INTO background_jobs (id,institution_id,created_by,kind,idempotency_key,payload,progress_total,max_attempts)
           VALUES ($1,$2,$3,'route',$4,$5,1,5)"#,
    ).bind(job_id).bind(source.institution_id).bind(created_by)
      .bind(format!("{destination_id}:{}", source.version_id)).bind(payload).execute(&mut *tx).await?;
    sqlx::query("UPDATE dicom_route_deliveries SET current_job_fk=$2 WHERE id=$1")
        .bind(delivery_id)
        .bind(job_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Some(job_id))
}

pub async fn get_delivery_source(
    pool: &PgPool,
    institution_id: i64,
    delivery_id: Uuid,
) -> Result<(RouteDestination, RouteSource), DbError> {
    let row = sqlx::query(
        r#"SELECT x.destination_fk,st.institution_id,v.id AS version_id,v.study_instance_uid,
           v.series_instance_uid,v.sop_instance_uid,COALESCE(i.sop_class_uid,'') AS sop_class_uid,
           v.transfer_syntax_uid,v.storage_path,se.modality,se.body_part_examined,
           st.description AS study_description,se.description AS series_description,
           (COALESCE(st.attributes,'{}') || COALESCE(se.attributes,'{}') || COALESCE(i.attributes,'{}')) AS attributes
           FROM dicom_route_deliveries x JOIN dicom_instance_versions v ON v.id=x.version_fk
           JOIN instances i ON i.id=v.instance_fk JOIN series se ON se.id=i.series_fk
           JOIN studies st ON st.id=se.study_fk
           WHERE x.institution_id=$1 AND x.id=$2"#,
    )
    .bind(institution_id)
    .bind(delivery_id)
    .fetch_optional(pool)
    .await?
    .ok_or(DbError::NotFound)?;
    let destination_id: Uuid = row.try_get("destination_fk")?;
    Ok((
        get_route_destination(pool, institution_id, destination_id).await?,
        decode_source(&row)?,
    ))
}

pub async fn mark_delivery_running(
    pool: &PgPool,
    institution_id: i64,
    id: Uuid,
) -> Result<(), DbError> {
    sqlx::query("UPDATE dicom_route_deliveries SET status='running',attempts=attempts+1,last_error=NULL WHERE institution_id=$1 AND id=$2")
        .bind(institution_id).bind(id).execute(pool).await?;
    Ok(())
}

pub async fn finish_delivery(
    pool: &PgPool,
    institution_id: i64,
    id: Uuid,
    success: bool,
    error: Option<&str>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE dicom_route_deliveries SET status=$3,last_error=$4,delivered_at=CASE WHEN $5 THEN now() ELSE delivered_at END WHERE institution_id=$1 AND id=$2")
        .bind(institution_id).bind(id).bind(if success { "succeeded" } else { "dead_letter" })
        .bind(error).bind(success).execute(pool).await?;
    Ok(())
}

pub async fn retry_delivery(
    pool: &PgPool,
    institution_id: i64,
    id: Uuid,
    error: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE dicom_route_deliveries SET status='queued',last_error=$3
         WHERE institution_id=$1 AND id=$2",
    )
    .bind(institution_id)
    .bind(id)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_route_deliveries(
    pool: &PgPool,
    institution_id: i64,
    limit: i64,
) -> Result<Vec<RouteDelivery>, DbError> {
    let rows = sqlx::query(
        r#"SELECT x.*,d.name AS destination_name FROM dicom_route_deliveries x
           JOIN dicom_route_destinations d ON d.id=x.destination_fk
           WHERE x.institution_id=$1 ORDER BY x.created_at DESC,x.id LIMIT $2"#,
    )
    .bind(institution_id)
    .bind(limit.clamp(1, 500))
    .fetch_all(pool)
    .await?;
    rows.iter().map(decode_delivery).collect()
}

pub async fn replay_route_delivery(
    pool: &PgPool,
    institution_id: i64,
    id: Uuid,
    created_by: Option<i64>,
) -> Result<Uuid, DbError> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query("SELECT destination_fk,version_fk,sop_instance_uid,status FROM dicom_route_deliveries WHERE institution_id=$1 AND id=$2 FOR UPDATE")
        .bind(institution_id).bind(id).fetch_optional(&mut *tx).await?.ok_or(DbError::NotFound)?;
    let status: String = row.try_get("status")?;
    if status != "dead_letter" {
        return Err(DbError::Conflict("只有死信投递可以重放".to_owned()));
    }
    let destination_id: Uuid = row.try_get("destination_fk")?;
    let version_id: i64 = row.try_get("version_fk")?;
    let sop: String = row.try_get("sop_instance_uid")?;
    let job_id = Uuid::new_v4();
    let payload = serde_json::json!({"delivery_id":id,"destination_id":destination_id,"version_id":version_id,"sop_instance_uid":sop});
    sqlx::query("INSERT INTO background_jobs (id,institution_id,created_by,kind,payload,progress_total,max_attempts) VALUES ($1,$2,$3,'route',$4,1,5)")
        .bind(job_id).bind(institution_id).bind(created_by).bind(payload).execute(&mut *tx).await?;
    sqlx::query("UPDATE dicom_route_deliveries SET status='queued',current_job_fk=$2,last_error=NULL WHERE id=$1")
        .bind(id).bind(job_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(job_id)
}

fn validate_destination(input: &RouteDestinationInput<'_>) -> Result<(), DbError> {
    if input.name.trim().is_empty() {
        return Err(DbError::Invalid("目的地名称不能为空".to_owned()));
    }
    match input.protocol {
        RouteProtocol::Dimse => {
            if trim(input.host).is_none()
                || input.port.is_none_or(|p| !(1..=65535).contains(&p))
                || trim(input.called_ae_title).is_none_or(|v| v.len() > 16)
                || trim(input.calling_ae_title).is_none_or(|v| v.len() > 16)
            {
                return Err(DbError::Invalid(
                    "DIMSE 目的地需要主机、有效端口和 1-16 字符 AE Title".to_owned(),
                ));
            }
        }
        RouteProtocol::Stow => {
            if trim(input.stow_url).is_none() {
                return Err(DbError::Invalid("STOW 目的地需要 URL".to_owned()));
            }
        }
    }
    Ok(())
}

fn validate_rule(input: &RouteRuleInput<'_>) -> Result<(), DbError> {
    if input.name.trim().is_empty() || !input.tag_matches.is_object() {
        return Err(DbError::Invalid(
            "规则名称不能为空且 tag_matches 必须是对象".to_owned(),
        ));
    }
    Ok(())
}

fn trim(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}
fn matches_optional(expected: &Option<String>, actual: Option<&str>) -> bool {
    expected
        .as_deref()
        .is_none_or(|e| actual.is_some_and(|a| a.eq_ignore_ascii_case(e)))
}
fn contains_optional(expected: &Option<String>, actual: Option<&str>) -> bool {
    expected
        .as_deref()
        .is_none_or(|e| actual.is_some_and(|a| a.to_lowercase().contains(&e.to_lowercase())))
}
fn json_contains(actual: &Value, expected: &Value) -> bool {
    let Some(expected) = expected.as_object() else {
        return false;
    };
    expected
        .iter()
        .all(|(key, value)| actual.get(key) == Some(value))
}

fn decode_destination(row: &sqlx::postgres::PgRow) -> Result<RouteDestination, DbError> {
    let auth_token: Option<String> = row.try_get("auth_token")?;
    let ca_pem: Option<String> = row.try_get("ca_pem")?;
    Ok(RouteDestination {
        id: row.try_get("id")?,
        institution_id: row.try_get("institution_id")?,
        name: row.try_get("name")?,
        protocol: RouteProtocol::parse(row.try_get("protocol")?)?,
        enabled: row.try_get("enabled")?,
        host: row.try_get("host")?,
        port: row.try_get("port")?,
        called_ae_title: row.try_get("called_ae_title")?,
        calling_ae_title: row.try_get("calling_ae_title")?,
        use_tls: row.try_get("use_tls")?,
        stow_url: row.try_get("stow_url")?,
        has_auth_token: auth_token.is_some(),
        has_ca_certificate: ca_pem.is_some(),
        auth_token,
        ca_pem,
        status: row.try_get("status")?,
        last_checked_at: row.try_get("last_checked_at")?,
        last_success_at: row.try_get("last_success_at")?,
        last_latency_ms: row.try_get("last_latency_ms")?,
        last_error: row.try_get("last_error")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}
fn decode_rule(row: &sqlx::postgres::PgRow) -> Result<RouteRule, DbError> {
    Ok(RouteRule {
        id: row.try_get("id")?,
        institution_id: row.try_get("institution_id")?,
        destination_id: row.try_get("destination_fk")?,
        destination_name: row.try_get("destination_name")?,
        name: row.try_get("name")?,
        priority: row.try_get("priority")?,
        enabled: row.try_get("enabled")?,
        source_ae_title: row.try_get("source_ae_title")?,
        modality: row.try_get("modality")?,
        body_part_examined: row.try_get("body_part_examined")?,
        study_description: row.try_get("study_description")?,
        series_description: row.try_get("series_description")?,
        tag_matches: row.try_get("tag_matches")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}
fn decode_source(row: &sqlx::postgres::PgRow) -> Result<RouteSource, DbError> {
    Ok(RouteSource {
        institution_id: row.try_get("institution_id")?,
        version_id: row.try_get("version_id")?,
        study_uid: row.try_get("study_instance_uid")?,
        series_uid: row.try_get("series_instance_uid")?,
        sop_uid: row.try_get("sop_instance_uid")?,
        sop_class_uid: row.try_get("sop_class_uid")?,
        transfer_syntax_uid: row.try_get("transfer_syntax_uid")?,
        storage_path: row.try_get("storage_path")?,
        modality: row.try_get("modality")?,
        body_part_examined: row.try_get("body_part_examined")?,
        study_description: row.try_get("study_description")?,
        series_description: row.try_get("series_description")?,
        attributes: row.try_get("attributes")?,
    })
}
fn decode_delivery(row: &sqlx::postgres::PgRow) -> Result<RouteDelivery, DbError> {
    Ok(RouteDelivery {
        id: row.try_get("id")?,
        destination_id: row.try_get("destination_fk")?,
        destination_name: row.try_get("destination_name")?,
        rule_id: row.try_get("rule_fk")?,
        sop_instance_uid: row.try_get("sop_instance_uid")?,
        job_id: row.try_get("current_job_fk")?,
        status: row.try_get("status")?,
        attempts: row.try_get("attempts")?,
        last_error: row.try_get("last_error")?,
        delivered_at: row.try_get("delivered_at")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn decode_observed_peer(row: &sqlx::postgres::PgRow) -> Result<ObservedDicomPeer, DbError> {
    Ok(ObservedDicomPeer {
        id: row.try_get("id")?,
        institution_id: row.try_get("institution_id")?,
        calling_ae_title: row.try_get("calling_ae_title")?,
        remote_host: row.try_get("remote_host")?,
        status: row.try_get("status")?,
        active_associations: row.try_get("active_associations")?,
        association_count: row.try_get("association_count")?,
        first_seen_at: row.try_get("first_seen_at")?,
        last_seen_at: row.try_get("last_seen_at")?,
        last_disconnected_at: row.try_get("last_disconnected_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_and_text_matching_is_deterministic() {
        let rule = RouteRule {
            id: Uuid::nil(),
            institution_id: 1,
            destination_id: Uuid::nil(),
            destination_name: "D".into(),
            name: "CT".into(),
            priority: 1,
            enabled: true,
            source_ae_title: Some("MODALITY".into()),
            modality: Some("CT".into()),
            body_part_examined: Some("CHEST".into()),
            study_description: Some("lung".into()),
            series_description: None,
            tag_matches: serde_json::json!({"00100040":{"vr":"CS","Value":["M"]}}),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let source = RouteSource {
            institution_id: 1,
            version_id: 1,
            study_uid: "1".into(),
            series_uid: "2".into(),
            sop_uid: "3".into(),
            sop_class_uid: "4".into(),
            transfer_syntax_uid: "1.2".into(),
            storage_path: "x".into(),
            modality: Some("ct".into()),
            body_part_examined: Some("chest".into()),
            study_description: Some("LUNG SCREEN".into()),
            series_description: None,
            attributes: serde_json::json!({"00100040":{"vr":"CS","Value":["M"]}}),
        };
        assert!(rule_matches(&rule, &source, Some("modality")));
        assert!(!rule_matches(&rule, &source, Some("OTHER")));
    }
}
