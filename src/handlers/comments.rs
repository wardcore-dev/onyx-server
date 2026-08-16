use axum::extract::{Extension, Path, State};
use axum::Json;
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::handlers::moderation_handlers::active_mute_expiry;
use crate::models::permission::Permission;
use crate::permissions::has_permission;
use crate::server::AppState;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// True if `group_info` is currently in channel mode. Comments are a
/// channel-only concept (a plain group already lets everyone reply inline).
fn is_channel(conn: &rusqlite::Connection) -> bool {
    conn.query_row("SELECT is_channel FROM group_info WHERE id = 1", [], |r| r.get(0))
        .unwrap_or(false)
}

/// Per-post summary for the comment badge under a channel post bubble: total
/// count plus a preview of the most recent comment (sender + content).
pub struct CommentPreview {
    pub count: i64,
    pub last_sender: Option<String>,
    pub last_content: Option<String>,
}

/// Load comment counts + last-comment preview for a batch of post ids, for
/// the comment badge on channel post bubbles. Missing ids simply have no
/// entry (treat as 0 comments).
pub fn load_comment_previews(conn: &Connection, post_ids: &[i64]) -> HashMap<i64, CommentPreview> {
    if post_ids.is_empty() {
        return HashMap::new();
    }
    let placeholders: String = post_ids
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 1))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT c.post_id, COUNT(*),
                (SELECT sender_username FROM comments c2 WHERE c2.post_id = c.post_id ORDER BY c2.timestamp_ms DESC, c2.id DESC LIMIT 1),
                (SELECT content FROM comments c2 WHERE c2.post_id = c.post_id ORDER BY c2.timestamp_ms DESC, c2.id DESC LIMIT 1)
         FROM comments c WHERE c.post_id IN ({}) GROUP BY c.post_id",
        placeholders
    );
    let mut map = HashMap::new();
    if let Ok(mut stmt) = conn.prepare(&sql) {
        let params: Vec<&dyn rusqlite::types::ToSql> = post_ids
            .iter()
            .map(|id| id as &dyn rusqlite::types::ToSql)
            .collect();
        if let Ok(rows) = stmt.query_map(params.as_slice(), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        }) {
            for row in rows.flatten() {
                map.insert(row.0, CommentPreview { count: row.1, last_sender: row.2, last_content: row.3 });
            }
        }
    }
    map
}

/// Load aggregated reactions for a batch of comment ids — same shape and
/// anonymity rules as messages.rs::load_reactions_map.
pub fn load_comment_reactions_map(
    conn: &Connection,
    comment_ids: &[i64],
    viewer_username: Option<&str>,
) -> HashMap<i64, Value> {
    if comment_ids.is_empty() {
        return HashMap::new();
    }
    let placeholders: String = comment_ids
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 1))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT comment_id, emoji, reactor_username FROM comment_reactions WHERE comment_id IN ({}) ORDER BY comment_id, emoji, created_at",
        placeholders
    );
    let mut map: HashMap<i64, HashMap<String, (i64, bool)>> = HashMap::new();
    if let Ok(mut stmt) = conn.prepare(&sql) {
        let params: Vec<&dyn rusqlite::types::ToSql> = comment_ids
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
                let entry = map.entry(row.0).or_default().entry(row.1).or_insert((0, false));
                entry.0 += 1;
                if viewer_username == Some(row.2.as_str()) {
                    entry.1 = true;
                }
            }
        }
    }
    map.into_iter()
        .map(|(id, emoji_map)| {
            let v: serde_json::Map<String, Value> = emoji_map
                .into_iter()
                .map(|(emoji, (count, reacted_by_me))| {
                    (emoji, json!({ "count": count, "reactedByMe": reacted_by_me }))
                })
                .collect();
            (id, Value::Object(v))
        })
        .collect()
}

fn is_valid_emoji(s: &str) -> bool {
    let trimmed = s.trim();
    if trimmed.is_empty() || trimmed.len() > 32 {
        return false;
    }
    trimmed.chars().any(|c| c as u32 > 127)
}

