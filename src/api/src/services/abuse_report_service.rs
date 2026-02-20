use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;
use redis::AsyncCommands;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    EntityTrait, QueryFilter, QueryOrder, Set, Statement, TransactionTrait,
};
use shared::{
    config::Config,
    entities::{abuse_reports, videos},
    queue::QueueService,
};
use uuid::Uuid;

use crate::{cache::feed_cache::FeedCacheService, error::ApiErrorResponse};

pub const MODERATION_VISIBLE: &str = "VISIBLE";
pub const MODERATION_QUARANTINED: &str = "QUARANTINED";
pub const MODERATION_REMOVED: &str = "REMOVED";

pub const REPORT_STATUS_OPEN: &str = "open";
pub const REPORT_STATUS_UNDER_REVIEW: &str = "under_review";
pub const REPORT_STATUS_AUTO_QUARANTINED: &str = "auto_quarantined";
pub const REPORT_STATUS_RESOLVED: &str = "resolved";
pub const REPORT_STATUS_REJECTED: &str = "rejected";

pub const REASON_CSAM: &str = "CHILD_SEXUAL_ABUSE_MATERIAL";
pub const REASON_MINOR_SEXUAL_EXPLOITATION: &str = "MINOR_SEXUAL_EXPLOITATION";

const ALLOWED_REASON_CODES: &[&str] = &[
    "PORNOGRAPHY",
    REASON_CSAM,
    REASON_MINOR_SEXUAL_EXPLOITATION,
    "RAPE_GLORIFICATION",
    "PEDOPHILIC_CONTENT",
    "ZOOPHILIA_OR_BESTIALITY",
    "NECROPHILIA",
    "EXPLICIT_SEXUAL_CONTENT",
    "ILLEGAL_SUBSTANCE_PROMOTION",
    "MALICIOUS_OR_MANIPULATIVE",
    "GRAPHIC_OR_DISTURBING",
    "MURDER_OR_SERIOUS_INJURY",
    "CORPSE_CONTENT",
    "NSFW_MISTAGGED",
    "HATE_OR_RACISM",
    "OTHER",
];

const SEVERE_REASON_CODES: &[&str] = &[
    REASON_CSAM,
    REASON_MINOR_SEXUAL_EXPLOITATION,
    "RAPE_GLORIFICATION",
    "PEDOPHILIC_CONTENT",
    "ZOOPHILIA_OR_BESTIALITY",
    "NECROPHILIA",
    "MURDER_OR_SERIOUS_INJURY",
    "GRAPHIC_OR_DISTURBING",
    "CORPSE_CONTENT",
];

#[derive(Debug, Clone, Copy)]
pub enum VideoModerationAction {
    Quarantine,
    Remove,
    Restore,
}

