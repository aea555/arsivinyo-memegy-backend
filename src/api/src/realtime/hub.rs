use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, mpsc};
use uuid::Uuid;

use crate::metrics::{
    WS_ACTIVE_CONNECTIONS, WS_CONNECTION_DROPPED_TOTAL, WS_CONNECTION_REJECTED_TOTAL,
    WS_EVENTS_FANOUT_TOTAL,
};

#[derive(Debug)]
pub enum HubRegisterError {
    GlobalLimitExceeded,
    PerUserLimitExceeded,
}

#[derive(Clone)]
pub struct RealtimeHub {
    inner: Arc<Mutex<HubState>>,
    max_conn_per_user: usize,
    max_conn_global: usize,
    send_buffer: usize,
}

#[derive(Debug)]
struct HubState {
    users: HashMap<Uuid, HashMap<Uuid, ConnectionEntry>>,
}

#[derive(Debug)]
struct ConnectionEntry {
    sender: mpsc::Sender<String>,
    bootstrapping: bool,
    pending: Vec<String>,
}

impl RealtimeHub {
    pub fn new(max_conn_per_user: usize, max_conn_global: usize, send_buffer: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HubState {
                users: HashMap::new(),
            })),
            max_conn_per_user,
            max_conn_global,
            send_buffer,
        }
    }

    pub async fn register_connection(
        &self,
        user_id: Uuid,
    ) -> Result<(Uuid, mpsc::Receiver<String>), HubRegisterError> {
        let mut state = self.inner.lock().await;

        let total = total_connections(&state.users);
        if total >= self.max_conn_global {
            WS_CONNECTION_REJECTED_TOTAL.inc();
            return Err(HubRegisterError::GlobalLimitExceeded);
        }

        let user_entry = state.users.entry(user_id).or_default();
        if user_entry.len() >= self.max_conn_per_user {
            WS_CONNECTION_REJECTED_TOTAL.inc();
            return Err(HubRegisterError::PerUserLimitExceeded);
        }

        let conn_id = Uuid::new_v4();
        let (tx, rx) = mpsc::channel(self.send_buffer);
        user_entry.insert(
            conn_id,
            ConnectionEntry {
                sender: tx,
                bootstrapping: true,
                pending: Vec::new(),
            },
        );
        WS_ACTIVE_CONNECTIONS.set(total_connections(&state.users) as i64);
        Ok((conn_id, rx))
    }

    pub async fn mark_bootstrapped_and_take_pending(
        &self,
        user_id: Uuid,
        conn_id: Uuid,
    ) -> Vec<String> {
        let mut state = self.inner.lock().await;
        if let Some(connections) = state.users.get_mut(&user_id) {
            if let Some(conn) = connections.get_mut(&conn_id) {
                conn.bootstrapping = false;
                return std::mem::take(&mut conn.pending);
            }
        }
        Vec::new()
    }

    pub async fn remove_connection(&self, user_id: Uuid, conn_id: Uuid) {
        let mut state = self.inner.lock().await;
        if let Some(connections) = state.users.get_mut(&user_id) {
            connections.remove(&conn_id);
            if connections.is_empty() {
                state.users.remove(&user_id);
            }
        }
        WS_ACTIVE_CONNECTIONS.set(total_connections(&state.users) as i64);
    }

    pub async fn fanout_to_user(&self, user_id: Uuid, payload: &str) {
        let mut send_targets: Vec<(Uuid, mpsc::Sender<String>)> = Vec::new();
        let mut stale_ids: Vec<Uuid> = Vec::new();

        {
            let mut state = self.inner.lock().await;
            let mut became_empty = false;
            if let Some(connections) = state.users.get_mut(&user_id) {
                for (conn_id, entry) in connections.iter_mut() {
                    if entry.bootstrapping {
                        if entry.pending.len() >= self.send_buffer {
                            stale_ids.push(*conn_id);
                        } else {
                            entry.pending.push(payload.to_string());
                        }
                        continue;
                    }
                    send_targets.push((*conn_id, entry.sender.clone()));
                }

                for stale in &stale_ids {
                    connections.remove(stale);
                }
                became_empty = connections.is_empty();
            }

            if became_empty {
                state.users.remove(&user_id);
            }
            WS_ACTIVE_CONNECTIONS.set(total_connections(&state.users) as i64);
        }

        if !stale_ids.is_empty() {
            WS_CONNECTION_DROPPED_TOTAL.inc_by(stale_ids.len() as u64);
        }

        let mut failed_ids: Vec<Uuid> = Vec::new();
        for (conn_id, sender) in send_targets {
            match sender.try_send(payload.to_string()) {
                Ok(()) => WS_EVENTS_FANOUT_TOTAL.inc(),
                Err(_) => failed_ids.push(conn_id),
            }
        }

        if !failed_ids.is_empty() {
            WS_CONNECTION_DROPPED_TOTAL.inc_by(failed_ids.len() as u64);
            let mut state = self.inner.lock().await;
            if let Some(connections) = state.users.get_mut(&user_id) {
                for conn_id in &failed_ids {
                    connections.remove(conn_id);
                }
                if connections.is_empty() {
                    state.users.remove(&user_id);
                }
            }
            WS_ACTIVE_CONNECTIONS.set(total_connections(&state.users) as i64);
        }
    }
}

fn total_connections(users: &HashMap<Uuid, HashMap<Uuid, ConnectionEntry>>) -> usize {
    users.values().map(HashMap::len).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bootstrapping_connection_buffers_then_flushes() {
        let hub = RealtimeHub::new(3, 10, 8);
        let user_id = Uuid::new_v4();
        let (conn_id, mut rx) = hub.register_connection(user_id).await.unwrap();

        hub.fanout_to_user(user_id, "{\"type\":\"video.status.processing\"}")
            .await;
        assert!(rx.try_recv().is_err());

        let pending = hub
            .mark_bootstrapped_and_take_pending(user_id, conn_id)
            .await;
        assert_eq!(pending.len(), 1);
    }

    #[tokio::test]
    async fn live_connection_receives_fanout() {
        let hub = RealtimeHub::new(3, 10, 8);
        let user_id = Uuid::new_v4();
        let (conn_id, mut rx) = hub.register_connection(user_id).await.unwrap();

        let _ = hub
            .mark_bootstrapped_and_take_pending(user_id, conn_id)
            .await;

        let payload = "{\"type\":\"video.status.published\"}";
        hub.fanout_to_user(user_id, payload).await;
        let received = rx.recv().await.expect("message should be delivered");
        assert_eq!(received, payload);
    }
}
