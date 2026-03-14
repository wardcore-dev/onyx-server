use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use serde_json::Value;

use crate::db::Db;

type Tx = mpsc::UnboundedSender<String>;

#[derive(Clone)]
pub struct Hub {
    inner: Arc<Mutex<HubInner>>,
}

struct HubInner {
    connections: HashMap<String, Vec<Tx>>,
    subscribed: HashSet<String>,
}

impl Hub {
    pub fn new() -> Self {
        Hub {
            inner: Arc::new(Mutex::new(HubInner {
                connections: HashMap::new(),
                subscribed: HashSet::new(),
            })),
        }
    }

    pub async fn register(&self, username: &str, tx: Tx) {
        let mut inner = self.inner.lock().await;
        inner.connections
            .entry(username.to_string())
            .or_default()
            .push(tx);
    }

    pub async fn unregister(&self, username: &str, tx: &Tx) {
        let mut inner = self.inner.lock().await;
        if let Some(senders) = inner.connections.get_mut(username) {
            senders.retain(|s| !s.same_channel(tx));
            if senders.is_empty() {
                inner.connections.remove(username);
                inner.subscribed.remove(username);
            }
        }
    }

    pub async fn subscribe(&self, username: &str) {
        let mut inner = self.inner.lock().await;
        inner.subscribed.insert(username.to_string());
    }

    pub async fn unsubscribe(&self, username: &str) {
        let mut inner = self.inner.lock().await;
        inner.subscribed.remove(username);
    }

    pub async fn send_to_user(&self, username: &str, message: &Value) {
        let text = message.to_string();
        let inner = self.inner.lock().await;
        if let Some(senders) = inner.connections.get(username) {
            for tx in senders {
                let _ = tx.send(text.clone());
            }
        }
    }

    /// Broadcast to all DB members who are connected
    pub async fn broadcast_to_all(&self, db: &Db, message: &Value) {
        let members: Vec<String> = {
            let conn = match db.lock() {
                Ok(c) => c,
                Err(_) => return,
            };
            let mut stmt = match conn.prepare("SELECT username FROM members") {
                Ok(s) => s,
                Err(_) => return,
            };
            stmt.query_map([], |r| r.get::<_, String>(0))
                .ok()
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
                .unwrap_or_default()
        };

        let text = message.to_string();
        let inner = self.inner.lock().await;

        for member in &members {
            if let Some(senders) = inner.connections.get(member) {
                for tx in senders {
                    let _ = tx.send(text.clone());
                }
            }
        }
    }

    /// Broadcast only to WebSocket-subscribed users
    pub async fn broadcast_to_subscribed(&self, message: &Value) {
        let text = message.to_string();
        let inner = self.inner.lock().await;

        for username in &inner.subscribed {
            if let Some(senders) = inner.connections.get(username) {
                for tx in senders {
                    let _ = tx.send(text.clone());
                }
            }
        }
    }

    pub async fn online_count(&self) -> usize {
        let inner = self.inner.lock().await;
        inner.connections.len()
    }

    /// Broadcast to all subscribed users (including anonymous/public channel viewers)
    /// This is different from broadcast_to_all which only sends to DB members
    pub async fn broadcast_to_all_subscribed(&self, message: &Value) {
        let text = message.to_string();
        let inner = self.inner.lock().await;

        for username in &inner.subscribed {
            if let Some(senders) = inner.connections.get(username) {
                for tx in senders {
                    let _ = tx.send(text.clone());
                }
            }
        }
    }
}
