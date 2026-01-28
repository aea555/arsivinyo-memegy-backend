use redis::{Client, AsyncCommands};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use anyhow::Result;

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
        conn.lpush::<_, _, ()>("video_processing_queue", job_json).await?;
        Ok(())
    }
    
    // For Worker
    pub async fn pop_video_job(&self) -> Result<Option<VideoProcessJob>> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        // BRPOP blocks until an item is available. 0 timeout means infinite.
        // Returns (key, value)
        let result: Option<(String, String)> = conn.brpop("video_processing_queue", 0.0).await?;
        
        match result {
            Some((_, job_json)) => {
                let job: VideoProcessJob = serde_json::from_str(&job_json)?;
                Ok(Some(job))
            }
            None => Ok(None),
        }
    }
}
