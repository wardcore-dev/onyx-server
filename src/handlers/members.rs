use axum::extract::{Extension, Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::models::ban::BanRequest;
use crate::models::permission::Permission;
use crate::permissions::{count_owner_equivalent_members, has_any_permission, has_permission, is_owner_equivalent};
use crate::server::AppState;

/// True if the target's membership row exists at all (used to give a clean 404
/// rather than an opaque permission failure for a non-member target).
fn is_member(conn: &rusqlite::Connection, username: &str) -> Result<bool, AppError> {
    conn.prepare("SELECT COUNT(*) FROM members WHERE username = ?1")?
        .query_row(rusqlite::params![username], |r| r.get::<_, i64>(0))
        .map(|c| c > 0)
        .map_err(|e| AppError::Internal(e.to_string()))
}

pub async fn check_ban_status(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;
    let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

    // Check if user is banned
    let ban_info = conn
        .prepare("SELECT banned_by, reason, banned_at FROM bans WHERE username = ?1")?
        .query_row(rusqlite::params![username], |row| {
            Ok(json!({
                "banned": true,
                "banned_by": row.get::<_, String>(0)?,
                "reason": row.get::<_, Option<String>>(1)?,
                "banned_at": row.get::<_, String>(2)?,
            }))
        });

    match ban_info {
        Ok(info) => Ok(Json(info)),
        Err(_) => Ok(Json(json!({"banned": false}))),
    }
}

pub async fn list_members(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;
    let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

    // Check membership
    if !is_member(&conn, &username)? {
        return Err(AppError::NotFound("Not a member of this group".into()));
    }

    let mut stmt = conn.prepare(
        "SELECT m.username, m.display_name, m.role, m.joined_at, m.role_id, r.name, r.color, mu.expires_at
         FROM members m LEFT JOIN roles r ON r.id = m.role_id
         LEFT JOIN mutes mu ON mu.username = m.username
         ORDER BY m.joined_at"
    )?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let members: Vec<Value> = stmt
        .query_map([], |row| {
            let expires_at: Option<i64> = row.get(7)?;
            let is_muted = expires_at.is_some_and(|e| e > now);
            Ok(json!({
                "username": row.get::<_, String>(0)?,
                "display_name": row.get::<_, String>(1)?,
                "role": row.get::<_, String>(2)?,
                "joined_at": row.get::<_, String>(3)?,
                "role_id": row.get::<_, Option<i64>>(4)?,
                "role_name": row.get::<_, Option<String>>(5)?,
                "role_color": row.get::<_, Option<String>>(6)?,
                "is_muted": is_muted,
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(Json(json!(members)))
}

pub async fn kick_member(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(target_username): Path<String>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0.clone();
    let target = target_username.clone();
    let db = state.db.clone();

    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        let conn = db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

        if !has_permission(&conn, &username, Permission::KICK_MEMBERS)? {
            return Err(AppError::Forbidden("Insufficient permissions".into()));
        }

        if !is_member(&conn, &target)? {
            return Err(AppError::NotFound("Not a member of this group".into()));
        }

        // A target holding the system Owner role can only be kicked by another
        // Owner-equivalent member — generalizes the old "can't kick the owner" rule.
        if is_owner_equivalent(&conn, &target)? && !is_owner_equivalent(&conn, &username)? {
            return Err(AppError::Forbidden("Cannot kick an owner".into()));
        }

        conn.execute(
            "DELETE FROM members WHERE username = ?1",
            rusqlite::params![target],
        )?;

        Ok(())
    }).await.map_err(|_| AppError::Internal("DB task failed".into()))??;

    // Broadcast to all members that someone left/was kicked
    let packet_broadcast = json!({
        "type": "group_member_left",
        "group_id": 1,
        "username": target_username,
    });
    state.hub.broadcast_to_all_subscribed(&packet_broadcast).await;

    // Send specific "kicked" notification to the kicked user
    let packet_kicked = json!({
        "type": "kicked",
        "group_id": 1,
    });
    state.hub.send_to_user(&target_username, &packet_kicked).await;

    Ok(Json(json!({ "ok": true })))
}

pub async fn ban_member(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(target_username): Path<String>,
    Json(req): Json<BanRequest>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0.clone();
    let target = target_username.clone();
    let reason = req.reason.clone();
    let db = state.db.clone();

    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        let conn = db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

        if !has_any_permission(&conn, &username, &[Permission::BAN_MEMBERS, Permission::VIEW_BAN_LIST])? {
            return Err(AppError::Forbidden("Insufficient permissions".into()));
        }

        if is_member(&conn, &target)? && is_owner_equivalent(&conn, &target)? && !is_owner_equivalent(&conn, &username)? {
            return Err(AppError::Forbidden("Cannot ban an owner".into()));
        }

        conn.execute(
            "DELETE FROM members WHERE username = ?1",
            rusqlite::params![target],
        )?;

        conn.execute(
            "INSERT OR REPLACE INTO bans (username, banned_by, reason) VALUES (?1, ?2, ?3)",
            rusqlite::params![target, username, reason],
        )?;

        Ok(())
    }).await.map_err(|_| AppError::Internal("DB task failed".into()))??;

    // Broadcast to all members that someone was banned
    let packet_broadcast = json!({
        "type": "group_member_left",
        "group_id": 1,
        "username": target_username,
    });
    state.hub.broadcast_to_all_subscribed(&packet_broadcast).await;

    // Send specific "banned" notification to the banned user
    let packet_banned = json!({
        "type": "banned",
        "group_id": 1,
        "reason": req.reason,
    });
    state.hub.send_to_user(&target_username, &packet_banned).await;

    Ok(Json(json!({ "ok": true })))
}

pub async fn unban_member(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(target_username): Path<String>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0.clone();
    let target = target_username.clone();
    let db = state.db.clone();

    // Perform database operations in a blocking task
    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        let conn = db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

        if !has_any_permission(&conn, &username, &[Permission::BAN_MEMBERS, Permission::VIEW_BAN_LIST])? {
            return Err(AppError::Forbidden("Insufficient permissions".into()));
        }

        // Remove from bans table
        conn.execute(
            "DELETE FROM bans WHERE username = ?1",
            rusqlite::params![target],
        )?;

        // Get user info from sessions to re-add to members
        let user_info: Result<(String, String), _> = conn
            .prepare("SELECT display_name, public_key FROM sessions WHERE username = ?1")?
            .query_row(rusqlite::params![target], |row| {
                Ok((row.get(0)?, row.get(1)?))
            });

        // Re-add user to members with default role
        if let Ok((display_name, public_key)) = user_info {
            conn.execute(
                "INSERT OR REPLACE INTO members (username, display_name, public_key, role, role_id) VALUES (?1, ?2, ?3, 'member', 3)",
                rusqlite::params![target, display_name, public_key],
            )?;
        }

        Ok(())
    }).await.map_err(|_| AppError::Internal("DB task failed".into()))??;

    // Send unbanned event to the user
    let packet_unbanned = json!({
        "type": "unbanned",
        "group_id": 1,
    });
    state.hub.send_to_user(&target_username, &packet_unbanned).await;

    Ok(Json(json!({ "ok": true })))
}

