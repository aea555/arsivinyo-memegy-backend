use std::net::IpAddr;

use anyhow::Result;
use chrono::Utc;
use ipnet::IpNet;
use redis::AsyncCommands;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, Set,
};
use serde::{Deserialize, Serialize};
use shared::{
    config::Config,
    entities::{ip_bans, user_bans},
    queue::QueueService,
};
use std::sync::Arc;
use uuid::Uuid;

const USER_BAN_CACHE_PREFIX: &str = "ban:user:v1:";
const IP_BAN_CACHE_PREFIX: &str = "ban:ip:v1:";
const IP_VERDICT_CACHE_PREFIX: &str = "ban:ipverdict:v1:";
const IP_BAN_VERSION_KEY: &str = "ban:ip:version";
const TEMP_BAN_CACHE_CAP_SECS: usize = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BanEnforcementError {
    Banned,
    Unavailable,
}

#[derive(Debug, Clone)]
pub enum ParsedIpTarget {
    Ip(IpAddr),
    Cidr(IpNet),
}

impl ParsedIpTarget {
    pub fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        if trimmed.contains('/') {
            let cidr = trimmed.parse::<IpNet>().ok()?;
            return Some(Self::Cidr(cidr));
        }
        let ip = trimmed.parse::<IpAddr>().ok()?;
        Some(Self::Ip(ip))
    }

    pub fn canonical(&self) -> String {
        match self {
            Self::Ip(ip) => ip.to_string(),
            Self::Cidr(cidr) => cidr.to_string(),
        }
    }

    pub fn target_kind(&self) -> &'static str {
        match self {
            Self::Ip(_) => "ip",
            Self::Cidr(_) => "cidr",
        }
    }

    pub fn ip_family(&self) -> i16 {
        match self {
            Self::Ip(ip) => {
                if ip.is_ipv4() {
                    4
                } else {
                    6
                }
            }
            Self::Cidr(cidr) => {
                if cidr.addr().is_ipv4() {
                    4
                } else {
                    6
                }
            }
        }
    }

    pub fn cidr_prefix(&self) -> Option<i16> {
        match self {
            Self::Ip(_) => None,
            Self::Cidr(cidr) => Some(cidr.prefix_len() as i16),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BanCacheEntry {
    banned: bool,
    expires_at_unix: Option<i64>,
}

#[derive(Clone)]
pub struct BanService {
    db: DatabaseConnection,
    queue: QueueService,
    config: Arc<Config>,
}

impl BanService {
    pub fn new(db: DatabaseConnection, queue: QueueService, config: Arc<Config>) -> Self {
        Self { db, queue, config }
    }

    fn user_cache_key(user_id: Uuid) -> String {
        format!("{}{}", USER_BAN_CACHE_PREFIX, user_id)
    }

    fn ip_cache_key(ip: IpAddr) -> String {
        format!("{}{}", IP_BAN_CACHE_PREFIX, ip)
    }

    fn ip_verdict_cache_key(version: &str, ip: IpAddr) -> String {
        format!("{}{}:{}", IP_VERDICT_CACHE_PREFIX, version, ip)
    }

    fn active_ttl(
        banned_until: Option<chrono::DateTime<chrono::FixedOffset>>,
        permanent_ttl_secs: usize,
    ) -> usize {
        if let Some(until) = banned_until {
            let now = Utc::now();
            let remaining = (until.with_timezone(&Utc) - now).num_seconds().max(1) as usize;
            remaining.min(TEMP_BAN_CACHE_CAP_SECS)
        } else {
            permanent_ttl_secs.max(1)
        }
    }

    async fn read_cache_entry(&self, key: &str) -> Result<Option<BanCacheEntry>> {
        let mut conn = self.queue.get_conn().await?;
        let raw: Option<String> = conn.get(key).await?;
        Ok(raw.and_then(|v| serde_json::from_str::<BanCacheEntry>(&v).ok()))
    }

    async fn write_cache_entry(
        &self,
        key: &str,
        entry: &BanCacheEntry,
        ttl_secs: usize,
    ) -> Result<()> {
        let json = serde_json::to_string(entry)?;
        let mut conn = self.queue.get_conn().await?;
        let _: () = redis::cmd("SET")
            .arg(key)
            .arg(&json)
            .arg("EX")
            .arg(ttl_secs.max(1))
            .query_async(&mut conn)
            .await?;
        Ok(())
    }

    async fn delete_cache_key(&self, key: &str) -> Result<()> {
        let mut conn = self.queue.get_conn().await?;
        let _: () = conn.del(key).await?;
        Ok(())
    }

    fn cache_entry_is_expired(entry: &BanCacheEntry) -> bool {
        if !entry.banned {
            return false;
        }
        match entry.expires_at_unix {
            Some(ts) => Utc::now().timestamp() >= ts,
            None => false,
        }
    }

    async fn query_active_user_ban(&self, user_id: Uuid) -> Result<Option<user_bans::Model>> {
        let now = Utc::now().fixed_offset();
        let active = user_bans::Entity::find()
            .filter(user_bans::Column::UserId.eq(user_id))
            .filter(user_bans::Column::LiftedAt.is_null())
            .filter(
                Condition::any()
                    .add(user_bans::Column::BannedUntil.is_null())
                    .add(user_bans::Column::BannedUntil.gt(now)),
            )
            .order_by_desc(user_bans::Column::CreatedAt)
            .one(&self.db)
            .await?;
        Ok(active)
    }

    async fn is_user_banned_internal(&self, user_id: Uuid) -> Result<bool> {
        let key = Self::user_cache_key(user_id);
        if let Some(entry) = self.read_cache_entry(&key).await? {
            if Self::cache_entry_is_expired(&entry) {
                let _ = self.delete_cache_key(&key).await;
            } else {
                return Ok(entry.banned);
            }
        }

        let active = self.query_active_user_ban(user_id).await?;
        if let Some(ban) = active {
            let ttl = Self::active_ttl(
                ban.banned_until,
                self.config.ban_user_cache_permanent_ttl_secs,
            );
            let entry = BanCacheEntry {
                banned: true,
                expires_at_unix: ban.banned_until.map(|dt| dt.timestamp()),
            };
            let _ = self.write_cache_entry(&key, &entry, ttl).await;
            Ok(true)
        } else {
            let entry = BanCacheEntry {
                banned: false,
                expires_at_unix: None,
            };
            let _ = self
                .write_cache_entry(
                    &key,
                    &entry,
                    self.config.ban_user_cache_negative_ttl_secs.max(1),
                )
                .await;
            Ok(false)
        }
    }

    pub async fn ensure_user_not_banned(&self, user_id: Uuid) -> Result<(), BanEnforcementError> {
        match self.is_user_banned_internal(user_id).await {
            Ok(true) => Err(BanEnforcementError::Banned),
            Ok(false) => Ok(()),
            Err(err) => {
                tracing::error!("Failed user ban check for {}: {:?}", user_id, err);
                Err(BanEnforcementError::Unavailable)
            }
        }
    }

    pub async fn get_active_user_ban(&self, user_id: Uuid) -> Result<Option<user_bans::Model>> {
        self.query_active_user_ban(user_id).await
    }

    pub async fn upsert_user_ban(
        &self,
        user_id: Uuid,
        banned_until: Option<chrono::DateTime<chrono::FixedOffset>>,
        reason: Option<String>,
        actor_sub: &str,
    ) -> Result<user_bans::Model> {
        let existing = user_bans::Entity::find()
            .filter(user_bans::Column::UserId.eq(user_id))
            .filter(user_bans::Column::LiftedAt.is_null())
            .order_by_desc(user_bans::Column::CreatedAt)
            .one(&self.db)
            .await?;

        let now = Utc::now().fixed_offset();
        let model = if let Some(row) = existing {
            let mut active: user_bans::ActiveModel = row.into();
            active.reason = Set(reason);
            active.banned_until = Set(banned_until);
            active.created_by_admin_sub = Set(actor_sub.to_string());
            active.created_at = Set(now);
            active.lifted_at = Set(None);
            active.lifted_by_admin_sub = Set(None);
            active.lift_reason = Set(None);
            active.update(&self.db).await?
        } else {
            user_bans::ActiveModel {
                id: Set(Uuid::new_v4()),
                user_id: Set(user_id),
                reason: Set(reason),
                banned_until: Set(banned_until),
                created_by_admin_sub: Set(actor_sub.to_string()),
                created_at: Set(now),
                lifted_at: Set(None),
                lifted_by_admin_sub: Set(None),
                lift_reason: Set(None),
            }
            .insert(&self.db)
            .await?
        };

        let key = Self::user_cache_key(user_id);
        let ttl = Self::active_ttl(
            model.banned_until,
            self.config.ban_user_cache_permanent_ttl_secs,
        );
        let entry = BanCacheEntry {
            banned: true,
            expires_at_unix: model.banned_until.map(|dt| dt.timestamp()),
        };
        self.write_cache_entry(&key, &entry, ttl).await?;

        Ok(model)
    }

    pub async fn lift_user_ban(
        &self,
        user_id: Uuid,
        actor_sub: &str,
        lift_reason: Option<String>,
    ) -> Result<bool> {
        let existing = user_bans::Entity::find()
            .filter(user_bans::Column::UserId.eq(user_id))
            .filter(user_bans::Column::LiftedAt.is_null())
            .order_by_desc(user_bans::Column::CreatedAt)
            .one(&self.db)
            .await?;

        if let Some(model) = existing {
            let mut active: user_bans::ActiveModel = model.into();
            active.lifted_at = Set(Some(Utc::now().fixed_offset()));
            active.lifted_by_admin_sub = Set(Some(actor_sub.to_string()));
            active.lift_reason = Set(lift_reason);
            active.update(&self.db).await?;
            self.delete_cache_key(&Self::user_cache_key(user_id))
                .await?;
            return Ok(true);
        }

        self.delete_cache_key(&Self::user_cache_key(user_id))
            .await?;
        Ok(false)
    }

    async fn query_exact_ip_ban(&self, ip: IpAddr) -> Result<Option<ip_bans::Model>> {
        let now = Utc::now().fixed_offset();
        let model = ip_bans::Entity::find()
            .filter(ip_bans::Column::Target.eq(ip.to_string()))
            .filter(ip_bans::Column::TargetKind.eq("ip"))
            .filter(ip_bans::Column::LiftedAt.is_null())
            .filter(
                Condition::any()
                    .add(ip_bans::Column::BannedUntil.is_null())
                    .add(ip_bans::Column::BannedUntil.gt(now)),
            )
            .order_by_desc(ip_bans::Column::CreatedAt)
            .one(&self.db)
            .await?;
        Ok(model)
    }

    async fn current_ip_ban_version(&self) -> Result<String> {
        let mut conn = self.queue.get_conn().await?;
        let version: Option<String> = conn.get(IP_BAN_VERSION_KEY).await?;
        Ok(version.unwrap_or_else(|| "0".to_string()))
    }

    async fn bump_ip_ban_version(&self) -> Result<()> {
        let mut conn = self.queue.get_conn().await?;
        let _: i64 = conn.incr(IP_BAN_VERSION_KEY, 1).await?;
        Ok(())
    }

    async fn query_cidr_ip_bans(&self, ip: IpAddr) -> Result<Vec<ip_bans::Model>> {
        let now = Utc::now().fixed_offset();
        let family = if ip.is_ipv4() { 4 } else { 6 };
        let rows = ip_bans::Entity::find()
            .filter(ip_bans::Column::TargetKind.eq("cidr"))
            .filter(ip_bans::Column::IpFamily.eq(family))
            .filter(ip_bans::Column::LiftedAt.is_null())
            .filter(
                Condition::any()
                    .add(ip_bans::Column::BannedUntil.is_null())
                    .add(ip_bans::Column::BannedUntil.gt(now)),
            )
            .order_by_desc(ip_bans::Column::CidrPrefix)
            .order_by_desc(ip_bans::Column::CreatedAt)
            .all(&self.db)
            .await?;
        Ok(rows)
    }

    fn cidr_match_for_ip(ip: IpAddr, rows: &[ip_bans::Model]) -> Option<ip_bans::Model> {
        rows.iter()
            .find(|row| {
                row.target
                    .parse::<IpNet>()
                    .ok()
                    .is_some_and(|cidr| cidr.contains(&ip))
            })
            .cloned()
    }

    async fn is_ip_banned_internal(&self, ip: IpAddr) -> Result<bool> {
        let exact_key = Self::ip_cache_key(ip);
        if let Some(entry) = self.read_cache_entry(&exact_key).await?
            && !Self::cache_entry_is_expired(&entry)
            && entry.banned
        {
            return Ok(true);
        }

        if let Some(exact) = self.query_exact_ip_ban(ip).await? {
            let ttl = Self::active_ttl(
                exact.banned_until,
                self.config.ban_ip_cache_permanent_ttl_secs,
            );
            let entry = BanCacheEntry {
                banned: true,
                expires_at_unix: exact.banned_until.map(|dt| dt.timestamp()),
            };
            let _ = self.write_cache_entry(&exact_key, &entry, ttl).await;
            return Ok(true);
        } else {
            let _ = self
                .write_cache_entry(
                    &exact_key,
                    &BanCacheEntry {
                        banned: false,
                        expires_at_unix: None,
                    },
                    self.config.ban_ip_cache_negative_ttl_secs.max(1),
                )
                .await;
        }

        let version = self
            .current_ip_ban_version()
            .await
            .unwrap_or_else(|_| "0".to_string());
        let verdict_key = Self::ip_verdict_cache_key(&version, ip);
        if let Some(entry) = self.read_cache_entry(&verdict_key).await?
            && !Self::cache_entry_is_expired(&entry)
        {
            return Ok(entry.banned);
        }

        let cidr_rows = self.query_cidr_ip_bans(ip).await?;
        let matched = Self::cidr_match_for_ip(ip, &cidr_rows);
        let entry = BanCacheEntry {
            banned: matched.is_some(),
            expires_at_unix: matched
                .as_ref()
                .and_then(|row| row.banned_until.map(|dt| dt.timestamp())),
        };
        let ttl = matched
            .as_ref()
            .map(|row| {
                Self::active_ttl(
                    row.banned_until,
                    self.config.ban_ip_cache_permanent_ttl_secs,
                )
            })
            .unwrap_or_else(|| self.config.ban_ip_verdict_ttl_secs.max(1));
        let _ = self.write_cache_entry(&verdict_key, &entry, ttl).await;
        Ok(entry.banned)
    }

    pub async fn ensure_ip_not_banned(&self, ip: IpAddr) -> Result<(), BanEnforcementError> {
        match self.is_ip_banned_internal(ip).await {
            Ok(true) => Err(BanEnforcementError::Banned),
            Ok(false) => Ok(()),
            Err(err) => {
                tracing::error!("Failed IP ban check for {}: {:?}", ip, err);
                Err(BanEnforcementError::Unavailable)
            }
        }
    }

    pub async fn get_active_ip_ban(
        &self,
        target: &ParsedIpTarget,
    ) -> Result<Option<ip_bans::Model>> {
        let now = Utc::now().fixed_offset();
        let model = ip_bans::Entity::find()
            .filter(ip_bans::Column::Target.eq(target.canonical()))
            .filter(ip_bans::Column::TargetKind.eq(target.target_kind()))
            .filter(ip_bans::Column::LiftedAt.is_null())
            .filter(
                Condition::any()
                    .add(ip_bans::Column::BannedUntil.is_null())
                    .add(ip_bans::Column::BannedUntil.gt(now)),
            )
            .order_by_desc(ip_bans::Column::CreatedAt)
            .one(&self.db)
            .await?;
        Ok(model)
    }

    pub async fn upsert_ip_ban(
        &self,
        target: &ParsedIpTarget,
        banned_until: Option<chrono::DateTime<chrono::FixedOffset>>,
        reason: Option<String>,
        actor_sub: &str,
    ) -> Result<ip_bans::Model> {
        let canonical = target.canonical();
        let existing = ip_bans::Entity::find()
            .filter(ip_bans::Column::Target.eq(canonical.clone()))
            .filter(ip_bans::Column::TargetKind.eq(target.target_kind()))
            .filter(ip_bans::Column::LiftedAt.is_null())
            .order_by_desc(ip_bans::Column::CreatedAt)
            .one(&self.db)
            .await?;

        let now = Utc::now().fixed_offset();
        let model = if let Some(row) = existing {
            let mut active: ip_bans::ActiveModel = row.into();
            active.reason = Set(reason);
            active.banned_until = Set(banned_until);
            active.created_by_admin_sub = Set(actor_sub.to_string());
            active.created_at = Set(now);
            active.ip_family = Set(target.ip_family());
            active.cidr_prefix = Set(target.cidr_prefix());
            active.lifted_at = Set(None);
            active.lifted_by_admin_sub = Set(None);
            active.lift_reason = Set(None);
            active.update(&self.db).await?
        } else {
            ip_bans::ActiveModel {
                id: Set(Uuid::new_v4()),
                target: Set(canonical),
                target_kind: Set(target.target_kind().to_string()),
                ip_family: Set(target.ip_family()),
                cidr_prefix: Set(target.cidr_prefix()),
                reason: Set(reason),
                banned_until: Set(banned_until),
                created_by_admin_sub: Set(actor_sub.to_string()),
                created_at: Set(now),
                lifted_at: Set(None),
                lifted_by_admin_sub: Set(None),
                lift_reason: Set(None),
            }
            .insert(&self.db)
            .await?
        };

        match target {
            ParsedIpTarget::Ip(ip) => {
                let entry = BanCacheEntry {
                    banned: true,
                    expires_at_unix: model.banned_until.map(|dt| dt.timestamp()),
                };
                let ttl = Self::active_ttl(
                    model.banned_until,
                    self.config.ban_ip_cache_permanent_ttl_secs,
                );
                self.write_cache_entry(&Self::ip_cache_key(*ip), &entry, ttl)
                    .await?;
            }
            ParsedIpTarget::Cidr(_) => {}
        }
        self.bump_ip_ban_version().await?;

        Ok(model)
    }

    pub async fn lift_ip_ban(
        &self,
        target: &ParsedIpTarget,
        actor_sub: &str,
        lift_reason: Option<String>,
    ) -> Result<bool> {
        let existing = ip_bans::Entity::find()
            .filter(ip_bans::Column::Target.eq(target.canonical()))
            .filter(ip_bans::Column::TargetKind.eq(target.target_kind()))
            .filter(ip_bans::Column::LiftedAt.is_null())
            .order_by_desc(ip_bans::Column::CreatedAt)
            .one(&self.db)
            .await?;

        if let Some(model) = existing {
            let mut active: ip_bans::ActiveModel = model.into();
            active.lifted_at = Set(Some(Utc::now().fixed_offset()));
            active.lifted_by_admin_sub = Set(Some(actor_sub.to_string()));
            active.lift_reason = Set(lift_reason);
            active.update(&self.db).await?;
            if let ParsedIpTarget::Ip(ip) = target {
                let _ = self.delete_cache_key(&Self::ip_cache_key(*ip)).await;
            }
            self.bump_ip_ban_version().await?;
            return Ok(true);
        }

        if let ParsedIpTarget::Ip(ip) = target {
            let _ = self.delete_cache_key(&Self::ip_cache_key(*ip)).await;
        }
        self.bump_ip_ban_version().await?;
        Ok(false)
    }
}
