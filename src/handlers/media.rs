use axum::body::Body;
use axum::extract::{Extension, Multipart, Path, Query, Request, State};
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Deserialize)]
pub(crate) struct TokenQuery {
    token: Option<String>,
}

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::media::{self, local};
use crate::server::AppState;

pub async fn upload_media(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    mut multipart: Multipart,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;
    let media_id = Uuid::new_v4().to_string();

    let mut file_data: Option<Vec<u8>> = None;
    let mut original_filename = String::new();
    let mut mime_type = "application/octet-stream".to_string();

    while let Some(field) = multipart.next_field().await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {}", e)))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                if let Some(fname) = field.file_name() {
                    original_filename = fname.to_string();
                    println!("[media] Upload: original_filename from multipart: '{}'", original_filename);
                }
                if let Some(ct) = field.content_type() {
                    mime_type = ct.to_string();
                    println!("[media] Upload: content_type: '{}'", mime_type);
                }
                let data = field.bytes().await
                    .map_err(|e| AppError::BadRequest(format!("Failed to read file: {}", e)))?;

                let max_bytes = (state.config.media.max_file_size_mb as usize) * 1024 * 1024;
                if data.len() > max_bytes {
                    return Err(AppError::BadRequest(format!(
                        "File too large (max {} MB)", state.config.media.max_file_size_mb
                    )));
                }

                file_data = Some(data.to_vec());
            }
            _ => {}
        }
    }

    let data = file_data.ok_or(AppError::BadRequest("No file field in upload".into()))?;

    // Verify membership (single group)
    {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;
        let is_member: bool = conn
            .prepare("SELECT COUNT(*) FROM members WHERE username = ?1")
            .ok()
            .and_then(|mut s| s.query_row(rusqlite::params![username], |r| r.get::<_, i64>(0)).ok())
            .map(|c| c > 0)
            .unwrap_or(false);
        if !is_member {
            return Err(AppError::Forbidden("Not a member of this group".into()));
        }
    }

    // Detect the actual file type from magic bytes — ignore client-supplied Content-Type.
    // This prevents an attacker from uploading HTML/JS with a forged image/* MIME type.
    let actual_mime = detect_mime_from_magic(&data);
    mime_type = actual_mime.to_string();
    println!("[media] Upload: detected mime from magic bytes: '{}'", mime_type);

    // Check allowed types against the detected (not client-supplied) MIME
    let file_type = detect_file_type(&mime_type);
    if !state.config.media.allowed_types.contains(&file_type) {
        return Err(AppError::BadRequest(format!("File type '{}' not allowed", file_type)));
    }

    let (url, provider) = if state.config.media.provider == "local" {
        // Use server-specific subfolder to avoid conflicts between instances
        let server_subfolder = state.config.server.name.replace(|c: char| !c.is_alphanumeric() && c != '_' && c != '-', "_");
        let storage_base = std::path::PathBuf::from(&state.config.media.local.storage_path);
        let storage_path_with_subfolder = storage_base.join(&server_subfolder);

        // Create subfolder if it doesn't exist
        std::fs::create_dir_all(&storage_path_with_subfolder)
            .map_err(|e| AppError::Internal(format!("Failed to create media directory: {}", e)))?;

        let filename = local::save_file(
            storage_path_with_subfolder.to_str().unwrap_or(&state.config.media.local.storage_path),
            &media_id,
            &original_filename,
            &data,
        ).await.map_err(AppError::Internal)?;

        println!("[media] Upload: saved filename: '{}', original: '{}'", filename, original_filename);

        // Use public_url if configured, otherwise use relative path
        let url = if let Some(ref public_url) = state.config.server.public_url {
            format!("{}/data/media/{}", public_url.trim_end_matches('/'), filename)
        } else {
            format!("/data/media/{}", filename)
        };

        println!("[media] Upload: returning URL: '{}'", url);
        (url, "local".to_string())
    } else {
        let result = media::upload_to_provider(
            &state.config.media,
            &original_filename,
            data.clone(),
            &mime_type,
        ).await.map_err(AppError::Internal)?;
        (result.url, result.provider)
    };

    // Store in DB
    {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;
        conn.execute(
            "INSERT INTO media (id, uploader_username, original_filename, mime_type, size_bytes, storage_path, provider)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![media_id, username, original_filename, mime_type, data.len() as i64, url, provider],
        )?;
    }

    Ok(Json(json!({
        "ok": true,
        "id": media_id,
        "url": url,
        "original_filename": original_filename,
        "mime_type": mime_type,
        "provider": provider,
    })))
}

