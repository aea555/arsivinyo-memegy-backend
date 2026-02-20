//! `SeaORM` Entity

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "abuse_reports")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub video_id: Uuid,
    pub reporter_user_id: Uuid,
    pub reason_codes: Vec<String>,
    pub details: Option<String>,
    pub timestamp_seconds: Option<i32>,
    pub severity_score: i16,
    pub status: String,
    pub client_ip: String,
    pub user_agent: Option<String>,
    pub auto_quarantined: bool,
    pub auto_rule: Option<String>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
    pub closed_at: Option<DateTimeWithTimeZone>,
    pub closed_by_admin_sub: Option<String>,
    pub resolution_code: Option<String>,
    pub resolution_note: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::videos::Entity",
        from = "Column::VideoId",
        to = "super::videos::Column::Id",
        on_update = "Cascade",
        on_delete = "Cascade"
    )]
    Videos,
    #[sea_orm(
        belongs_to = "super::users::Entity",
        from = "Column::ReporterUserId",
        to = "super::users::Column::Id",
        on_update = "Cascade",
        on_delete = "Cascade"
    )]
    Users,
}

impl Related<super::videos::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Videos.def()
    }
}

impl Related<super::users::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Users.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
