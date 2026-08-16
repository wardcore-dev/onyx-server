use axum::extract::{Extension, Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::models::permission::Permission;
use crate::permissions::require_permission;
use crate::server::AppState;

fn is_valid_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// GET /roles - list all roles with member counts (any member)
pub async fn list_roles(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
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

    let mut stmt = conn.prepare(
        "SELECT id, name, color, permissions, is_system,
                (SELECT COUNT(*) FROM members WHERE members.role_id = roles.id) AS member_count
         FROM roles ORDER BY sort_order"
    )?;

    let roles: Vec<Value> = stmt
        .query_map([], |row| {
            let permissions_bits: i64 = row.get(3)?;
            Ok(json!({
                "id": row.get::<_, i64>(0)?,
                "name": row.get::<_, String>(1)?,
                "color": row.get::<_, String>(2)?,
                "permissions": Permission::from_bits_truncate(permissions_bits).to_names(),
                "is_system": row.get::<_, i64>(4)? != 0,
                "member_count": row.get::<_, i64>(5)?,
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(Json(json!(roles)))
}

#[derive(Deserialize)]
pub struct RoleRequest {
    pub name: String,
    pub color: String,
    pub permissions: Vec<String>,
}

/// POST /roles - create a new custom role (requires MANAGE_ROLES)
pub async fn create_role(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Json(body): Json<RoleRequest>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;
    require_permission(&state, &username, Permission::MANAGE_ROLES)?;

    let name = body.name.trim().to_string();
    if name.is_empty() || name.len() > 40 {
        return Err(AppError::BadRequest("name must be 1-40 characters".into()));
    }
    if !is_valid_color(&body.color) {
        return Err(AppError::BadRequest("color must be a hex string like #99AAB5".into()));
    }
    let permissions = Permission::from_names(&body.permissions).map_err(AppError::BadRequest)?;

    let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

    let next_sort_order: i64 = conn
        .query_row("SELECT COALESCE(MAX(sort_order), -1) + 1 FROM roles", [], |r| r.get(0))
        .unwrap_or(0);

    let insert_result = conn.execute(
        "INSERT INTO roles (name, color, permissions, is_system, sort_order) VALUES (?1, ?2, ?3, 0, ?4)",
        rusqlite::params![name, body.color, permissions.bits(), next_sort_order],
    );

    if let Err(e) = insert_result {
        if let rusqlite::Error::SqliteFailure(err, _) = &e {
            if err.code == rusqlite::ErrorCode::ConstraintViolation {
                return Err(AppError::Conflict("A role with that name already exists".into()));
            }
        }
        return Err(AppError::Internal(e.to_string()));
    }

    let id = conn.last_insert_rowid();

    Ok(Json(json!({
        "id": id,
        "name": name,
        "color": body.color,
        "permissions": permissions.to_names(),
        "is_system": false,
        "member_count": 0,
    })))
}

/// PUT /roles/:id - update a role (requires MANAGE_ROLES)
pub async fn update_role(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(role_id): Path<i64>,
    Json(body): Json<RoleRequest>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;
    require_permission(&state, &username, Permission::MANAGE_ROLES)?;

    let name = body.name.trim().to_string();
    if name.is_empty() || name.len() > 40 {
        return Err(AppError::BadRequest("name must be 1-40 characters".into()));
    }
    if !is_valid_color(&body.color) {
        return Err(AppError::BadRequest("color must be a hex string like #99AAB5".into()));
    }
    let permissions = Permission::from_names(&body.permissions).map_err(AppError::BadRequest)?;

    // The system Owner role's permission set is a floor: it can never be reduced below "all".
    if role_id == 1 && permissions != Permission::all() {
        return Err(AppError::BadRequest("Cannot reduce the Owner role's permissions".into()));
    }

    let affected_members: Vec<String> = {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

        let update_result = conn.execute(
            "UPDATE roles SET name = ?1, color = ?2, permissions = ?3 WHERE id = ?4",
            rusqlite::params![name, body.color, permissions.bits(), role_id],
        );

        let updated = match update_result {
            Ok(n) => n,
            Err(e) => {
                if let rusqlite::Error::SqliteFailure(err, _) = &e {
                    if err.code == rusqlite::ErrorCode::ConstraintViolation {
                        return Err(AppError::Conflict("A role with that name already exists".into()));
                    }
                }
                return Err(AppError::Internal(e.to_string()));
            }
        };

        if updated == 0 {
            return Err(AppError::NotFound("Role not found".into()));
        }

        let mut stmt = conn.prepare("SELECT username FROM members WHERE role_id = ?1")?;
        let rows = stmt.query_map([role_id], |r| r.get::<_, String>(0))?;
        rows.filter_map(|r| r.ok()).collect()
    };

    let packet = json!({
        "type": "role_updated",
        "id": role_id,
        "name": &name,
        "color": &body.color,
        "permissions": permissions.to_names(),
    });
    state.hub.broadcast_to_all_subscribed(&packet).await;

    for member in &affected_members {
        let packet_broadcast = json!({
            "type": "member_role_updated",
            "username": member,
            "role_name": &name,
            "role_color": &body.color,
        });
        state.hub.broadcast_to_all_subscribed(&packet_broadcast).await;
        let packet_role_changed = json!({
            "type": "role_changed",
            "group_id": 1,
            "role_id": role_id,
            "role_name": &name,
            "role_color": &body.color,
            "permissions": permissions.to_names(),
        });
        state.hub.send_to_user(member, &packet_role_changed).await;
    }

    Ok(Json(json!({
        "id": role_id,
        "name": name,
        "color": body.color,
        "permissions": permissions.to_names(),
    })))
}

/// DELETE /roles/:id - delete a custom role, reassigning its members to Member (requires MANAGE_ROLES)
pub async fn delete_role(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(role_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;
    require_permission(&state, &username, Permission::MANAGE_ROLES)?;

    let affected_members: Vec<String> = {
        let mut conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

        let is_system: bool = conn
            .query_row("SELECT is_system FROM roles WHERE id = ?1", [role_id], |r| r.get(0))
            .map_err(|_| AppError::NotFound("Role not found".into()))?;

        if is_system {
            return Err(AppError::Forbidden("Cannot delete a system role".into()));
        }

        let tx = conn.transaction()?;

        let affected: Vec<String> = {
            let mut stmt = tx.prepare("SELECT username FROM members WHERE role_id = ?1")?;
            let rows = stmt.query_map([role_id], |r| r.get::<_, String>(0))?;
            let collected: Vec<String> = rows.filter_map(|r| r.ok()).collect();
            collected
        };

        tx.execute("UPDATE members SET role_id = 3, role = 'member' WHERE role_id = ?1", [role_id])?;
        tx.execute("DELETE FROM roles WHERE id = ?1", [role_id])?;

        tx.commit()?;

        affected
    };

    let packet = json!({ "type": "role_deleted", "role_id": role_id });
    state.hub.broadcast_to_all_subscribed(&packet).await;

    for member in &affected_members {
        let packet = json!({
            "type": "role_changed",
            "group_id": 1,
            "role_id": 3,
            "role_name": "Member",
            "role_color": "#99AAB5",
            "permissions": Vec::<String>::new(),
        });
        state.hub.send_to_user(member, &packet).await;

        let packet_broadcast = json!({
            "type": "member_role_updated",
            "username": member,
            "role_name": "Member",
            "role_color": "#99AAB5",
        });
        state.hub.broadcast_to_all_subscribed(&packet_broadcast).await;
    }

    Ok(Json(json!({ "ok": true })))
}
