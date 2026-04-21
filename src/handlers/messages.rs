use axum::extract::{Extension, Path, Query, State};
use axum::Json;
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
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

    // Attach reactions
    let ids: Vec<i64> = messages.iter()
        .filter_map(|m| m.get("id").and_then(|v| v.as_i64()))
        .collect();
    let reactions_map = load_reactions_map(&conn, &ids);
    drop(conn);
    let messages: Vec<Value> = messages.into_iter().map(|mut m| {
        if let Some(id) = m.get("id").and_then(|v| v.as_i64()) {
            let r = reactions_map.get(&id).cloned().unwrap_or(json!({}));
            m.as_object_mut().unwrap().insert("reactions".to_string(), r);
        }
        m
    }).collect();

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

    if !state.msg_rate_limiter.check_and_record(&username) {
        return Err(AppError::BadRequest(format!(
            "Rate limit exceeded: max {} messages per minute",
            state.config.security.max_messages_per_minute
        )));
    }

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

    if !state.join_rate_limiter.check_and_record(&username) {
        return Err(AppError::BadRequest(format!(
            "Rate limit exceeded: max {} join attempts per minute",
            state.config.security.max_joins_per_minute
        )));
    }

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
    let mut messages = messages;
    messages.reverse();
    let ids: Vec<i64> = messages.iter()
        .filter_map(|m| m.get("id").and_then(|v| v.as_i64()))
        .collect();
    let reactions_map = load_reactions_map(&conn, &ids);
    drop(conn);
    let messages: Vec<Value> = messages.into_iter().map(|mut m| {
        if let Some(id) = m.get("id").and_then(|v| v.as_i64()) {
            let r = reactions_map.get(&id).cloned().unwrap_or(json!({}));
            m.as_object_mut().unwrap().insert("reactions".to_string(), r);
        }
        m
    }).collect();
    Ok(Json(json!(messages)))
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

// ─── Reactions ────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct AddReactionRequest {
    pub emoji: String,
}

fn is_valid_emoji(s: &str) -> bool {
    let trimmed = s.trim();
    if trimmed.is_empty() || trimmed.len() > 32 {
        return false;
    }
    trimmed.chars().any(|c| c as u32 > 127)
}

/// Load aggregated reactions for a list of message IDs.
/// Returns { emoji -> [username, ...] } per message_id.
pub fn load_reactions_map(
    conn: &Connection,
    message_ids: &[i64],
) -> HashMap<i64, Value> {
    if message_ids.is_empty() {
        return HashMap::new();
    }
    let placeholders: String = message_ids
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 1))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT message_id, emoji, reactor_username FROM message_reactions WHERE message_id IN ({}) ORDER BY message_id, emoji, created_at",
        placeholders
    );
    let mut map: HashMap<i64, HashMap<String, Vec<String>>> = HashMap::new();
    if let Ok(mut stmt) = conn.prepare(&sql) {
        let params: Vec<&dyn rusqlite::types::ToSql> = message_ids
            .iter()
            .map(|id| id as &dyn rusqlite::types::ToSql)
            .collect();
        if let Ok(rows) = stmt.query_map(params.as_slice(), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        }) {
            for row in rows.flatten() {
                map.entry(row.0)
                    .or_default()
                    .entry(row.1)
                    .or_default()
                    .push(row.2);
            }
        }
    }
    map.into_iter()
        .map(|(id, emoji_map)| {
            let v: serde_json::Map<String, Value> = emoji_map
                .into_iter()
                .map(|(emoji, users)| (emoji, Value::Array(users.into_iter().map(Value::String).collect())))
                .collect();
            (id, Value::Object(v))
        })
        .collect()
}

/// POST /groups/{group_id}/messages/{message_id}/reactions
pub async fn add_reaction(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path((_group_id, message_id)): Path<(i64, i64)>,
    Json(req): Json<AddReactionRequest>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;
    let emoji = req.emoji.trim().to_string();

    if !is_valid_emoji(&emoji) {
        return Err(AppError::BadRequest("Invalid emoji".into()));
    }

    let reactions = {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

        let is_member: bool = conn
            .prepare("SELECT COUNT(*) FROM members WHERE username = ?1")?
            .query_row([&username], |r| r.get::<_, i64>(0))
            .map(|c| c > 0)
            .unwrap_or(false);
        if !is_member {
            return Err(AppError::Forbidden("Not a member".into()));
        }

        let msg_exists: bool = conn
            .prepare("SELECT COUNT(*) FROM messages WHERE id = ?1")?
            .query_row([message_id], |r| r.get::<_, i64>(0))
            .map(|c| c > 0)
            .unwrap_or(false);
        if !msg_exists {
            return Err(AppError::NotFound("Message not found".into()));
        }

        conn.execute(
            "INSERT OR IGNORE INTO message_reactions (message_id, reactor_username, emoji) VALUES (?1, ?2, ?3)",
            rusqlite::params![message_id, username, emoji],
        )?;

        let map = load_reactions_map(&conn, &[message_id]);
        map.get(&message_id).cloned().unwrap_or(json!({}))
    };

    let packet = json!({
        "type": "reaction_update",
        "group_id": 1,
        "message_id": message_id,
        "reactions": reactions,
        "actor": username,
        "emoji": emoji,
        "action": "add",
    });
    state.hub.broadcast_to_all_subscribed(&packet).await;

    Ok(Json(json!({ "ok": true, "reactions": reactions })))
}

/// DELETE /groups/{group_id}/messages/{message_id}/reactions/{emoji}
pub async fn remove_reaction(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path((_group_id, message_id, emoji_raw)): Path<(i64, i64, String)>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;
    let emoji = emoji_raw.trim().to_string();

    if !is_valid_emoji(&emoji) {
        return Err(AppError::BadRequest("Invalid emoji".into()));
    }

    let reactions = {
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
            "DELETE FROM message_reactions WHERE message_id = ?1 AND reactor_username = ?2 AND emoji = ?3",
            rusqlite::params![message_id, username, emoji],
        )?;

        let map = load_reactions_map(&conn, &[message_id]);
        map.get(&message_id).cloned().unwrap_or(json!({}))
    };

    let packet = json!({
        "type": "reaction_update",
        "group_id": 1,
        "message_id": message_id,
        "reactions": reactions,
        "actor": username,
        "emoji": emoji,
        "action": "remove",
    });
    state.hub.broadcast_to_all_subscribed(&packet).await;

    Ok(Json(json!({ "ok": true, "reactions": reactions })))
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