#[derive(Debug, Clone)]
pub struct SubmitReportInput {
    pub video_id: Uuid,
    pub reporter_user_id: Uuid,
    pub reason_codes: Vec<String>,
    pub details: Option<String>,
    pub timestamp_seconds: Option<i32>,
    pub client_ip: String,
    pub user_agent: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SubmitReportOutcome {
    pub report: abuse_reports::Model,
    pub created: bool,
    pub auto_quarantined: bool,
    pub auto_rule: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ModerationOutcome {
    pub video: videos::Model,
    pub changed: bool,
}

#[derive(Clone)]
pub struct AbuseReportService {
    db: DatabaseConnection,
    queue: QueueService,
    feed_cache: FeedCacheService,
    config: Arc<Config>,
}

impl AbuseReportService {
    pub fn new(
        db: DatabaseConnection,
        queue: QueueService,
        feed_cache: FeedCacheService,
        config: Arc<Config>,
    ) -> Self {
        Self {
            db,
            queue,
            feed_cache,
            config,
        }
    }

    pub fn normalize_reason_codes(&self, reason_codes: &[String]) -> Result<Vec<String>, String> {
        if reason_codes.is_empty() {
            return Err("reason_codes must not be empty".to_string());
        }
        if reason_codes.len() > self.config.report_reason_max_count {
            return Err(format!(
                "reason_codes exceeds max count {}",
                self.config.report_reason_max_count
            ));
        }

        let allowed: std::collections::HashSet<&str> =
            ALLOWED_REASON_CODES.iter().copied().collect();
        let mut unique: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

        for raw in reason_codes {
            let normalized = raw.trim().to_ascii_uppercase();
            if normalized.is_empty() {
                return Err("reason_codes contains empty value".to_string());
            }
            if !allowed.contains(normalized.as_str()) {
                return Err(format!("unsupported reason code: {}", normalized));
            }
            unique.insert(normalized);
        }

        Ok(unique.into_iter().collect())
    }

    pub fn severity_score_for_reasons(reason_codes: &[String]) -> i16 {
        reason_codes
            .iter()
            .map(|code| match code.as_str() {
                REASON_CSAM | REASON_MINOR_SEXUAL_EXPLOITATION => 100,
                "RAPE_GLORIFICATION" | "PEDOPHILIC_CONTENT" => 95,
                "ZOOPHILIA_OR_BESTIALITY" | "NECROPHILIA" => 90,
                "MURDER_OR_SERIOUS_INJURY" | "CORPSE_CONTENT" => 85,
                "GRAPHIC_OR_DISTURBING" => 75,
                "PORNOGRAPHY" | "EXPLICIT_SEXUAL_CONTENT" => 70,
                "ILLEGAL_SUBSTANCE_PROMOTION" | "MALICIOUS_OR_MANIPULATIVE" => 65,
                "HATE_OR_RACISM" => 60,
                "NSFW_MISTAGGED" => 40,
                _ => 30,
            })
            .max()
            .unwrap_or(0)
    }

    pub fn is_immediate_auto_quarantine(reason_codes: &[String]) -> bool {
        reason_codes
            .iter()
            .any(|code| code == REASON_CSAM || code == REASON_MINOR_SEXUAL_EXPLOITATION)
    }

    pub fn has_severe_reason(reason_codes: &[String]) -> bool {
        reason_codes
            .iter()
            .any(|code| SEVERE_REASON_CODES.contains(&code.as_str()))
    }

    pub async fn submit_or_update_report(
        &self,
        input: SubmitReportInput,
    ) -> Result<SubmitReportOutcome, ApiErrorResponse> {
        let now = Utc::now().fixed_offset();
        let severity_score = Self::severity_score_for_reasons(&input.reason_codes);

        let txn = self.db.begin().await.map_err(ApiErrorResponse::db_error)?;

        let video = videos::Entity::find_by_id(input.video_id)
            .filter(videos::Column::DeletedAt.is_null())
            .filter(
                Condition::any()
                    .add(videos::Column::Status.eq("PUBLISHED"))
                    .add(videos::Column::Status.eq("published"))
                    .add(videos::Column::Status.eq("COMPLETED"))
                    .add(videos::Column::Status.eq("completed"))
                    .add(videos::Column::S3Bucket.eq(self.config.minio_bucket_videos.clone())),
            )
            .one(&txn)
            .await
            .map_err(ApiErrorResponse::db_error)?
            .ok_or_else(|| ApiErrorResponse::not_found("Video not found"))?;

        if video.moderation_state == MODERATION_REMOVED {
            return Err(ApiErrorResponse::not_found("Video not found"));
        }

        let existing = abuse_reports::Entity::find()
            .filter(abuse_reports::Column::VideoId.eq(input.video_id))
            .filter(abuse_reports::Column::ReporterUserId.eq(input.reporter_user_id))
            .filter(abuse_reports::Column::ClosedAt.is_null())
            .order_by_desc(abuse_reports::Column::CreatedAt)
            .one(&txn)
            .await
            .map_err(ApiErrorResponse::db_error)?;

        let created = existing.is_none();

        let mut report = if let Some(existing_report) = existing {
            let mut active: abuse_reports::ActiveModel = existing_report.into();
            active.reason_codes = Set(input.reason_codes.clone());
            active.details = Set(input.details.clone());
            active.timestamp_seconds = Set(input.timestamp_seconds);
            active.severity_score = Set(severity_score);
            active.client_ip = Set(input.client_ip.clone());
            active.user_agent = Set(input.user_agent.clone());
            active.updated_at = Set(now);
            active
                .update(&txn)
                .await
                .map_err(ApiErrorResponse::db_error)?
        } else {
            abuse_reports::ActiveModel {
                id: Set(Uuid::new_v4()),
                video_id: Set(input.video_id),
                reporter_user_id: Set(input.reporter_user_id),
                reason_codes: Set(input.reason_codes.clone()),
                details: Set(input.details.clone()),
                timestamp_seconds: Set(input.timestamp_seconds),
                severity_score: Set(severity_score),
                status: Set(REPORT_STATUS_OPEN.to_string()),
                client_ip: Set(input.client_ip.clone()),
                user_agent: Set(input.user_agent.clone()),
                auto_quarantined: Set(false),
                auto_rule: Set(None),
                created_at: Set(now),
                updated_at: Set(now),
                closed_at: Set(None),
                closed_by_admin_sub: Set(None),
                resolution_code: Set(None),
                resolution_note: Set(None),
            }
            .insert(&txn)
            .await
            .map_err(ApiErrorResponse::db_error)?
        };

        let mut auto_rule: Option<String> = None;
        if self.config.auto_quarantine_enabled {
            if Self::is_immediate_auto_quarantine(&input.reason_codes) {
                auto_rule = Some("immediate_minor_sexual_exploitation".to_string());
            } else if Self::has_severe_reason(&input.reason_codes) {
                let window_secs = self.config.auto_quarantine_window_secs.max(1) as i64;
                let threshold = self.config.auto_quarantine_severe_distinct_reporters.max(1) as i64;
                let sql = r#"
                    SELECT COUNT(DISTINCT reporter_user_id)::bigint AS reporter_count
                    FROM abuse_reports
                    WHERE video_id = $1
                      AND closed_at IS NULL
                      AND created_at >= (NOW() - ($2 || ' seconds')::interval)
                "#;
                let row = txn
                    .query_one(Statement::from_sql_and_values(
                        DatabaseBackend::Postgres,
                        sql,
                        vec![input.video_id.into(), window_secs.into()],
                    ))
                    .await
                    .map_err(ApiErrorResponse::db_error)?;
                let reporter_count = row
                    .and_then(|r| r.try_get::<i64>("", "reporter_count").ok())
                    .unwrap_or(0);
                if reporter_count >= threshold {
                    auto_rule = Some(format!(
                        "severe_distinct_reporters_{}",
                        self.config.auto_quarantine_severe_distinct_reporters.max(1)
                    ));
                }
            }
        }

        let mut moderation_changed = false;
        if let Some(ref rule) = auto_rule {
            let moderation = self
                .apply_video_moderation_action_txn(
                    &txn,
                    input.video_id,
                    VideoModerationAction::Quarantine,
                    Some("AUTO_QUARANTINE"),
                    "system:auto_quarantine",
                    Some(report.id),
                )
                .await?;
            moderation_changed = moderation.changed;

            let mut active: abuse_reports::ActiveModel = report.into();
            active.auto_quarantined = Set(true);
            active.auto_rule = Set(Some(rule.clone()));
            active.status = Set(REPORT_STATUS_AUTO_QUARANTINED.to_string());
            active.updated_at = Set(now);
            report = active
                .update(&txn)
                .await
                .map_err(ApiErrorResponse::db_error)?;
        }

        txn.commit().await.map_err(ApiErrorResponse::db_error)?;

        if moderation_changed {
            self.invalidate_public_video_caches().await;
        }

        Ok(SubmitReportOutcome {
            report,
            created,
            auto_quarantined: auto_rule.is_some(),
            auto_rule,
        })
    }

    async fn apply_video_moderation_action_txn(
        &self,
        txn: &sea_orm::DatabaseTransaction,
        video_id: Uuid,
        action: VideoModerationAction,
        reason_code: Option<&str>,
        actor: &str,
        source_report_id: Option<Uuid>,
    ) -> Result<ModerationOutcome, ApiErrorResponse> {
        let video = videos::Entity::find_by_id(video_id)
            .one(txn)
            .await
            .map_err(ApiErrorResponse::db_error)?
            .ok_or_else(|| ApiErrorResponse::not_found("Video not found"))?;

        if video.deleted_at.is_some() {
            return Err(ApiErrorResponse::not_found("Video not found"));
        }

        let target_state = match action {
            VideoModerationAction::Quarantine => MODERATION_QUARANTINED,
            VideoModerationAction::Remove => MODERATION_REMOVED,
            VideoModerationAction::Restore => MODERATION_VISIBLE,
        };

        let next_reason = if matches!(action, VideoModerationAction::Restore) {
            None
        } else {
            reason_code.map(str::to_string)
        };
        let next_source_report = if matches!(action, VideoModerationAction::Restore) {
            None
        } else {
            source_report_id
        };

        let changed = video.moderation_state != target_state
            || video.moderation_reason_code != next_reason
            || video.moderation_source_report_id != next_source_report;

        if !changed {
            return Ok(ModerationOutcome {
                video,
                changed: false,
            });
        }

        let mut active: videos::ActiveModel = video.into();
        active.moderation_state = Set(target_state.to_string());
        active.moderation_reason_code = Set(next_reason);
        active.moderation_updated_at = Set(Some(Utc::now().fixed_offset()));
        active.moderation_updated_by = Set(Some(actor.to_string()));
        active.moderation_source_report_id = Set(next_source_report);

        let updated = active
            .update(txn)
            .await
            .map_err(ApiErrorResponse::db_error)?;

        Ok(ModerationOutcome {
            video: updated,
            changed: true,
        })
    }

    pub async fn apply_video_moderation_action(
        &self,
        video_id: Uuid,
        action: VideoModerationAction,
        reason_code: Option<&str>,
        actor: &str,
        source_report_id: Option<Uuid>,
    ) -> Result<ModerationOutcome, ApiErrorResponse> {
        let txn = self.db.begin().await.map_err(ApiErrorResponse::db_error)?;

        let outcome = self
            .apply_video_moderation_action_txn(
                &txn,
                video_id,
                action,
                reason_code,
                actor,
                source_report_id,
            )
            .await?;

        txn.commit().await.map_err(ApiErrorResponse::db_error)?;

        if outcome.changed {
            self.invalidate_public_video_caches().await;
        }

        Ok(outcome)
    }

    pub async fn purge_expired_reports(&self) -> Result<u64> {
        let retention_days = self.config.abuse_report_retention_days.max(1);
        let cutoff = Utc::now() - chrono::Duration::days(retention_days);

        let result = abuse_reports::Entity::delete_many()
            .filter(abuse_reports::Column::CreatedAt.lt(cutoff.fixed_offset()))
            .exec(&self.db)
            .await?;

        Ok(result.rows_affected)
    }

    async fn invalidate_public_video_caches(&self) {
        let _ = self.feed_cache.invalidate_all().await;

        if let Ok(mut conn) = self.queue.get_conn().await {
            let mut keys: Vec<String> = Vec::new();
            keys.extend(
                conn.keys::<_, Vec<String>>("search:cache:*")
                    .await
                    .unwrap_or_default(),
            );
            if !keys.is_empty() {
                let _: Result<(), _> = conn.del(keys).await;
            }
        }
    }
}