/// GET /messages/:id/comments - list a post's comment thread (any member)
pub async fn list_comments(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(post_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;
    let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

    let is_member: bool = conn
        .prepare("SELECT COUNT(*) FROM members WHERE username = ?1")?
        .query_row([&username], |r| r.get::<_, i64>(0))
        .map(|c| c > 0)
        .unwrap_or(false);
    if !is_member {
        return Err(AppError::Forbidden("Not a member of this group".into()));
    }

    let post_exists: bool = conn
        .prepare("SELECT COUNT(*) FROM messages WHERE id = ?1")?
        .query_row([post_id], |r| r.get::<_, i64>(0))
        .map(|c| c > 0)
        .unwrap_or(false);
    if !post_exists {
        return Err(AppError::NotFound("Post not found".into()));
    }

    let mut stmt = conn.prepare(
        "SELECT id, sender_username, content, reply_to_id, reply_to_sender, reply_to_content, timestamp, timestamp_ms
         FROM comments WHERE post_id = ?1
         ORDER BY timestamp_ms ASC, id ASC"
    )?;
    let mut comments: Vec<Value> = stmt
        .query_map([post_id], |row| {
            Ok(json!({
                "id": row.get::<_, i64>(0)?,
                "post_id": post_id,
                "sender": row.get::<_, String>(1)?,
                "content": row.get::<_, String>(2)?,
                "reply_to_id": row.get::<_, Option<i64>>(3)?,
                "reply_to_sender": row.get::<_, Option<String>>(4)?,
                "reply_to_content": row.get::<_, Option<String>>(5)?,
                "timestamp": row.get::<_, String>(6)?,
                "timestamp_ms": row.get::<_, i64>(7)?,
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();

    let ids: Vec<i64> = comments.iter().filter_map(|c| c.get("id").and_then(|v| v.as_i64())).collect();
    let reactions_map = load_comment_reactions_map(&conn, &ids, Some(username.as_str()));
    for c in comments.iter_mut() {
        if let Some(id) = c.get("id").and_then(|v| v.as_i64()) {
            let r = reactions_map.get(&id).cloned().unwrap_or(json!({}));
            c.as_object_mut().unwrap().insert("reactions".to_string(), r);
        }
    }

    Ok(Json(json!(comments)))
}

#[derive(Deserialize)]
pub struct AddCommentReactionRequest {
    pub emoji: String,
}

/// POST /comments/:id/reactions
pub async fn add_comment_reaction(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(comment_id): Path<i64>,
    Json(req): Json<AddCommentReactionRequest>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;
    let emoji = req.emoji.trim().to_string();
    if !is_valid_emoji(&emoji) {
        return Err(AppError::BadRequest("Invalid emoji".into()));
    }

    let (post_id, reactions) = {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

        let is_member: bool = conn
            .prepare("SELECT COUNT(*) FROM members WHERE username = ?1")?
            .query_row([&username], |r| r.get::<_, i64>(0))
            .map(|c| c > 0)
            .unwrap_or(false);
        if !is_member {
            return Err(AppError::Forbidden("Not a member".into()));
        }

        let post_id: i64 = conn
            .query_row("SELECT post_id FROM comments WHERE id = ?1", [comment_id], |r| r.get(0))
            .map_err(|_| AppError::NotFound("Comment not found".into()))?;

        // Same 2-distinct-emoji cap as message reactions (see messages.rs
        // add_reaction).
        let already_has_this: bool = conn
            .prepare("SELECT COUNT(*) FROM comment_reactions WHERE comment_id = ?1 AND reactor_username = ?2 AND emoji = ?3")?
            .query_row(rusqlite::params![comment_id, username, emoji], |r| r.get::<_, i64>(0))
            .map(|c| c > 0)
            .unwrap_or(false);
        if !already_has_this {
            let distinct_count: i64 = conn
                .prepare("SELECT COUNT(DISTINCT emoji) FROM comment_reactions WHERE comment_id = ?1 AND reactor_username = ?2")?
                .query_row(rusqlite::params![comment_id, username], |r| r.get(0))
                .unwrap_or(0);
            if distinct_count >= 2 {
                return Err(AppError::BadRequest("You can react with at most 2 different emojis per comment".into()));
            }
        }

        conn.execute(
            "INSERT OR IGNORE INTO comment_reactions (comment_id, reactor_username, emoji) VALUES (?1, ?2, ?3)",
            rusqlite::params![comment_id, username, emoji],
        )?;

        let map = load_comment_reactions_map(&conn, &[comment_id], Some(username.as_str()));
        (post_id, map.get(&comment_id).cloned().unwrap_or(json!({})))
    };

    let anon_reactions = {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;
        load_comment_reactions_map(&conn, &[comment_id], None)
            .get(&comment_id)
            .cloned()
            .unwrap_or(json!({}))
    };
    let packet = json!({
        "type": "comment_reaction_update",
        "group_id": 1,
        "post_id": post_id,
        "comment_id": comment_id,
        "reactions": anon_reactions,
        "emoji": emoji,
        "action": "add",
    });
    state.hub.broadcast_to_all_subscribed(&packet).await;

    Ok(Json(json!({ "ok": true, "reactions": reactions })))
}

/// DELETE /comments/:id/reactions/:emoji
pub async fn remove_comment_reaction(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path((comment_id, emoji_raw)): Path<(i64, String)>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;
    let emoji = emoji_raw.trim().to_string();
    if !is_valid_emoji(&emoji) {
        return Err(AppError::BadRequest("Invalid emoji".into()));
    }

    let (post_id, reactions) = {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

        let is_member: bool = conn
            .prepare("SELECT COUNT(*) FROM members WHERE username = ?1")?
            .query_row([&username], |r| r.get::<_, i64>(0))
            .map(|c| c > 0)
            .unwrap_or(false);
        if !is_member {
            return Err(AppError::Forbidden("Not a member".into()));
        }

        let post_id: i64 = conn
            .query_row("SELECT post_id FROM comments WHERE id = ?1", [comment_id], |r| r.get(0))
            .map_err(|_| AppError::NotFound("Comment not found".into()))?;

        conn.execute(
            "DELETE FROM comment_reactions WHERE comment_id = ?1 AND reactor_username = ?2 AND emoji = ?3",
            rusqlite::params![comment_id, username, emoji],
        )?;

        let map = load_comment_reactions_map(&conn, &[comment_id], Some(username.as_str()));
        (post_id, map.get(&comment_id).cloned().unwrap_or(json!({})))
    };

    let anon_reactions = {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;
        load_comment_reactions_map(&conn, &[comment_id], None)
            .get(&comment_id)
            .cloned()
            .unwrap_or(json!({}))
    };
    let packet = json!({
        "type": "comment_reaction_update",
        "group_id": 1,
        "post_id": post_id,
        "comment_id": comment_id,
        "reactions": anon_reactions,
        "emoji": emoji,
        "action": "remove",
    });
    state.hub.broadcast_to_all_subscribed(&packet).await;

    Ok(Json(json!({ "ok": true, "reactions": reactions })))
}

#[derive(Deserialize)]
pub struct SendCommentRequest {
    pub content: String,
    pub reply_to_id: Option<i64>,
}

#[derive(Deserialize)]
pub struct EditCommentRequest {
    pub content: String,
}

/// POST /messages/:id/comments - add a comment to a channel post (any member;
/// muted members are blocked like regular messages)
pub async fn create_comment(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(post_id): Path<i64>,
    Json(req): Json<SendCommentRequest>,
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
        return Err(AppError::BadRequest("Comment content required".into()));
    }
    if content.len() > state.config.server.max_message_length as usize {
        return Err(AppError::BadRequest(format!(
            "Comment too long (max {} chars)", state.config.server.max_message_length
        )));
    }

    let (comment_id, timestamp, timestamp_ms, reply_to_sender, reply_to_content) = {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

        let is_member: bool = conn
            .prepare("SELECT COUNT(*) FROM members WHERE username = ?1")?
            .query_row([&username], |r| r.get::<_, i64>(0))
            .map(|c| c > 0)
            .unwrap_or(false);
        if !is_member {
            return Err(AppError::Forbidden("Not a member of this group".into()));
        }

        if let Some(expires_at) = active_mute_expiry(&conn, &username) {
            return Err(AppError::Forbidden(format!("You are muted until {}", expires_at)));
        }

        if !is_channel(&conn) {
            return Err(AppError::Forbidden("Comments are only available on channel posts".into()));
        }

        let post_exists: bool = conn
            .prepare("SELECT COUNT(*) FROM messages WHERE id = ?1")?
            .query_row([post_id], |r| r.get::<_, i64>(0))
            .map(|c| c > 0)
            .unwrap_or(false);
        if !post_exists {
            return Err(AppError::NotFound("Post not found".into()));
        }

        let (reply_to_sender, reply_to_content): (Option<String>, Option<String>) =
            if let Some(reply_id) = req.reply_to_id {
                conn.prepare("SELECT sender_username, content FROM comments WHERE id = ?1 AND post_id = ?2")?
                    .query_row(rusqlite::params![reply_id, post_id], |r| Ok((r.get(0)?, r.get(1)?)))
                    .ok()
                    .unwrap_or((None, None))
            } else {
                (None, None)
            };

        let timestamp_ms = now_ms();
        let timestamp = chrono::Utc::now().to_rfc3339();

        conn.execute(
            "INSERT INTO comments (post_id, sender_username, content, reply_to_id, reply_to_sender, reply_to_content, timestamp, timestamp_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![post_id, username, content, req.reply_to_id, reply_to_sender, reply_to_content, timestamp, timestamp_ms],
        )?;

        (conn.last_insert_rowid(), timestamp, timestamp_ms, reply_to_sender, reply_to_content)
    };

    let comment = json!({
        "id": comment_id,
        "post_id": post_id,
        "sender": username,
        "content": content,
        "reply_to_id": req.reply_to_id,
        "reply_to_sender": reply_to_sender,
        "reply_to_content": reply_to_content,
        "timestamp": timestamp,
        "timestamp_ms": timestamp_ms,
    });

    let packet = json!({
        "type": "comment_added",
        "group_id": 1,
        "post_id": post_id,
        "comment": comment,
    });
    state.hub.broadcast_to_all_subscribed(&packet).await;

    Ok(Json(comment))
}

/// PATCH /comments/:id - edit own comment. No "edit any comment" permission,
/// mirroring edit_message in messages.rs.
pub async fn edit_comment(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(comment_id): Path<i64>,
    Json(req): Json<EditCommentRequest>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;
    let new_content = req.content.trim().to_string();

    if new_content.is_empty() {
        return Err(AppError::BadRequest("Content cannot be empty".into()));
    }
    if new_content.len() > state.config.server.max_message_length as usize {
        return Err(AppError::BadRequest(format!(
            "Comment too long (max {} chars)", state.config.server.max_message_length
        )));
    }

    let post_id = {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

        let is_member: bool = conn
            .prepare("SELECT COUNT(*) FROM members WHERE username = ?1")?
            .query_row([&username], |r| r.get::<_, i64>(0))
            .map(|c| c > 0)
            .unwrap_or(false);
        if !is_member {
            return Err(AppError::Forbidden("Not a member".into()));
        }

        let post_id: i64 = conn
            .query_row("SELECT post_id FROM comments WHERE id = ?1", [comment_id], |r| r.get(0))
            .map_err(|_| AppError::NotFound("Comment not found".into()))?;

        let rows_affected = conn.execute(
            "UPDATE comments SET content = ?1 WHERE id = ?2 AND sender_username = ?3",
            rusqlite::params![new_content, comment_id, username],
        )?;
        if rows_affected == 0 {
            return Err(AppError::NotFound("Comment not found or not yours".into()));
        }

        post_id
    };

    let packet = json!({
        "type": "comment_edited",
        "group_id": 1,
        "post_id": post_id,
        "comment_id": comment_id,
        "new_content": new_content,
        "edited_by": username,
    });
    state.hub.broadcast_to_all_subscribed(&packet).await;

    Ok(Json(json!({ "ok": true })))
}

/// DELETE /comments/:id - delete own comment, or any comment for
/// DELETE_MESSAGES holders
pub async fn delete_comment(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(comment_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;

    let post_id = {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

        let is_member: bool = conn
            .prepare("SELECT COUNT(*) FROM members WHERE username = ?1")?
            .query_row([&username], |r| r.get::<_, i64>(0))
            .map(|c| c > 0)
            .unwrap_or(false);
        if !is_member {
            return Err(AppError::Forbidden("Not a member".into()));
        }

        let post_id: i64 = conn
            .query_row("SELECT post_id FROM comments WHERE id = ?1", [comment_id], |r| r.get(0))
            .map_err(|_| AppError::NotFound("Comment not found".into()))?;

        let rows_affected = if has_permission(&conn, &username, Permission::DELETE_MESSAGES)? {
            conn.execute("DELETE FROM comments WHERE id = ?1", [comment_id])?
        } else {
            conn.execute(
                "DELETE FROM comments WHERE id = ?1 AND sender_username = ?2",
                rusqlite::params![comment_id, username],
            )?
        };

        if rows_affected == 0 {
            return Err(AppError::NotFound("Comment not found or not yours".into()));
        }

        post_id
    };

    let packet = json!({
        "type": "comment_deleted",
        "group_id": 1,
        "post_id": post_id,
        "comment_id": comment_id,
    });
    state.hub.broadcast_to_all_subscribed(&packet).await;

    Ok(Json(json!({ "ok": true })))
}
