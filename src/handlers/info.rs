use axum::extract::{Extension, Path, State};
use axum::Json;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::permissions::has_permission;
use crate::models::permission::Permission;
use crate::server::AppState;

/// GET /info - client expects "name" field for server name
pub async fn get_info(
    State(state): State<AppState>,
) -> Json<Value> {
    let (group_info, member_count) = {
        let conn = state.db.lock().unwrap();

        let info: Option<(String, String, String, i64, bool, Option<String>)> = conn
            .prepare(
                "SELECT name, description, invite_token, avatar_version, is_channel, public_channel_token FROM group_info WHERE id = 1"
            )
            .ok()
            .and_then(|mut stmt| {
                stmt.query_row([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, bool>(4)?,
                        r.get::<_, Option<String>>(5)?,
                    ))
                }).ok()
            });

        let member_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM members", [], |r| r.get(0))
            .unwrap_or(0);

        (info, member_count)
    };

    let mut features = vec!["messaging", "websocket"];
    if state.config.moderation.enable_moderators {
        features.push("moderation");
    }
    if state.config.media.provider != "none" {
        features.push("media_upload");
    }
    if state.config.media.provider == "local" {
        features.push("media_local");
    }
    if state.config.media.allowed_types.iter().any(|t| t == "image") {
        features.push("images");
    }
    if state.config.media.allowed_types.iter().any(|t| t == "video") {
        features.push("video");
    }
    if state.config.voice.enabled {
        features.push("voice");
    }
    features.push("custom_roles");

    if let Some((group_name, description, _invite_token, _avatar_version, is_channel, public_token)) = group_info {
        let vc = &state.config.voice;
        let mut response = json!({
            "ok": true,
            "name": state.config.server.name,
            "description": state.config.server.description,
            "version": env!("CARGO_PKG_VERSION"),
            "motd": state.config.server.motd,
            "media_provider": state.config.media.provider,
            "max_file_size_mb": state.config.media.max_file_size_mb,
            "max_message_length": state.config.server.max_message_length,
            "max_members_per_group": state.config.server.max_members_per_group,
            "features": features,
            "groups_count": 1,
            "total_members": member_count,
            "group_name": group_name,
            "group_description": description,
            "is_channel": is_channel,
            "voice_config": {
                "quality": vc.quality,
                "max_bitrate_bps": vc.effective_bitrate_bps(),
                "noise_suppression": vc.noise_suppression,
                "echo_cancellation": vc.echo_cancellation,
                "auto_gain_control": vc.auto_gain_control,
                "stereo": vc.stereo,
            },
        });

        // Add public_channel_token if available (for public channels)
        if let Some(token) = public_token {
            response["public_channel_token"] = json!(token);
        }

        Json(response)
    } else {
        Json(json!({
            "ok": false,
            "error": "Group not initialized",
            "name": state.config.server.name,
            "version": env!("CARGO_PKG_VERSION"),
        }))
    }
}

/// GET /groups - returns array of groups (single group)
pub async fn get_groups(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
) -> Json<Value> {
    let username = auth.0;

    let group_info = {
        let conn = state.db.lock().unwrap();

        let info: Option<(i64, String, bool, String, String, i64, i64)> = conn
            .prepare(
                "SELECT id, name, is_channel, owner_username, invite_token, avatar_version, slow_mode_seconds FROM group_info WHERE id = 1"
            )
            .ok()
            .and_then(|mut stmt| {
                stmt.query_row([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, bool>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, i64>(5)?,
                        r.get::<_, i64>(6)?,
                    ))
                }).ok()
            });

        let member_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM members", [], |r| r.get(0))
            .unwrap_or(0);

        // Get current user's role
        let user_role: String = conn
            .prepare("SELECT role FROM members WHERE username = ?1")
            .ok()
            .and_then(|mut stmt| stmt.query_row([&username], |r| r.get(0)).ok())
            .unwrap_or_else(|| "member".to_string());

        let can_see_invite = has_permission(&conn, &username, Permission::VIEW_INVITE_LINK).unwrap_or(false);

        let permissions_bits: Option<i64> = conn
            .query_row(
                "SELECT r.permissions FROM members m JOIN roles r ON r.id = m.role_id WHERE m.username = ?1",
                [&username],
                |r| r.get(0),
            )
            .ok();
        let my_permissions = permissions_bits
            .map(Permission::from_bits_truncate)
            .unwrap_or(Permission::empty())
            .to_names();

        (info, member_count, user_role, can_see_invite, my_permissions)
    };

    let (info, member_count, user_role, can_see_invite, my_permissions) = group_info;

    if let Some((id, name, is_channel, owner, invite_link, avatar_version, slow_mode_seconds)) = info {
        let mut group = json!({
            "id": id,
            "name": name,
            "is_channel": is_channel,
            "owner": owner,
            "avatar_version": avatar_version,
            "member_count": member_count,
            "my_role": user_role,
            "my_permissions": my_permissions,
            "slow_mode_seconds": slow_mode_seconds,
        });
        if can_see_invite {
            group["invite_link"] = json!(invite_link);
        }
        Json(json!([group]))
    } else {
        Json(json!([]))
    }
}

/// GET /group - single group info (kept for compatibility, unauthenticated)
/// invite_token is intentionally omitted — this endpoint is public.
pub async fn get_group(
    State(state): State<AppState>,
) -> Json<Value> {
    let group_info = {
        let conn = state.db.lock().unwrap();

        let info: Option<(String, String, i64, bool, String, i64)> = conn
            .prepare(
                "SELECT name, description, avatar_version, is_channel, owner_username, slow_mode_seconds FROM group_info WHERE id = 1"
            )
            .ok()
            .and_then(|mut stmt| {
                stmt.query_row([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, bool>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, i64>(5)?,
                    ))
                }).ok()
            });

        let member_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM members", [], |r| r.get(0))
            .unwrap_or(0);

        (info, member_count)
    };

    let (group_info, member_count) = group_info;

    if let Some((name, description, avatar_version, is_channel, owner, slow_mode_seconds)) = group_info {
        Json(json!({
            "ok": true,
            "name": name,
            "description": description,
            "is_channel": is_channel,
            "owner": owner,
            "public_key": state.group_public_key,
            "avatar_version": avatar_version,
            "member_count": member_count,
            "slow_mode_seconds": slow_mode_seconds,
        }))
    } else {
        Json(json!({
            "ok": false,
            "error": "Group not found",
        }))
    }
}
pub async fn get_public_channel(
    State(state): State<AppState>,
    Path(public_token): Path<String>,
) -> Result<Json<Value>, AppError> {
    let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;
    let channel_info: Option<(String, String, i64, String)> = conn.prepare("SELECT name, description, avatar_version, owner_username FROM group_info WHERE id = 1 AND public_channel_token = ?1 AND is_channel = 1").ok().and_then(|mut stmt| stmt.query_row([&public_token], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?))).ok());
    if let Some((name, description, avatar_version, owner)) = channel_info { Ok(Json(json!({ "ok": true, "name": name, "description": description, "avatar_version": avatar_version, "owner": owner, "is_channel": true, "public_token": public_token }))) } else { Err(AppError::NotFound("Public channel not found".into())) }
}
