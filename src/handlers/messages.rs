use axum::extract::{Extension, Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::models::message::SendMessageRequest;
use crate::server::AppState;

#[derive(Deserialize)]
pub struct EditMessageRequest {
    pub content: String,
}

#[derive(Deserialize)]
pub struct HistoryQuery {
    pub before_id: Option<i64>,
    pub limit: Option<i64>,
}

/// GET /group/{group_id}/history - client uses this route
pub async fn get_group_history(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(_group_id): Path<i64>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Value>, AppError> {
    get_history_inner(state, auth, query).await
}

/// GET /history - legacy route
pub async fn get_history(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Value>, AppError> {
    get_history_inner(state, auth, query).await
}

async fn get_history_inner(
    state: AppState,
    auth: AuthUser,
    query: HistoryQuery,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;
    let limit = query.limit.unwrap_or(50).min(200);

    let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

    let is_member: bool = conn
        .prepare("SELECT COUNT(*) FROM members WHERE username = ?1")?
        .query_row([&username], |r| r.get::<_, i64>(0))
        .map(|c| c > 0)
        .unwrap_or(false);

    if !is_member {
        return Err(AppError::Forbidden("Not a member of this group".into()));
    }

    let messages: Vec<Value> = if let Some(before_id) = query.before_id {
        let mut stmt = conn.prepare(
            "SELECT id, sender_username, content, reply_to_id, reply_to_sender, reply_to_content, timestamp, timestamp_ms
             FROM messages WHERE id < ?1
             ORDER BY id DESC LIMIT ?2"
        )?;
        let rows = stmt.query_map(rusqlite::params![before_id, limit], |row| {
            Ok(json!({
                "id": row.get::<_, i64>(0)?,
                "sender": row.get::<_, String>(1)?,
                "content": row.get::<_, String>(2)?,
                "reply_to_id": row.get::<_, Option<i64>>(3)?,
                "reply_to_sender": row.get::<_, Option<String>>(4)?,
                "reply_to_content": row.get::<_, Option<String>>(5)?,
                "timestamp": row.get::<_, String>(6)?,
                "timestamp_ms": row.get::<_, i64>(7)?,
            }))
        })?;
        rows.filter_map(|r| r.ok()).collect()
    } else {
        let mut stmt = conn.prepare(
            "SELECT id, sender_username, content, reply_to_id, reply_to_sender, reply_to_content, timestamp, timestamp_ms
             FROM messages
             ORDER BY id DESC LIMIT ?1"
        )?;
        let rows = stmt.query_map([limit], |row| {
            Ok(json!({
                "id": row.get::<_, i64>(0)?,
                "sender": row.get::<_, String>(1)?,
                "content": row.get::<_, String>(2)?,
                "reply_to_id": row.get::<_, Option<i64>>(3)?,
                "reply_to_sender": row.get::<_, Option<String>>(4)?,
                "reply_to_content": row.get::<_, Option<String>>(5)?,
                "timestamp": row.get::<_, String>(6)?,
                "timestamp_ms": row.get::<_, i64>(7)?,
            }))
        })?;
        rows.filter_map(|r| r.ok()).collect()
    };

    let mut messages = messages;
    messages.reverse();

    Ok(Json(json!(messages)))
}

/// POST /group/{group_id}/send - client uses this route
pub async fn send_group_message(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(_group_id): Path<i64>,
    Json(req): Json<SendMessageRequest>,
) -> Result<Json<Value>, AppError> {
    send_message_inner(state, auth, req).await
}

/// POST /send - legacy route
pub async fn send_message(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Json(req): Json<SendMessageRequest>,
) -> Result<Json<Value>, AppError> {
    send_message_inner(state, auth, req).await
}

async fn send_message_inner(
    state: AppState,
    auth: AuthUser,
    req: SendMessageRequest,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;
    let content = req.content.trim().to_string();

    if content.is_empty() {
        return Err(AppError::BadRequest("Message content required".into()));
    }
    if content.len() > state.config.server.max_message_length as usize {
        return Err(AppError::BadRequest(format!(
            "Message too long (max {} chars)", state.config.server.max_message_length
        )));
    }

    let (message_id, timestamp, timestamp_ms, reply_to_id, reply_to_sender, reply_to_content, sender) = {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

        let is_member: bool = conn
            .prepare("SELECT COUNT(*) FROM members WHERE username = ?1")?
            .query_row([&username], |r| r.get::<_, i64>(0))
            .map(|c| c > 0)
            .unwrap_or(false);

        if !is_member {
            return Err(AppError::Forbidden("Not a member of this group".into()));
        }

        // Check if this is a channel - channels post messages from channel name, not user
        let (is_channel, channel_name): (bool, String) = conn
            .prepare("SELECT is_channel, name FROM group_info WHERE id = 1")?
            .query_row([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap_or((false, String::new()));

        // For channels, use channel name as sender; for groups, use username
        let sender = if is_channel {
            channel_name
        } else {
            username.clone()
        };

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let timestamp_ms = now.as_millis() as i64;
        let timestamp = chrono::Utc::now().to_rfc3339();

        let (reply_to_id, reply_to_sender, reply_to_content) = if let Some(ref_id) = req.reply_to_id {
            let snapshot: Option<(Option<String>, Option<String>)> = conn
                .prepare("SELECT sender_username, content FROM messages WHERE id = ?1")?
                .query_row([ref_id], |r| {
                    Ok((Some(r.get::<_, String>(0)?), Some(r.get::<_, String>(1)?)))
                })
                .ok();

            match snapshot {
                Some((sender, content_snap)) => (Some(ref_id), sender, content_snap),
                None => (Some(ref_id), req.reply_to_sender.clone(), req.reply_to_content.clone()),
            }
        } else {
            (None, None, None)
        };

        conn.execute(
            "INSERT INTO messages (sender_username, content, reply_to_id, reply_to_sender, reply_to_content, timestamp, timestamp_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![sender, content, reply_to_id, reply_to_sender, reply_to_content, timestamp, timestamp_ms],
        )?;

        let message_id = conn.last_insert_rowid();

        (message_id, timestamp, timestamp_ms, reply_to_id, reply_to_sender, reply_to_content, sender)
    };

    // Broadcast via WebSocket - client expects "group_msg" type
    let packet = json!({
        "type": "group_msg",
        "group_id": 1,
        "message_id": message_id,
        "from": sender.clone(),
        "sender": sender,
        "content": content,
        "timestamp": timestamp,
        "timestamp_ms": timestamp_ms,
        "reply_to_id": reply_to_id,
        "reply_to_sender": reply_to_sender,
        "reply_to_content": reply_to_content,
    });
    // Broadcast to all subscribed users (both authenticated and public viewers)
    // This avoids DB lock issues and prevents duplicate messages
    state.hub.broadcast_to_all_subscribed(&packet).await;

    Ok(Json(json!({
        "ok": true,
        "message_id": message_id,
        "timestamp": timestamp,
        "timestamp_ms": timestamp_ms,
    })))
}

/// POST /group/join/{invite_token}
pub async fn join_group(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(invite_token): Path<String>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;

    let (group_name, display_name) = {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

        // Verify invite token
        let group_exists: bool = conn
            .prepare("SELECT COUNT(*) FROM group_info WHERE id = 1 AND invite_token = ?1")?
            .query_row([&invite_token], |r| r.get::<_, i64>(0))
            .map(|c| c > 0)
            .unwrap_or(false);

        if !group_exists {
            return Err(AppError::NotFound("Invalid invite token".into()));
        }

        // Check if banned
        let is_banned: bool = conn
            .prepare("SELECT COUNT(*) FROM bans WHERE username = ?1")?
            .query_row([&username], |r| r.get::<_, i64>(0))
            .map(|c| c > 0)
            .unwrap_or(false);

        if is_banned {
            return Err(AppError::Forbidden("You are banned from this group".into()));
        }

        // Check if already member
        let is_member: bool = conn
            .prepare("SELECT COUNT(*) FROM members WHERE username = ?1")?
            .query_row([&username], |r| r.get::<_, i64>(0))
            .map(|c| c > 0)
            .unwrap_or(false);

        if is_member {
            return Err(AppError::Conflict("Already a member of this group".into()));
        }

        // Get display_name and public_key from session
        let (display_name, public_key): (String, String) = conn
            .prepare("SELECT display_name, public_key FROM sessions WHERE username = ?1")?
            .query_row([&username], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|_| AppError::Internal("Session not found".into()))?;

        conn.execute(
            "INSERT INTO members (username, display_name, public_key, role) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![username, display_name, public_key, state.config.moderation.default_role],
        )?;

        let group_name: String = conn
            .query_row("SELECT name FROM group_info WHERE id = 1", [], |r| r.get(0))
            .unwrap_or_else(|_| "Unknown".to_string());

        (group_name, display_name)
    };

    // Broadcast join
    let packet = json!({
        "type": "group_joined",
        "group_id": 1,
        "username": username,
        "display_name": display_name,
    });
    state.hub.broadcast_to_all_subscribed(&packet).await;

    Ok(Json(json!({
        "ok": true,
        "group_id": 1,
        "group_name": group_name,
    })))
}

/// POST /group/{group_id}/leave
pub async fn leave_group(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(_group_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;

    {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

        // Check if user is an owner
        let user_role: Option<String> = conn
            .prepare("SELECT role FROM members WHERE username = ?1")?
            .query_row([&username], |r| r.get(0))
            .ok();

        if user_role.as_deref() == Some("owner") {
            // Check if there are other owners
            let owner_count: i64 = conn
                .prepare("SELECT COUNT(*) FROM members WHERE role = 'owner'")?
                .query_row([], |r| r.get(0))
                .unwrap_or(0);

            if owner_count <= 1 {
                return Err(AppError::BadRequest("Last owner cannot leave the group".into()));
            }
        }

        let deleted = conn.execute(
            "DELETE FROM members WHERE username = ?1",
            rusqlite::params![username],
        )?;

        if deleted == 0 {
            return Err(AppError::NotFound("Not a member of this group".into()));
        }
    }

    let packet = json!({
        "type": "group_member_left",
        "group_id": 1,
        "username": username,
    });
    state.hub.broadcast_to_all_subscribed(&packet).await;

    Ok(Json(json!({ "ok": true })))
}

/// GET /channels/{public_token}/history - Public channel history (no auth required)
pub async fn get_public_channel_history(
    State(state): State<AppState>,
    Path(public_token): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = query.limit.unwrap_or(50).min(200);
    let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;
    let is_public_channel: bool = conn.prepare("SELECT COUNT(*) FROM group_info WHERE id = 1 AND public_channel_token = ?1 AND is_channel = 1").ok().and_then(|mut s| s.query_row([&public_token], |r| r.get::<_, i64>(0)).ok()).map(|c| c > 0).unwrap_or(false);
    if !is_public_channel { return Err(AppError::NotFound("Public channel not found".into())); }
    let messages: Vec<Value> = if let Some(before_id) = query.before_id { let mut stmt = conn.prepare("SELECT id, sender_username, content, reply_to_id, reply_to_sender, reply_to_content, timestamp, timestamp_ms FROM messages WHERE id < ?1 ORDER BY id DESC LIMIT ?2")?; let rows = stmt.query_map(rusqlite::params![before_id, limit], |row| Ok(json!({ "id": row.get::<_, i64>(0)?, "sender": row.get::<_, String>(1)?, "content": row.get::<_, String>(2)?, "reply_to_id": row.get::<_, Option<i64>>(3)?, "reply_to_sender": row.get::<_, Option<String>>(4)?, "reply_to_content": row.get::<_, Option<String>>(5)?, "timestamp": row.get::<_, String>(6)?, "timestamp_ms": row.get::<_, i64>(7)? })))?; rows.filter_map(|r| r.ok()).collect() } else { let mut stmt = conn.prepare("SELECT id, sender_username, content, reply_to_id, reply_to_sender, reply_to_content, timestamp, timestamp_ms FROM messages ORDER BY id DESC LIMIT ?1")?; let rows = stmt.query_map([limit], |row| Ok(json!({ "id": row.get::<_, i64>(0)?, "sender": row.get::<_, String>(1)?, "content": row.get::<_, String>(2)?, "reply_to_id": row.get::<_, Option<i64>>(3)?, "reply_to_sender": row.get::<_, Option<String>>(4)?, "reply_to_content": row.get::<_, Option<String>>(5)?, "timestamp": row.get::<_, String>(6)?, "timestamp_ms": row.get::<_, i64>(7)? })))?; rows.filter_map(|r| r.ok()).collect() };
    let mut messages = messages; messages.reverse(); Ok(Json(json!(messages)))
}

/// GET /my-role - Get current user's role
pub async fn get_my_role(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;
    let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

    let role: String = conn
        .prepare("SELECT role FROM members WHERE username = ?1")?
        .query_row([&username], |r| r.get(0))
        .unwrap_or_else(|_| "member".to_string());

    Ok(Json(json!({
        "ok": true,
        "role": role,
    })))
}

/// DELETE /groups/{group_id}/messages/{message_id} - Delete own message
pub async fn delete_message(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path((_group_id, message_id)): Path<(i64, i64)>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;

    let rows_affected = {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

        let is_member: bool = conn
            .prepare("SELECT COUNT(*) FROM members WHERE username = ?1")?
            .query_row([&username], |r| r.get::<_, i64>(0))
            .map(|c| c > 0)
            .unwrap_or(false);

        if !is_member {
            return Err(AppError::Forbidden("Not a member".into()));
        }

        conn.execute(
            "DELETE FROM messages WHERE id = ?1 AND sender_username = ?2",
            rusqlite::params![message_id, username],
        )?
    };

    if rows_affected == 0 {
        return Err(AppError::NotFound("Message not found or not yours".into()));
    }

    let packet = json!({
        "type": "group_msg_deleted",
        "group_id": 1,
        "message_id": message_id,
    });
    state.hub.broadcast_to_all_subscribed(&packet).await;

    Ok(Json(json!({ "ok": true })))
}

/// PATCH /groups/{group_id}/messages/{message_id} - Edit own message
pub async fn edit_message(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path((_group_id, message_id)): Path<(i64, i64)>,
    Json(req): Json<EditMessageRequest>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;
    let new_content = req.content.trim().to_string();

    if new_content.is_empty() {
        return Err(AppError::BadRequest("Content cannot be empty".into()));
    }

    let rows_affected = {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

        let is_member: bool = conn
            .prepare("SELECT COUNT(*) FROM members WHERE username = ?1")?
            .query_row([&username], |r| r.get::<_, i64>(0))
            .map(|c| c > 0)
            .unwrap_or(false);

        if !is_member {
            return Err(AppError::Forbidden("Not a member".into()));
        }

        conn.execute(
            "UPDATE messages SET content = ?1 WHERE id = ?2 AND sender_username = ?3",
            rusqlite::params![new_content, message_id, username],
        )?
    };

    if rows_affected == 0 {
        return Err(AppError::NotFound("Message not found or not yours".into()));
    }

    let packet = json!({
        "type": "group_msg_edited",
        "group_id": 1,
        "message_id": message_id,
        "new_content": new_content,
        "edited_by": username,
    });
    state.hub.broadcast_to_all_subscribed(&packet).await;

    Ok(Json(json!({ "ok": true })))
}
