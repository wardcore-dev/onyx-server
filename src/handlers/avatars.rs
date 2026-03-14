use axum::body::Body;
use axum::extract::{Extension, Multipart, State};
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::Json;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::server::AppState;

pub async fn upload_avatar(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    mut multipart: Multipart,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;

    {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;
        let role: String = conn
            .prepare("SELECT role FROM members WHERE username = ?1")?
            .query_row([&username], |r| r.get(0))
            .map_err(|_| AppError::NotFound("Not a member".into()))?;

        if role != "owner" && role != "moderator" {
            return Err(AppError::Forbidden("Only owner or moderator can change avatar".into()));
        }
    }

    let mut avatar_data: Option<Vec<u8>> = None;
    let mut mime_type = "image/png".to_string();

    while let Some(field) = multipart.next_field().await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {}", e)))?
    {
        if field.name() == Some("file") || field.name() == Some("avatar") {
            if let Some(ct) = field.content_type() {
                mime_type = ct.to_string();
            }
            let data = field.bytes().await
                .map_err(|e| AppError::BadRequest(format!("Read error: {}", e)))?;
            if data.len() > 5 * 1024 * 1024 {
                return Err(AppError::BadRequest("Avatar too large (max 5 MB)".into()));
            }
            avatar_data = Some(data.to_vec());
        }
    }

    let data = avatar_data.ok_or(AppError::BadRequest("No avatar file provided".into()))?;

    let new_version = {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

        conn.execute(
            "INSERT OR REPLACE INTO group_avatar (id, data, mime_type, updated_at) VALUES (1, ?1, ?2, datetime('now'))",
            rusqlite::params![data, mime_type],
        )?;

        conn.execute(
            "UPDATE group_info SET avatar_version = avatar_version + 1 WHERE id = 1",
            [],
        )?;

        let v: i32 = conn
            .query_row("SELECT avatar_version FROM group_info WHERE id = 1", [], |r| r.get(0))
            .unwrap_or(0);
        v
    };

    let packet = json!({
        "type": "avatar_update",
        "avatar_version": new_version,
    });
    state.hub.broadcast_to_all_subscribed(&packet).await;

    Ok(Json(json!({ "ok": true, "avatar_version": new_version })))
}

pub async fn get_avatar(
    State(state): State<AppState>,
) -> Result<Response, AppError> {
    let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

    let (data, mime): (Vec<u8>, String) = conn
        .prepare("SELECT data, mime_type FROM group_avatar WHERE id = 1")?
        .query_row([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|_| AppError::NotFound("No avatar".into()))?;

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CACHE_CONTROL, "public, max-age=3600")
        .body(Body::from(data))
        .unwrap())
}

pub async fn delete_avatar(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;

    {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;
        let owner: String = conn
            .prepare("SELECT owner_username FROM group_info WHERE id = 1")?
            .query_row([], |r| r.get(0))
            .map_err(|_| AppError::NotFound("Group not found".into()))?;

        if owner != username {
            return Err(AppError::Forbidden("Only owner can delete avatar".into()));
        }

        conn.execute("DELETE FROM group_avatar WHERE id = 1", [])?;
        conn.execute("UPDATE group_info SET avatar_version = avatar_version + 1 WHERE id = 1", [])?;
    }

    let packet = json!({
        "type": "avatar_update",
        "avatar_version": -1,
    });
    state.hub.broadcast_to_all_subscribed(&packet).await;

    Ok(Json(json!({ "ok": true })))
}
