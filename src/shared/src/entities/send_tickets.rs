//! `SeaORM` Entity

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "send_tickets")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub ticket_id: Uuid,
    pub user_id: Uuid,
    pub video_id: Uuid,
    pub device_id_hash: String,
    pub host_app_hint: Option<String>,
    pub created_at: DateTimeWithTimeZone,
    pub expires_at: DateTimeWithTimeZone,
    pub redeemed_at: Option<DateTimeWithTimeZone>,
    pub status: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::users::Entity",
        from = "Column::UserId",
        to = "super::users::Column::Id",
        on_update = "Cascade",
        on_delete = "Cascade"
    )]
    Users,
    #[sea_orm(
        belongs_to = "super::videos::Entity",
        from = "Column::VideoId",
        to = "super::videos::Column::Id",
        on_update = "Cascade",
        on_delete = "Cascade"
    )]
    Videos,
}

impl Related<super::users::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Users.def()
    }
}

impl Related<super::videos::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Videos.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
