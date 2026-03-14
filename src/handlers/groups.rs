use axum::extract::{Extension, Multipart, Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::server::AppState;

#[derive(Deserialize)]
pub struct RenameGroupRequest {
    pub name: String,
}

/// POST /groups/:id/rename - Rename group (owner only)
pub async fn rename_group(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(group_id): Path<i64>,
    Json(payload): Json<RenameGroupRequest>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;

    if group_id != 1 {
        return Err(AppError::NotFound("Group not found".into()));
    }

    // Check if user is owner
    let is_owner = {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;
        let role: String = conn
            .prepare("SELECT role FROM members WHERE username = ?1")
            .map_err(|e| AppError::Internal(e.to_string()))?
            .query_row([&username], |r| r.get(0))
            .map_err(|_| AppError::Unauthorized("Not a member".into()))?;

        role == "owner"
    };

    if !is_owner {
        return Err(AppError::Forbidden("Only owner can rename the group".into()));
    }

    // Validate name
    let new_name = payload.name.trim();
    if new_name.is_empty() {
        return Err(AppError::BadRequest("Group name cannot be empty".into()));
    }
    if new_name.len() > 100 {
        return Err(AppError::BadRequest("Group name too long (max 100 characters)".into()));
    }

    // Update group name
    {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;
        conn.execute(
            "UPDATE group_info SET name = ?1 WHERE id = 1",
            [new_name],
        )
        .map_err(|e| AppError::Internal(e.to_string()))?;
    }

    // Broadcast group info update via WebSocket
    let update_msg = json!({
        "type": "group_updated",
        "group_id": group_id,
        "name": new_name,
    });
    state.hub.broadcast_to_all_subscribed(&update_msg).await;

    Ok(Json(json!({
        "ok": true,
        "message": "Group renamed successfully",
        "name": new_name,
    })))
}

/// POST /groups/:id/avatar - Upload group avatar (owner only)
pub async fn upload_group_avatar(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(group_id): Path<i64>,
    mut multipart: Multipart,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;

    if group_id != 1 {
        return Err(AppError::NotFound("Group not found".into()));
    }

    // Check if user is owner
    let is_owner = {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;
        let role: String = conn
            .prepare("SELECT role FROM members WHERE username = ?1")
            .map_err(|e| AppError::Internal(e.to_string()))?
            .query_row([&username], |r| r.get(0))
            .map_err(|_| AppError::Unauthorized("Not a member".into()))?;

        role == "owner"
    };

    if !is_owner {
        return Err(AppError::Forbidden("Only owner can change the group avatar".into()));
    }

    // Parse multipart form data
    let mut image_data: Option<Vec<u8>> = None;
    let mut content_type: Option<String> = None;

    while let Some(field) = multipart.next_field().await.map_err(|e| AppError::BadRequest(e.to_string()))? {
        let name = field.name().unwrap_or("").to_string();

        if name == "avatar" {
            content_type = field.content_type().map(|ct| ct.to_string());
            image_data = Some(field.bytes().await.map_err(|e| AppError::BadRequest(e.to_string()))?.to_vec());
            break;
        }
    }

    let image_bytes = image_data.ok_or_else(|| AppError::BadRequest("No avatar file provided".into()))?;

    // Validate file size (max 5MB)
    if image_bytes.len() > 5 * 1024 * 1024 {
        return Err(AppError::BadRequest("Avatar file too large (max 5MB)".into()));
    }

    // Validate content type
    let ct = content_type.as_deref().unwrap_or("");
    if !ct.starts_with("image/") {
        return Err(AppError::BadRequest("File must be an image".into()));
    }

    // Determine file extension
    let extension = if ct.contains("png") {
        "png"
    } else if ct.contains("jpeg") || ct.contains("jpg") {
        "jpg"
    } else if ct.contains("gif") {
        "gif"
    } else if ct.contains("webp") {
        "webp"
    } else {
        "jpg" // default
    };

    // Increment avatar version
    let new_version = {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;
        let current_version: i64 = conn
            .query_row("SELECT avatar_version FROM group_info WHERE id = 1", [], |r| r.get(0))
            .unwrap_or(0);

        let new_version = current_version + 1;
        conn.execute(
            "UPDATE group_info SET avatar_version = ?1 WHERE id = 1",
            [new_version],
        )
        .map_err(|e| AppError::Internal(e.to_string()))?;

        new_version
    };

    // Save avatar file
    // Use storage path from config with server-specific subfolder to avoid conflicts between instances
    let server_subfolder = state.config.server.name.replace(|c: char| !c.is_alphanumeric() && c != '_' && c != '-', "_");
    let avatar_filename = format!("group_{}.{}", group_id, extension);
    let storage_base = PathBuf::from(&state.config.media.local.storage_path);
    let avatar_dir = storage_base.join(&server_subfolder);
    let avatar_path = avatar_dir.join(&avatar_filename);

    std::fs::create_dir_all(&avatar_dir)
        .map_err(|e| AppError::Internal(format!("Failed to create media directory: {}", e)))?;

    let mut file = std::fs::File::create(&avatar_path)
        .map_err(|e| AppError::Internal(format!("Failed to create avatar file: {}", e)))?;

    file.write_all(&image_bytes)
        .map_err(|e| AppError::Internal(format!("Failed to write avatar file: {}", e)))?;

    // Broadcast avatar update via WebSocket
    let update_msg = json!({
        "type": "group_avatar_updated",
        "group_id": group_id,
        "avatar_version": new_version,
    });
    state.hub.broadcast_to_all_subscribed(&update_msg).await;

    Ok(Json(json!({
        "ok": true,
        "message": "Group avatar updated successfully",
        "avatar_version": new_version,
    })))
}

/// GET /groups/:id/avatar - Get group avatar
pub async fn get_group_avatar(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
) -> Result<Vec<u8>, AppError> {
    if group_id != 1 {
        return Err(AppError::NotFound("Group not found".into()));
    }

    // Try to find the avatar file using config storage path with server-specific subfolder
    let server_subfolder = state.config.server.name.replace(|c: char| !c.is_alphanumeric() && c != '_' && c != '-', "_");
    let storage_base = PathBuf::from(&state.config.media.local.storage_path);
    let avatar_dir = storage_base.join(&server_subfolder);

    let extensions = ["png", "jpg", "jpeg", "gif", "webp"];
    for ext in &extensions {
        let avatar_path = avatar_dir.join(format!("group_{}.{}", group_id, ext));
        if let Ok(bytes) = std::fs::read(&avatar_path) {
            return Ok(bytes);
        }
    }

    Err(AppError::NotFound("Avatar not found".into()))
}

/// DELETE /groups/:id/avatar - Delete group avatar (owner only)
pub async fn delete_group_avatar(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(group_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;

    if group_id != 1 {
        return Err(AppError::NotFound("Group not found".into()));
    }

    // Check if user is owner
    let is_owner = {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;
        let role: String = conn
            .prepare("SELECT role FROM members WHERE username = ?1")
            .map_err(|e| AppError::Internal(e.to_string()))?
            .query_row([&username], |r| r.get(0))
            .map_err(|_| AppError::Unauthorized("Not a member".into()))?;

        role == "owner"
    };

    if !is_owner {
        return Err(AppError::Forbidden("Only owner can delete the group avatar".into()));
    }

    // Delete all possible avatar files using config storage path with server-specific subfolder
    let server_subfolder = state.config.server.name.replace(|c: char| !c.is_alphanumeric() && c != '_' && c != '-', "_");
    let storage_base = PathBuf::from(&state.config.media.local.storage_path);
    let avatar_dir = storage_base.join(&server_subfolder);

    let extensions = ["png", "jpg", "jpeg", "gif", "webp"];
    for ext in &extensions {
        let avatar_path = avatar_dir.join(format!("group_{}.{}", group_id, ext));
        let _ = std::fs::remove_file(&avatar_path); // Ignore errors if file doesn't exist
    }

    // Increment avatar version
    let new_version = {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;
        let current_version: i64 = conn
            .query_row("SELECT avatar_version FROM group_info WHERE id = 1", [], |r| r.get(0))
            .unwrap_or(0);

        let new_version = current_version + 1;
        conn.execute(
            "UPDATE group_info SET avatar_version = ?1 WHERE id = 1",
            [new_version],
        )
        .map_err(|e| AppError::Internal(e.to_string()))?;

        new_version
    };

    // Broadcast avatar update via WebSocket
    let update_msg = json!({
        "type": "group_avatar_updated",
        "group_id": group_id,
        "avatar_version": new_version,
    });
    state.hub.broadcast_to_all_subscribed(&update_msg).await;

    Ok(Json(json!({
        "ok": true,
        "message": "Group avatar deleted successfully",
        "avatar_version": new_version,
    })))
}