pub async fn download_media(
    State(state): State<AppState>,
    Query(query): Query<TokenQuery>,
    Path(filename): Path<String>,
    req: Request,
) -> Result<Response, AppError> {
    // Accept token from Authorization: Bearer header OR ?token= query param.
    // The ?token= fallback exists specifically for media because image/video widgets
    // in clients cannot set custom request headers when loading a URL directly.
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string())
        .or(query.token)
        .ok_or(AppError::Unauthorized("Authentication required".into()))?;

    let username = crate::auth::resolve_token(&state.db, &token)
        .ok_or(AppError::Unauthorized("Invalid token".into()))?;

    println!("[media] Download request for: {} by user: {}", filename, username);

    // Verify that user is a member of the group
    {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;
        let is_member: bool = conn
            .prepare("SELECT COUNT(*) FROM members WHERE username = ?1")
            .ok()
            .and_then(|mut s| s.query_row(rusqlite::params![username], |r| r.get::<_, i64>(0)).ok())
            .map(|c| c > 0)
            .unwrap_or(false);
        if !is_member {
            println!("[media] Access denied: {} is not a member", username);
            return Err(AppError::Forbidden("Not a member of this group".into()));
        }
    }

    if state.config.media.provider != "local" {
        println!("[media] Error: provider is not local");
        return Err(AppError::NotFound("Local media not available".into()));
    }

    // Use server-specific subfolder to avoid conflicts between instances
    let server_subfolder = state.config.server.name.replace(|c: char| !c.is_alphanumeric() && c != '_' && c != '-', "_");
    let storage_base = std::path::PathBuf::from(&state.config.media.local.storage_path);
    let storage_path_with_subfolder = storage_base.join(&server_subfolder);

    let data = local::read_file(
        storage_path_with_subfolder.to_str().unwrap_or(&state.config.media.local.storage_path),
        &filename
    )
        .await
        .map_err(|e| {
            println!("[media] Error reading file '{}': {:?}", filename, e);
            AppError::NotFound("File not found".into())
        })?;

    println!("[media] Successfully read file '{}', size: {} bytes for user: {}", filename, data.len(), username);

    let mime = mime_guess::from_path(&filename)
        .first_or_octet_stream()
        .to_string();

    let disposition = format!("attachment; filename=\"{}\"", filename);

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CACHE_CONTROL, "public, max-age=86400")
        .header("X-Content-Type-Options", "nosniff")
        .header(header::CONTENT_DISPOSITION, disposition)
        .body(Body::from(data))
        .unwrap())
}

fn detect_mime_from_magic(data: &[u8]) -> &'static str {
    match data {
        // Images
        d if d.starts_with(&[0xFF, 0xD8, 0xFF]) => "image/jpeg",
        d if d.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) => "image/png",
        d if d.starts_with(b"GIF87a") || d.starts_with(b"GIF89a") => "image/gif",
        d if d.len() >= 12 && d.starts_with(b"RIFF") && &d[8..12] == b"WEBP" => "image/webp",
        // Video
        d if d.len() >= 8 && &d[4..8] == b"ftyp" => "video/mp4",
        d if d.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) => "video/webm",
        d if d.len() >= 12 && d.starts_with(b"RIFF") && &d[8..12] == b"AVI " => "video/x-msvideo",
        // Audio
        d if d.starts_with(b"ID3")
            || (d.len() >= 2 && d[0] == 0xFF && (d[1] & 0xE0) == 0xE0) => "audio/mpeg",
        d if d.starts_with(b"OggS") => "audio/ogg",
        d if d.starts_with(b"fLaC") => "audio/flac",
        d if d.len() >= 12 && d.starts_with(b"RIFF") && &d[8..12] == b"WAVE" => "audio/wav",
        // Anything else is treated as opaque binary — not executable by browsers
        _ => "application/octet-stream",
    }
}

fn detect_file_type(mime: &str) -> String {
    if mime.starts_with("image/") {
        "image".to_string()
    } else if mime.starts_with("video/") {
        "video".to_string()
    } else if mime.starts_with("audio/") {
        "audio".to_string()
    } else {
        "file".to_string()
    }
}
