use anyhow::Result;
use redis::{AsyncCommands, Client};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::Config;

#[derive(Serialize, Deserialize, Debug)]
pub struct VideoProcessJob {
    pub video_id: Uuid,
    pub user_id: Uuid,
    pub raw_bucket: String,
    pub raw_key: String,
}

#[derive(Clone)]
pub struct QueueService {
    client: Client,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UserSignalEnvelope {
    pub user_id: Uuid,
    pub payload: String,
}

pub const VIDEO_STATUS_EVENTS_CHANNEL: &str = "video_status_events";

impl QueueService {
    pub fn new(config: &Config) -> Result<Self> {
        let client = Client::open(config.valkey_url.as_str())?;
        Ok(Self { client })
    }

    pub async fn get_conn(&self) -> Result<redis::aio::MultiplexedConnection> {
        Ok(self.client.get_multiplexed_async_connection().await?)
    }

    pub async fn push_video_job(&self, job: VideoProcessJob) -> Result<()> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        let job_json = serde_json::to_string(&job)?;
        conn.lpush::<_, _, ()>("video_processing_queue", job_json)
            .await?;
        Ok(())
    }

    // For Worker
    pub async fn pop_video_job(&self) -> Result<Option<VideoProcessJob>> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        // Use a finite BRPOP timeout so idle polling returns `None`
        // instead of surfacing client/network read timeout errors.
        // Returns (key, value)
        let result: Option<(String, String)> = conn.brpop("video_processing_queue", 5.0).await?;

        match result {
            Some((_, job_json)) => {
                let job: VideoProcessJob = serde_json::from_str(&job_json)?;
                Ok(Some(job))
            }
            None => Ok(None),
        }
    }

    pub async fn publish_video_status_event(&self, user_id: Uuid, payload: &str) -> Result<()> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        let envelope = UserSignalEnvelope {
            user_id,
            payload: payload.to_string(),
        };
        let msg = serde_json::to_string(&envelope)?;
        conn.publish::<_, _, ()>(VIDEO_STATUS_EVENTS_CHANNEL, msg)
            .await?;
        Ok(())
    }

    pub async fn subscribe_video_status_events(&self) -> Result<redis::aio::PubSub> {
        let mut pubsub = self.client.get_async_pubsub().await?;
        pubsub.subscribe(VIDEO_STATUS_EVENTS_CHANNEL).await?;
        Ok(pubsub)
    }
}
