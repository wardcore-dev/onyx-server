use axum::extract::{Extension, Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::models::permission::Permission;
use crate::permissions::{has_any_permission, has_permission, is_owner_equivalent};
use crate::server::AppState;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

// ─── Mute ───────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct MuteRequest {
    pub reason: Option<String>,
    /// Mute duration in minutes. Required — there is no "forever" mute; use a
    /// large number of minutes if a long mute is intended.
    pub duration_minutes: i64,
}

/// POST /members/:username/mute - Mute a member for N minutes (moderator+)
pub async fn mute_member(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(target_username): Path<String>,
    Json(req): Json<MuteRequest>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0.clone();
    let target = target_username.clone();
    let reason = req.reason.clone();

    if req.duration_minutes <= 0 {
        return Err(AppError::BadRequest("duration_minutes must be positive".into()));
    }
    if req.duration_minutes > 60 * 24 * 365 {
        return Err(AppError::BadRequest("duration_minutes too large (max 1 year)".into()));
    }

    let expires_at = now_ms() + req.duration_minutes * 60_000;
    let db = state.db.clone();
    let actor = username.clone();

    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        let conn = db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

        if !has_any_permission(&conn, &actor, &[Permission::MUTE_MEMBERS, Permission::VIEW_MUTE_LIST])? {
            return Err(AppError::Forbidden("Insufficient permissions".into()));
        }

        if is_owner_equivalent(&conn, &target)? && !is_owner_equivalent(&conn, &actor)? {
            return Err(AppError::Forbidden("Cannot mute an owner".into()));
        }

        conn.execute(
            "INSERT OR REPLACE INTO mutes (username, muted_by, reason, expires_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![target, actor, reason, expires_at],
        )?;

        Ok(())
    }).await.map_err(|_| AppError::Internal("DB task failed".into()))??;

    let packet = json!({
        "type": "member_muted",
        "group_id": 1,
        "username": target_username,
        "muted_by": username,
        "expires_at": expires_at,
        "reason": req.reason,
    });
    state.hub.broadcast_to_all_subscribed(&packet).await;

    Ok(Json(json!({ "ok": true, "expires_at": expires_at })))
}

/// POST /members/:username/unmute - Lift a mute early (moderator+)
pub async fn unmute_member(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(target_username): Path<String>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0.clone();
    let target = target_username.clone();
    let db = state.db.clone();
    let actor = username.clone();

    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        let conn = db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

        if !has_any_permission(&conn, &actor, &[Permission::MUTE_MEMBERS, Permission::VIEW_MUTE_LIST])? {
            return Err(AppError::Forbidden("Insufficient permissions".into()));
        }

        conn.execute("DELETE FROM mutes WHERE username = ?1", rusqlite::params![target])?;

        Ok(())
    }).await.map_err(|_| AppError::Internal("DB task failed".into()))??;

    let packet = json!({
        "type": "member_unmuted",
        "group_id": 1,
        "username": target_username,
        "unmuted_by": username,
    });
    state.hub.broadcast_to_all_subscribed(&packet).await;

    Ok(Json(json!({ "ok": true })))
}

/// GET /mutes - List currently active mutes (moderator+)
pub async fn list_mutes(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;
    let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

    if !has_permission(&conn, &username, Permission::VIEW_MUTE_LIST)? {
        return Err(AppError::Forbidden("Insufficient permissions".into()));
    }

    let now = now_ms();
    let mut stmt = conn.prepare(
        "SELECT username, muted_by, reason, muted_at, expires_at FROM mutes WHERE expires_at > ?1 ORDER BY expires_at DESC"
    )?;
    let mutes: Vec<Value> = stmt
        .query_map(rusqlite::params![now], |row| {
            Ok(json!({
                "username": row.get::<_, String>(0)?,
                "muted_by": row.get::<_, String>(1)?,
                "reason": row.get::<_, Option<String>>(2)?,
                "muted_at": row.get::<_, String>(3)?,
                "expires_at": row.get::<_, i64>(4)?,
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(Json(json!(mutes)))
}

/// Returns Some(expires_at_ms) if the user is currently muted (and the mute hasn't expired).
/// Expired mute rows are lazily cleaned up.
pub fn active_mute_expiry(conn: &rusqlite::Connection, username: &str) -> Option<i64> {
    let expires_at: Option<i64> = conn
        .prepare("SELECT expires_at FROM mutes WHERE username = ?1")
        .ok()?
        .query_row(rusqlite::params![username], |r| r.get(0))
        .ok();

    match expires_at {
        Some(exp) if exp > now_ms() => Some(exp),
        Some(_) => {
            let _ = conn.execute("DELETE FROM mutes WHERE username = ?1", rusqlite::params![username]);
            None
        }
        None => None,
    }
}

// ─── Slow mode ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SlowModeRequest {
    /// Seconds between messages for regular (non-moderator) members. 0 disables slow mode.
    pub seconds: i64,
}

/// POST /groups/:id/slow-mode - Set slow mode delay (owner/moderator)
pub async fn set_slow_mode(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(group_id): Path<i64>,
    Json(req): Json<SlowModeRequest>,
) -> Result<Json<Value>, AppError> {
    if group_id != 1 {
        return Err(AppError::NotFound("Group not found".into()));
    }
    if req.seconds < 0 || req.seconds > 6 * 60 * 60 {
        return Err(AppError::BadRequest("seconds must be between 0 and 21600".into()));
    }

    let username = auth.0;
    {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;
        if !has_permission(&conn, &username, Permission::MANAGE_SLOW_MODE)? {
            return Err(AppError::Forbidden("Insufficient permissions".into()));
        }

        conn.execute(
            "UPDATE group_info SET slow_mode_seconds = ?1 WHERE id = 1",
            rusqlite::params![req.seconds],
        )?;
    }

    let packet = json!({
        "type": "slow_mode_updated",
        "group_id": 1,
        "seconds": req.seconds,
        "updated_by": username,
    });
    state.hub.broadcast_to_all_subscribed(&packet).await;

    Ok(Json(json!({ "ok": true, "seconds": req.seconds })))
}

