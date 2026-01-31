use serde::{Deserialize, Serialize};
use shared::entities::videos;
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
pub struct UserVideoDto {
    pub id: Uuid,
    pub title: Option<String>,
    pub description: Option<String>,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::FixedOffset>,
    pub is_anonymous: bool,
    pub like_count: i64,
}

impl From<videos::Model> for UserVideoDto {
    fn from(model: videos::Model) -> Self {
        Self {
            id: model.id,
            title: model.title,
            description: model.description,
            status: model.status,
            created_at: model.created_at,
            is_anonymous: model.is_anonymous,
            like_count: model.like_count,
        }
    }
}
