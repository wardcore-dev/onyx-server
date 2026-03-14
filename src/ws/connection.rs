use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, Query, State};
use axum::response::Response;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tracing::info;
use std::sync::Arc;

use crate::auth;
use crate::db::Db;
use crate::server::AppState;
use crate::ws::hub::Hub;
use crate::ws::protocol::ClientMessage;

#[derive(Deserialize)]
pub struct WsQuery {
    pub token: String,
}

pub async fn ws_upgrade(
    State(state): State<AppState>,
    Query(query): Query<WsQuery>,
    ws: axum::extract::ws::WebSocketUpgrade,
) -> Result<Response, axum::http::StatusCode> {
    let username = auth::resolve_token(&state.db, &query.token)
        .ok_or(axum::http::StatusCode::UNAUTHORIZED)?;

    let db = state.db.clone();
    let hub = state.hub.clone();
    Ok(ws.on_upgrade(move |socket| handle_socket(socket, username, db, hub)))
}

async fn handle_socket(socket: WebSocket, username: String, db: Db, hub: Hub) {
    let (mut ws_sink, mut ws_stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    hub.register(&username, tx.clone()).await;
    info!("[ws] {} connected", username);

    let init_msg = json!({"type": "init_complete"}).to_string();
    let _ = tx.send(init_msg);

    let username_clone = username.clone();
    let hub_clone = hub.clone();
    let tx_clone = tx.clone();
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if ws_sink.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
        hub_clone.unregister(&username_clone, &tx_clone).await;
        info!("[ws] {} send task ended", username_clone);
    });

    let hub_recv = hub.clone();
    let db_recv = db.clone();
    let username_recv = username.clone();
    let tx_recv = tx.clone();
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_stream.next().await {
            match msg {
                Message::Text(text) => {
                    handle_client_message(&text, &username_recv, &db_recv, &hub_recv, &tx_recv).await;
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        hub_recv.unregister(&username_recv, &tx_recv).await;
        info!("[ws] {} recv task ended", username_recv);
    });

    tokio::select! {
        _ = send_task => {},
        _ = recv_task => {},
    }
}

async fn handle_client_message(
    text: &str,
    username: &str,
    db: &Db,
    hub: &Hub,
    tx: &mpsc::UnboundedSender<String>,
) {
    let msg: ClientMessage = match serde_json::from_str(text) {
        Ok(m) => m,
        Err(e) => {
            let err = json!({"type": "error", "message": format!("Invalid message: {}", e)});
            let _ = tx.send(err.to_string());
            return;
        }
    };

    match msg {
        ClientMessage::Ping => {
            let _ = tx.send(json!({"type": "pong"}).to_string());
        }

        ClientMessage::SubscribeGroup { group_id: _ } => {
            // Single-group: verify membership without group_id in query
            let is_member = {
                let conn = match db.lock() {
                    Ok(c) => c,
                    Err(_) => return,
                };
                conn.prepare("SELECT COUNT(*) FROM members WHERE username = ?1")
                    .ok()
                    .and_then(|mut s| {
                        s.query_row(rusqlite::params![username], |r| r.get::<_, i64>(0)).ok()
                    })
                    .map(|c| c > 0)
                    .unwrap_or(false)
            };

            if is_member {
                hub.subscribe(username).await;
            } else {
                let err = json!({"type": "error", "message": "Not a member of this group"});
                let _ = tx.send(err.to_string());
            }
        }

        ClientMessage::UnsubscribeGroup { group_id: _ } => {
            hub.unsubscribe(username).await;
        }

        ClientMessage::Typing { group_id } => {
            let packet = json!({
                "type": "typing",
                "group_id": group_id,
                "username": username,
            });
            hub.broadcast_to_subscribed(&packet).await;
        }

        ClientMessage::Presence { state } => {
            let packet = json!({
                "type": "presence",
                "from": username,
                "state": state,
                "is_online": state != "offline",
            });
            hub.broadcast_to_all(db, &packet).await;
        }
    }
}

/// WebSocket handler for public channels (read-only, no authentication required)
pub async fn ws_public_upgrade(
    State(state): State<AppState>,
    Path(public_token): Path<String>,
    ws: axum::extract::ws::WebSocketUpgrade,
) -> Result<Response, axum::http::StatusCode> {
    // Verify the public token exists and is for a channel
    let is_valid = {
        let conn = state.db.lock().map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
        conn.prepare("SELECT COUNT(*) FROM group_info WHERE id = 1 AND public_channel_token = ?1 AND is_channel = 1")
            .ok()
            .and_then(|mut s| s.query_row([&public_token], |r| r.get::<_, i64>(0)).ok())
            .map(|c| c > 0)
            .unwrap_or(false)
    };

    if !is_valid {
        info!("[ws-public] Invalid public token: {}", public_token);
        return Err(axum::http::StatusCode::NOT_FOUND);
    }

    // Generate anonymous username for this connection
    let anon_username = format!("public:anon:{}", uuid::Uuid::new_v4());
    let db = state.db.clone();
    let hub = state.hub.clone();

    info!("[ws-public] Public channel connection: {}", public_token);
    Ok(ws.on_upgrade(move |socket| handle_public_socket(socket, anon_username, public_token, db, hub)))
}

async fn handle_public_socket(
    socket: WebSocket,
    anon_username: String,
    public_token: String,
    db: Db,
    hub: Hub,
) {
    let (mut ws_sink, mut ws_stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    hub.register(&anon_username, tx.clone()).await;
    info!("[ws-public] {} connected to channel {}", anon_username, public_token);

    // Auto-subscribe to receive messages
    hub.subscribe(&anon_username).await;

    let init_msg = json!({"type": "init_complete", "readonly": true}).to_string();
    let _ = tx.send(init_msg);

    let anon_username_clone = anon_username.clone();
    let hub_clone = hub.clone();
    let tx_clone = tx.clone();
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if ws_sink.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
        hub_clone.unregister(&anon_username_clone, &tx_clone).await;
        info!("[ws-public] {} send task ended", anon_username_clone);
    });

    let hub_recv = hub.clone();
    let anon_username_recv = anon_username.clone();
    let tx_recv = tx.clone();
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_stream.next().await {
            match msg {
                Message::Text(text) => {
                    // Public connections are read-only, ignore all client messages except ping
                    handle_public_client_message(&text, &anon_username_recv, &tx_recv).await;
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        hub_recv.unregister(&anon_username_recv, &tx_recv).await;
        info!("[ws-public] {} recv task ended", anon_username_recv);
    });

    tokio::select! {
        _ = send_task => {},
        _ = recv_task => {},
    }
}

async fn handle_public_client_message(
    text: &str,
    _anon_username: &str,
    tx: &mpsc::UnboundedSender<String>,
) {
    let msg: ClientMessage = match serde_json::from_str(text) {
        Ok(m) => m,
        Err(e) => {
            let err = json!({"type": "error", "message": format!("Invalid message: {}", e)});
            let _ = tx.send(err.to_string());
            return;
        }
    };

    match msg {
        ClientMessage::Ping => {
            let _ = tx.send(json!({"type": "pong"}).to_string());
        }
        // All other messages are ignored for public (read-only) connections
        _ => {
            let err = json!({"type": "error", "message": "Read-only connection"});
            let _ = tx.send(err.to_string());
        }
    }
}
