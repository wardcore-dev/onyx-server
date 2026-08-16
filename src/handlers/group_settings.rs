use axum::extract::{Extension, Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::models::permission::Permission;
use crate::permissions::require_permission;
use crate::server::AppState;

struct GroupSettings {
    description: String,
    motd: String,
    max_members: i64,
    max_message_length: i64,
    max_messages_per_minute: i64,
    default_role_id: i64,
}

fn load_settings(conn: &rusqlite::Connection) -> Result<GroupSettings, AppError> {
    conn.query_row(
        "SELECT description, motd, max_members, max_message_length, max_messages_per_minute, default_role_id
         FROM group_info WHERE id = 1",
        [],
        |r| {
            Ok(GroupSettings {
                description: r.get(0)?,
                motd: r.get(1)?,
                max_members: r.get(2)?,
                max_message_length: r.get(3)?,
                max_messages_per_minute: r.get(4)?,
                default_role_id: r.get(5)?,
            })
        },
    ).map_err(|_| AppError::NotFound("Group not found".into()))
}

fn settings_json(s: &GroupSettings) -> Value {
    json!({
        "description": s.description,
        "motd": s.motd,
        "max_members": s.max_members,
        "max_message_length": s.max_message_length,
        "max_messages_per_minute": s.max_messages_per_minute,
        "default_role_id": s.default_role_id,
    })
}

/// GET /groups/:id/settings - read live group settings (any member)
pub async fn get_settings(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(group_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    if group_id != 1 {
        return Err(AppError::NotFound("Group not found".into()));
    }
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

    let settings = load_settings(&conn)?;
    Ok(Json(settings_json(&settings)))
}

#[derive(Deserialize, Default)]
pub struct UpdateSettingsRequest {
    pub description: Option<String>,
    pub motd: Option<String>,
    pub max_members: Option<i64>,
    pub max_message_length: Option<i64>,
    pub max_messages_per_minute: Option<i64>,
    pub default_role_id: Option<i64>,
}

/// PUT /groups/:id/settings - partial update of live group settings (requires MANAGE_SETTINGS)
pub async fn update_settings(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(group_id): Path<i64>,
    Json(body): Json<UpdateSettingsRequest>,
) -> Result<Json<Value>, AppError> {
    if group_id != 1 {
        return Err(AppError::NotFound("Group not found".into()));
    }
    let username = auth.0;
    require_permission(&state, &username, Permission::MANAGE_SETTINGS)?;

    if let Some(len) = body.max_message_length {
        if len < 1 || len > 100_000 {
            return Err(AppError::BadRequest("max_message_length must be between 1 and 100000".into()));
        }
    }
    if let Some(max) = body.max_members {
        if max < 1 || max > 100_000 {
            return Err(AppError::BadRequest("max_members must be between 1 and 100000".into()));
        }
    }
    if let Some(rate) = body.max_messages_per_minute {
        if rate < 0 || rate > 10_000 {
            return Err(AppError::BadRequest("max_messages_per_minute must be between 0 and 10000".into()));
        }
    }

    let updated = {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

        if let Some(role_id) = body.default_role_id {
            let exists: bool = conn
                .prepare("SELECT COUNT(*) FROM roles WHERE id = ?1")?
                .query_row([role_id], |r| r.get::<_, i64>(0))
                .map(|c| c > 0)
                .unwrap_or(false);
            if !exists {
                return Err(AppError::BadRequest("default_role_id does not reference an existing role".into()));
            }
        }

        let current = load_settings(&conn)?;

        let updated = GroupSettings {
            description: body.description.unwrap_or(current.description),
            motd: body.motd.unwrap_or(current.motd),
            max_members: body.max_members.unwrap_or(current.max_members),
            max_message_length: body.max_message_length.unwrap_or(current.max_message_length),
            max_messages_per_minute: body.max_messages_per_minute.unwrap_or(current.max_messages_per_minute),
            default_role_id: body.default_role_id.unwrap_or(current.default_role_id),
        };

        conn.execute(
            "UPDATE group_info SET description = ?1, motd = ?2, max_members = ?3, max_message_length = ?4,
                                    max_messages_per_minute = ?5, default_role_id = ?6
             WHERE id = 1",
            rusqlite::params![
                updated.description,
                updated.motd,
                updated.max_members,
                updated.max_message_length,
                updated.max_messages_per_minute,
                updated.default_role_id,
            ],
        )?;

        updated
    };

    let response = settings_json(&updated);

    let packet = json!({
        "type": "group_settings_updated",
        "group_id": 1,
        "settings": response,
    });
    state.hub.broadcast_to_all_subscribed(&packet).await;

    Ok(Json(response))
}