pub async fn list_bans(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;
    let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

    if !has_permission(&conn, &username, Permission::VIEW_BAN_LIST)? {
        return Err(AppError::Forbidden("Insufficient permissions".into()));
    }

    let mut stmt = conn.prepare(
        "SELECT username, banned_by, reason, banned_at FROM bans ORDER BY banned_at DESC"
    )?;

    let bans: Vec<Value> = stmt
        .query_map([], |row| {
            Ok(json!({
                "username": row.get::<_, String>(0)?,
                "banned_by": row.get::<_, String>(1)?,
                "reason": row.get::<_, Option<String>>(2)?,
                "banned_at": row.get::<_, String>(3)?,
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(Json(json!(bans)))
}

#[derive(Deserialize)]
pub struct SetRoleRequest {
    pub role_id: i64,
}

pub async fn set_role(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(target_username): Path<String>,
    Json(body): Json<SetRoleRequest>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0.clone();
    let target = target_username.clone();
    let new_role_id = body.role_id;
    let db = state.db.clone();

    let (role_name, role_color, role_permissions) = tokio::task::spawn_blocking(move || -> Result<(String, String, i64), AppError> {
        let conn = db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

        if !has_permission(&conn, &username, Permission::MANAGE_ROLES)? {
            return Err(AppError::Forbidden("Insufficient permissions".into()));
        }

        let (role_name, role_color, role_permissions): (String, String, i64) = conn
            .prepare("SELECT name, color, permissions FROM roles WHERE id = ?1")?
            .query_row(rusqlite::params![new_role_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(|_| AppError::BadRequest("Unknown role_id".into()))?;

        if !is_member(&conn, &target)? {
            return Err(AppError::NotFound("User is not a member of this group".into()));
        }

        // Promoting to the system Owner role requires the caller to already be an Owner.
        if new_role_id == 1 && !is_owner_equivalent(&conn, &username)? {
            return Err(AppError::Forbidden("Only owners can promote a member to Owner".into()));
        }

        // Demoting an existing Owner away from role_id=1 must always leave at least one Owner.
        if is_owner_equivalent(&conn, &target)? && new_role_id != 1 {
            if count_owner_equivalent_members(&conn)? <= 1 {
                return Err(AppError::BadRequest("Cannot demote the last owner".into()));
            }
        }

        let updated = conn.execute(
            "UPDATE members SET role_id = ?1, role = ?2 WHERE username = ?3",
            rusqlite::params![new_role_id, role_name.to_lowercase(), target],
        )?;

        if updated == 0 {
            return Err(AppError::NotFound("User is not a member of this group".into()));
        }

        Ok((role_name, role_color, role_permissions))
    }).await.map_err(|_| AppError::Internal("DB task failed".into()))??;

    // Send role_changed event to the user whose role was changed
    let packet_role_changed = json!({
        "type": "role_changed",
        "group_id": 1,
        "role_id": new_role_id,
        "role_name": &role_name,
        "role_color": &role_color,
        "permissions": Permission::from_bits_truncate(role_permissions).to_names(),
    });
    state.hub.send_to_user(&target_username, &packet_role_changed).await;

    // Also broadcast a lightweight group-wide notice so every connected
    // client can keep its username->role-color map (used for chat nickname
    // coloring) fresh without having to reload the member list.
    let packet_member_role_updated = json!({
        "type": "member_role_updated",
        "username": &target_username,
        "role_name": &role_name,
        "role_color": &role_color,
    });
    state.hub.broadcast_to_all_subscribed(&packet_member_role_updated).await;

    Ok(Json(json!({
        "ok": true,
        "role_id": new_role_id,
        "role_name": role_name,
        "role_color": role_color,
    })))
}
