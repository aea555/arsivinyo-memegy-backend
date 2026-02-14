use std::time::Duration;

use futures_util::StreamExt;
use shared::queue::UserSignalEnvelope;

use crate::state::AppState;

pub fn spawn_realtime_subscriber(state: AppState) {
    tokio::spawn(async move {
        loop {
            let pubsub = state.queue.subscribe_video_status_events().await;
            let mut pubsub = match pubsub {
                Ok(ps) => ps,
                Err(e) => {
                    tracing::error!("Realtime pubsub subscribe failed: {:?}", e);
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
            };

            tracing::info!("Realtime pubsub subscriber connected");
            let mut stream = pubsub.on_message();
            while let Some(msg) = stream.next().await {
                let payload = match msg.get_payload::<String>() {
                    Ok(value) => value,
                    Err(e) => {
                        tracing::warn!("Invalid realtime pubsub payload: {:?}", e);
                        continue;
                    }
                };

                let envelope = match serde_json::from_str::<UserSignalEnvelope>(&payload) {
                    Ok(value) => value,
                    Err(e) => {
                        tracing::warn!("Invalid realtime envelope: {:?}", e);
                        continue;
                    }
                };

                state
                    .realtime_hub
                    .fanout_to_user(envelope.user_id, &envelope.payload)
                    .await;
            }

            tracing::warn!("Realtime pubsub stream disconnected, retrying");
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
}
