use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(UserBans::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(UserBans::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(UserBans::UserId).uuid().not_null())
                    .col(ColumnDef::new(UserBans::Reason).text().null())
                    .col(
                        ColumnDef::new(UserBans::BannedUntil)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(UserBans::CreatedByAdminSub)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(UserBans::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(UserBans::LiftedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(ColumnDef::new(UserBans::LiftedByAdminSub).string().null())
                    .col(ColumnDef::new(UserBans::LiftReason).text().null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_user_bans_user_id")
                            .from(UserBans::Table, UserBans::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(IpBans::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(IpBans::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(IpBans::Target).string().not_null())
                    .col(ColumnDef::new(IpBans::TargetKind).string().not_null())
                    .col(ColumnDef::new(IpBans::IpFamily).small_integer().not_null())
                    .col(ColumnDef::new(IpBans::CidrPrefix).small_integer().null())
                    .col(ColumnDef::new(IpBans::Reason).text().null())
                    .col(
                        ColumnDef::new(IpBans::BannedUntil)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(IpBans::CreatedByAdminSub)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(IpBans::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(IpBans::LiftedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(ColumnDef::new(IpBans::LiftedByAdminSub).string().null())
                    .col(ColumnDef::new(IpBans::LiftReason).text().null())
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                ALTER TABLE ip_bans
                ADD CONSTRAINT chk_ip_bans_target_kind
                CHECK (target_kind IN ('ip', 'cidr'));
                "#,
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(SecurityEvents::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(SecurityEvents::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(SecurityEvents::UserId).uuid().null())
                    .col(
                        ColumnDef::new(SecurityEvents::EventType)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(SecurityEvents::Ip).string().not_null())
                    .col(ColumnDef::new(SecurityEvents::RequestId).string().null())
                    .col(ColumnDef::new(SecurityEvents::UserAgent).string().null())
                    .col(
                        ColumnDef::new(SecurityEvents::MetadataJson)
                            .json()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(SecurityEvents::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_security_events_user_id")
                            .from(SecurityEvents::Table, SecurityEvents::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::SetNull)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE UNIQUE INDEX uniq_user_bans_active_user
                ON user_bans (user_id)
                WHERE lifted_at IS NULL;
                "#,
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE INDEX idx_user_bans_active_user
                ON user_bans (user_id, created_at DESC)
                WHERE lifted_at IS NULL;
                "#,
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_user_bans_banned_until")
                    .table(UserBans::Table)
                    .col(UserBans::BannedUntil)
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE UNIQUE INDEX uniq_ip_bans_active_target
                ON ip_bans (target)
                WHERE lifted_at IS NULL;
                "#,
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE INDEX idx_ip_bans_exact_active
                ON ip_bans (target)
                WHERE lifted_at IS NULL AND target_kind = 'ip';
                "#,
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE INDEX idx_ip_bans_cidr_active
                ON ip_bans (ip_family, cidr_prefix DESC, created_at DESC)
                WHERE lifted_at IS NULL AND target_kind = 'cidr';
                "#,
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_ip_bans_banned_until")
                    .table(IpBans::Table)
                    .col(IpBans::BannedUntil)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_security_events_user_created_at")
                    .table(SecurityEvents::Table)
                    .col(SecurityEvents::UserId)
                    .col(SecurityEvents::CreatedAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_security_events_ip_created_at")
                    .table(SecurityEvents::Table)
                    .col(SecurityEvents::Ip)
                    .col(SecurityEvents::CreatedAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_security_events_created_at")
                    .table(SecurityEvents::Table)
                    .col(SecurityEvents::CreatedAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_security_events_event_type_created_at")
                    .table(SecurityEvents::Table)
                    .col(SecurityEvents::EventType)
                    .col(SecurityEvents::CreatedAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(SecurityEvents::Table).to_owned())
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE ip_bans DROP CONSTRAINT IF EXISTS chk_ip_bans_target_kind;",
            )
            .await?;
        manager
            .drop_table(Table::drop().table(IpBans::Table).to_owned())
            .await?;

        manager
            .drop_table(Table::drop().table(UserBans::Table).to_owned())
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum UserBans {
    Table,
    Id,
    UserId,
    Reason,
    BannedUntil,
    CreatedByAdminSub,
    CreatedAt,
    LiftedAt,
    LiftedByAdminSub,
    LiftReason,
}

#[derive(DeriveIden)]
enum IpBans {
    Table,
    Id,
    Target,
    TargetKind,
    IpFamily,
    CidrPrefix,
    Reason,
    BannedUntil,
    CreatedByAdminSub,
    CreatedAt,
    LiftedAt,
    LiftedByAdminSub,
    LiftReason,
}

#[derive(DeriveIden)]
enum SecurityEvents {
    Table,
    Id,
    UserId,
    EventType,
    Ip,
    RequestId,
    UserAgent,
    MetadataJson,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Users {
    Table,
    Id,
}
