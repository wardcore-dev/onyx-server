use axum::extract::{Extension, Path, State};
use axum::Json;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::models::ban::BanRequest;
use crate::models::member::Role;
use crate::server::AppState;

/// Get a member's role from the single-group members table
fn get_member_role(conn: &rusqlite::Connection, username: &str) -> Result<Role, AppError> {
    let role_str: String = conn
        .prepare("SELECT role FROM members WHERE username = ?1")?
        .query_row(rusqlite::params![username], |r| r.get(0))
        .map_err(|_| AppError::NotFound("Not a member of this group".into()))?;
    Role::from_str(&role_str).ok_or(AppError::Internal("Invalid role in database".into()))
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
    let _role = get_member_role(&conn, &username)?;

    let mut stmt = conn.prepare(
        "SELECT username, display_name, role, joined_at FROM members ORDER BY joined_at"
    )?;

    let members: Vec<Value> = stmt
        .query_map([], |row| {
            Ok(json!({
                "username": row.get::<_, String>(0)?,
                "display_name": row.get::<_, String>(1)?,
                "role": row.get::<_, String>(2)?,
                "joined_at": row.get::<_, String>(3)?,
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

        let my_role = get_member_role(&conn, &username)?;
        if !my_role.can_moderate() {
            return Err(AppError::Forbidden("Insufficient permissions".into()));
        }

        let target_role = get_member_role(&conn, &target)?;
        if target_role.is_owner() {
            return Err(AppError::Forbidden("Cannot kick the owner".into()));
        }
        if target_role.can_moderate() && !my_role.is_owner() {
            return Err(AppError::Forbidden("Only the owner can kick moderators".into()));
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

        let my_role = get_member_role(&conn, &username)?;
        if !my_role.can_moderate() {
            return Err(AppError::Forbidden("Insufficient permissions".into()));
        }

        if let Ok(target_role) = get_member_role(&conn, &target) {
            if target_role.is_owner() {
                return Err(AppError::Forbidden("Cannot ban the owner".into()));
            }
            if target_role.can_moderate() && !my_role.is_owner() {
                return Err(AppError::Forbidden("Only the owner can ban moderators".into()));
            }
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

        let my_role = get_member_role(&conn, &username)?;
        if !my_role.can_moderate() {
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
                "INSERT OR REPLACE INTO members (username, display_name, public_key, role) VALUES (?1, ?2, ?3, 'member')",
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

    // Only moderators and owners can view the ban list
    let my_role = get_member_role(&conn, &username)?;
    if !my_role.can_moderate() {
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

pub async fn set_role(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(target_username): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0.clone();
    let target = target_username.clone();
    let new_role_str = body["role"].as_str()
        .ok_or(AppError::BadRequest("role required".into()))?;
    let new_role = Role::from_str(new_role_str)
        .ok_or(AppError::BadRequest("role must be: owner, moderator, or member".into()))?;

    // Save the role string for later use
    let role_str = new_role.as_str().to_string();
    let db = state.db.clone();

    // Perform database operations in a blocking task
    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        let conn = db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

        let my_role = get_member_role(&conn, &username)?;
        if !my_role.is_owner() {
            return Err(AppError::Forbidden("Only owners can change roles".into()));
        }

        // Get current role of target user
        let target_current_role = get_member_role(&conn, &target)?;

        // If promoting to owner, check we don't exceed 3 owners
        if new_role.is_owner() && !target_current_role.is_owner() {
            let owner_count: i64 = conn
                .prepare("SELECT COUNT(*) FROM members WHERE role = 'owner'")?
                .query_row([], |r| r.get(0))
                .unwrap_or(0);

            if owner_count >= 3 {
                return Err(AppError::BadRequest("Maximum 3 owners allowed".into()));
            }
        }

        // If demoting an owner, ensure at least 1 owner remains
        if target_current_role.is_owner() && !new_role.is_owner() {
            let owner_count: i64 = conn
                .prepare("SELECT COUNT(*) FROM members WHERE role = 'owner'")?
                .query_row([], |r| r.get(0))
                .unwrap_or(0);

            if owner_count <= 1 {
                return Err(AppError::BadRequest("Cannot demote the last owner".into()));
            }
        }

        let updated = conn.execute(
            "UPDATE members SET role = ?1 WHERE username = ?2",
            rusqlite::params![new_role.as_str(), target],
        )?;

        if updated == 0 {
            return Err(AppError::NotFound("User is not a member of this group".into()));
        }

        Ok(())
    }).await.map_err(|_| AppError::Internal("DB task failed".into()))??;

    // Send role_changed event to the user whose role was changed
    let packet_role_changed = json!({
        "type": "role_changed",
        "group_id": 1,
        "role": &role_str,
    });
    state.hub.send_to_user(&target_username, &packet_role_changed).await;

    Ok(Json(json!({ "ok": true, "role": role_str })))
}
