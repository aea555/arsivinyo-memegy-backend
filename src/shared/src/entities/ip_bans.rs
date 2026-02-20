//! `SeaORM` Entity

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "ip_bans")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub target: String,
    pub target_kind: String,
    pub ip_family: i16,
    pub cidr_prefix: Option<i16>,
    pub reason: Option<String>,
    pub banned_until: Option<DateTimeWithTimeZone>,
    pub created_by_admin_sub: String,
    pub created_at: DateTimeWithTimeZone,
    pub lifted_at: Option<DateTimeWithTimeZone>,
    pub lifted_by_admin_sub: Option<String>,
    pub lift_reason: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
