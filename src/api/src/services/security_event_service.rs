use anyhow::Result;
use chrono::Utc;
use redis::AsyncCommands;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    EntityTrait, QueryFilter, Set, Statement,
};
use serde::{Deserialize, Serialize};
use shared::{config::Config, entities::security_events, queue::QueueService};
use std::sync::Arc;
use uuid::Uuid;

const INVESTIGATION_CACHE_TTL_SECS: usize = 300;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserIpActivity {
    pub ip: String,
    pub last_seen: chrono::DateTime<chrono::FixedOffset>,
    pub event_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpUserActivity {
    pub user_id: Uuid,
    pub last_seen: chrono::DateTime<chrono::FixedOffset>,
    pub event_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvestigationPage<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<u64>,
}

#[derive(Clone)]
pub struct SecurityEventService {
    db: DatabaseConnection,
    queue: QueueService,
    config: Arc<Config>,
}

impl SecurityEventService {
    pub fn new(db: DatabaseConnection, queue: QueueService, config: Arc<Config>) -> Self {
        Self { db, queue, config }
    }

    pub async fn record(
        &self,
        event_type: &str,
        user_id: Option<Uuid>,
        ip: &str,
        request_id: Option<&str>,
        user_agent: Option<&str>,
        metadata_json: serde_json::Value,
    ) -> Result<()> {
        let event = security_events::ActiveModel {
            id: Set(Uuid::new_v4()),
            user_id: Set(user_id),
            event_type: Set(event_type.to_string()),
            ip: Set(ip.to_string()),
            request_id: Set(request_id.map(str::to_string)),
            user_agent: Set(user_agent.map(str::to_string)),
            metadata_json: Set(metadata_json),
            created_at: Set(Utc::now().fixed_offset()),
        };
        event.insert(&self.db).await?;
        Ok(())
    }

    pub async fn purge_expired(&self) -> Result<u64> {
        let retention_days = self.config.security_events_retention_days.max(1);
        let cutoff = Utc::now() - chrono::Duration::days(retention_days);
        let result = security_events::Entity::delete_many()
            .filter(security_events::Column::CreatedAt.lt(cutoff.fixed_offset()))
            .exec(&self.db)
            .await?;
        Ok(result.rows_affected)
    }

    fn user_ips_cache_key(user_id: Uuid, window_days: i64, limit: u64, cursor: u64) -> String {
        format!(
            "user:ips:v1:{}:window:{}:limit:{}:cursor:{}",
            user_id, window_days, limit, cursor
        )
    }

    fn ip_users_cache_key(ip: &str, window_days: i64, limit: u64, cursor: u64) -> String {
        format!(
            "ip:users:v1:{}:window:{}:limit:{}:cursor:{}",
            ip, window_days, limit, cursor
        )
    }

    async fn read_cached_page<T: for<'de> Deserialize<'de>>(
        &self,
        cache_key: &str,
    ) -> Option<InvestigationPage<T>> {
        let mut conn = self.queue.get_conn().await.ok()?;
        let raw: Option<String> = conn.get(cache_key).await.ok()?;
        raw.and_then(|json| serde_json::from_str::<InvestigationPage<T>>(&json).ok())
    }

    async fn write_cached_page<T: Serialize>(&self, cache_key: &str, page: &InvestigationPage<T>) {
        if let Ok(mut conn) = self.queue.get_conn().await
            && let Ok(json) = serde_json::to_string(page)
        {
            let _: Result<(), _> = conn
                .set_ex(cache_key, json, INVESTIGATION_CACHE_TTL_SECS as u64)
                .await;
        }
    }

    pub async fn recent_ips_for_user(
        &self,
        user_id: Uuid,
        window_days: i64,
        limit: u64,
        cursor: u64,
    ) -> Result<InvestigationPage<UserIpActivity>> {
        let window_days = window_days.clamp(1, 3650);
        let limit = limit.clamp(1, 200);
        let cache_key = Self::user_ips_cache_key(user_id, window_days, limit, cursor);
        if let Some(page) = self.read_cached_page::<UserIpActivity>(&cache_key).await {
            return Ok(page);
        }

        let sql = r#"
            SELECT
                ip,
                MAX(created_at) AS last_seen,
                COUNT(*)::bigint AS event_count
            FROM security_events
            WHERE user_id = $1
              AND created_at >= (NOW() - ($2 || ' days')::interval)
            GROUP BY ip
            ORDER BY last_seen DESC
            LIMIT $3 OFFSET $4
        "#;
        let rows = self
            .db
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                sql,
                vec![
                    user_id.into(),
                    window_days.into(),
                    ((limit + 1) as i64).into(),
                    (cursor as i64).into(),
                ],
            ))
            .await?;

        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            let ip: String = row.try_get("", "ip")?;
            let last_seen: chrono::DateTime<chrono::FixedOffset> = row.try_get("", "last_seen")?;
            let event_count: i64 = row.try_get("", "event_count")?;
            items.push(UserIpActivity {
                ip,
                last_seen,
                event_count,
            });
        }
        let next_cursor = if items.len() as u64 > limit {
            items.truncate(limit as usize);
            Some(cursor + limit)
        } else {
            None
        };

        let page = InvestigationPage { items, next_cursor };
        self.write_cached_page(&cache_key, &page).await;
        Ok(page)
    }

    pub async fn recent_users_for_ip(
        &self,
        ip: &str,
        window_days: i64,
        limit: u64,
        cursor: u64,
    ) -> Result<InvestigationPage<IpUserActivity>> {
        let window_days = window_days.clamp(1, 3650);
        let limit = limit.clamp(1, 200);
        let cache_key = Self::ip_users_cache_key(ip, window_days, limit, cursor);
        if let Some(page) = self.read_cached_page::<IpUserActivity>(&cache_key).await {
            return Ok(page);
        }

        let sql = r#"
            SELECT
                user_id,
                MAX(created_at) AS last_seen,
                COUNT(*)::bigint AS event_count
            FROM security_events
            WHERE ip = $1
              AND user_id IS NOT NULL
              AND created_at >= (NOW() - ($2 || ' days')::interval)
            GROUP BY user_id
            ORDER BY last_seen DESC
            LIMIT $3 OFFSET $4
        "#;
        let rows = self
            .db
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                sql,
                vec![
                    ip.into(),
                    window_days.into(),
                    ((limit + 1) as i64).into(),
                    (cursor as i64).into(),
                ],
            ))
            .await?;

        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            let user_id: Uuid = row.try_get("", "user_id")?;
            let last_seen: chrono::DateTime<chrono::FixedOffset> = row.try_get("", "last_seen")?;
            let event_count: i64 = row.try_get("", "event_count")?;
            items.push(IpUserActivity {
                user_id,
                last_seen,
                event_count,
            });
        }
        let next_cursor = if items.len() as u64 > limit {
            items.truncate(limit as usize);
            Some(cursor + limit)
        } else {
            None
        };

        let page = InvestigationPage { items, next_cursor };
        self.write_cached_page(&cache_key, &page).await;
        Ok(page)
    }
}
