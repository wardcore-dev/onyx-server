use axum::extract::State;
use axum::Json;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{json, Value};

use crate::auth;
use crate::error::AppError;
use crate::models::session::{RegisterRequest, ChallengeRequest, VerifyRequest};
use crate::server::AppState;

pub async fn register(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> Result<Json<Value>, AppError> {
    let username = req.username.trim().to_string();
    let display_name = req.display_name.trim().to_string();
    let public_key = req.public_key.trim().to_string();
    let password_hash = req.password_hash.trim().to_string();

    if username.is_empty() || public_key.is_empty() {
        return Err(AppError::BadRequest("username and public_key required".into()));
    }
    // Password is optional (required for groups, optional for channels)
    if username.len() > 64 || display_name.len() > 128 {
        return Err(AppError::BadRequest("username or display_name too long".into()));
    }

    // Validate public key is valid base64 and 32 bytes
    let pk_bytes = BASE64.decode(&public_key)
        .map_err(|_| AppError::BadRequest("invalid base64 public_key".into()))?;
    if pk_bytes.len() != 32 {
        return Err(AppError::BadRequest("public_key must be 32 bytes (Ed25519)".into()));
    }

    let token = auth::generate_token();
    let token_hash = auth::hash_token(&token);

    // Debug logging
    println!("[register] user={}, pk={}, pw_hash={}",
        username,
        &public_key[..8],
        if password_hash.is_empty() { "(empty)" } else { "(set)" }
    );

    {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

        // Check if banned
        let is_banned: bool = conn
            .prepare("SELECT COUNT(*) FROM bans WHERE username = ?1")
            .ok()
            .and_then(|mut s| s.query_row([&username], |r| r.get::<_, i64>(0)).ok())
            .map(|c| c > 0)
            .unwrap_or(false);

        if is_banned {
            return Err(AppError::Forbidden("You are banned from this server".into()));
        }

        // Check if username already registered with different pubkey
        let existing: Option<(String, String)> = conn
            .prepare("SELECT public_key, password_hash FROM sessions WHERE username = ?1")
            .ok()
            .and_then(|mut s| s.query_row([&username], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            }).ok());

        if let Some((existing_pk, stored_pw_hash)) = existing {
            // Verify password if one was set during registration.
            // Double-hash both sides so the comparison is over fixed-length digests
            // and does not short-circuit on the first differing byte.
            if !stored_pw_hash.is_empty()
                && auth::hash_token(&stored_pw_hash) != auth::hash_token(&password_hash)
            {
                return Err(AppError::Unauthorized("Invalid password".into()));
            }

            // Password is correct — update identity fields but NOT the token in sessions.
            // The new token is stored in device_tokens to support multiple devices.
            conn.execute(
                "UPDATE sessions SET display_name = ?1, public_key = ?2, last_seen = datetime('now') WHERE username = ?3",
                rusqlite::params![display_name, public_key, username],
            )?;
            conn.execute(
                "INSERT OR IGNORE INTO device_tokens (username, token) VALUES (?1, ?2)",
                rusqlite::params![username, token_hash],
            )?;

            // Check if user is in members table, add if not (important for channels)
            let is_member: bool = conn
                .prepare("SELECT COUNT(*) FROM members WHERE username = ?1")?
                .query_row([&username], |r| r.get::<_, i64>(0))
                .map(|c| c > 0)
                .unwrap_or(false);

            if is_member {
                // Update existing member record
                conn.execute(
                    "UPDATE members SET public_key = ?1, display_name = ?2 WHERE username = ?3",
                    rusqlite::params![public_key, display_name, username],
                )?;
            } else {
                // Add to members if not already there (re-joining after being removed)
                let group_exists: bool = conn
                    .prepare("SELECT COUNT(*) FROM group_info WHERE id = 1")?
                    .query_row([], |r| r.get::<_, i64>(0))
                    .map(|c| c > 0)
                    .unwrap_or(false);

                if group_exists {
                    // Check if there are any existing members
                    let member_count: i64 = conn
                        .prepare("SELECT COUNT(*) FROM members")?
                        .query_row([], |r| r.get(0))?;

                    // First user gets owner role, others get default role
                    let role = if member_count == 0 { "owner" } else { &state.config.moderation.default_role };

                    conn.execute(
                        "INSERT INTO members (username, display_name, public_key, role) VALUES (?1, ?2, ?3, ?4)",
                        rusqlite::params![username, display_name, public_key, role],
                    )?;

                    // If this is the first user (owner), update group_info owner_username
                    if member_count == 0 {
                        conn.execute(
                            "UPDATE group_info SET owner_username = ?1 WHERE id = 1",
                            [&username],
                        )?;
                        println!("[register] Set {} as group owner", username);
                    }

                    println!("[register] Added user {} to members with role {}", username, role);
                }
            }
        } else {
            conn.execute(
                "INSERT INTO sessions (username, display_name, token, public_key, password_hash) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![username, display_name, token_hash, public_key, password_hash],
            )?;
            // Also store in device_tokens for multi-device support
            conn.execute(
                "INSERT OR IGNORE INTO device_tokens (username, token) VALUES (?1, ?2)",
                rusqlite::params![username, token_hash],
            )?;

            // Auto-add to the group if not already a member
            let is_member: bool = conn
                .prepare("SELECT COUNT(*) FROM members WHERE username = ?1")?
                .query_row([&username], |r| r.get::<_, i64>(0))
                .map(|c| c > 0)
                .unwrap_or(false);

            if !is_member {
                let group_exists: bool = conn
                    .prepare("SELECT COUNT(*) FROM group_info WHERE id = 1")?
                    .query_row([], |r| r.get::<_, i64>(0))
                    .map(|c| c > 0)
                    .unwrap_or(false);

                if group_exists {
                    conn.execute(
                        "INSERT OR IGNORE INTO members (username, display_name, public_key, role) VALUES (?1, ?2, ?3, ?4)",
                        rusqlite::params![username, display_name, public_key, state.config.moderation.default_role],
                    )?;
                }
            }
        }
    }

    println!("[register] SUCCESS: user={}", username);

    Ok(Json(json!({
        "ok": true,
        "token": token,
        "username": username,
    })))
}

pub async fn challenge(
    State(state): State<AppState>,
    Json(req): Json<ChallengeRequest>,
) -> Result<Json<Value>, AppError> {
    let username = req.username.trim().to_string();
    let public_key = req.public_key.trim().to_string();

    {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;
        let stored_pk: Option<String> = conn
            .prepare("SELECT public_key FROM sessions WHERE username = ?1")
            .ok()
            .and_then(|mut s| s.query_row([&username], |r| r.get(0)).ok());

        match stored_pk {
            Some(pk) if pk == public_key => {}
            Some(_) => return Err(AppError::Unauthorized("Public key mismatch".into())),
            None => return Err(AppError::NotFound("User not registered".into())),
        }
    }

    let nonce = auth::generate_nonce();
    let nonce_b64 = BASE64.encode(&nonce);

    {
        let mut store = state.nonces.lock().map_err(|_| AppError::Internal("nonce lock".into()))?;
        // Prune expired entries on every insert to bound memory growth
        let now = std::time::Instant::now();
        store.retain(|_, (_, ts)| now.duration_since(*ts).as_secs() < auth::NONCE_TTL_SECS);
        if store.len() >= auth::NONCE_MAX_ENTRIES {
            return Err(AppError::BadRequest("Too many pending challenges, try again later".into()));
        }
        store.insert(username.clone(), (nonce, now));
    }

    Ok(Json(json!({
        "ok": true,
        "nonce": nonce_b64,
    })))
}

pub async fn verify(
    State(state): State<AppState>,
    Json(req): Json<VerifyRequest>,
) -> Result<Json<Value>, AppError> {
    let username = req.username.trim().to_string();
    let signature = req.signature.trim().to_string();

    let nonce = {
        let mut store = state.nonces.lock().map_err(|_| AppError::Internal("nonce lock".into()))?;
        let (nonce_bytes, issued_at) = store.remove(&username)
            .ok_or(AppError::BadRequest("No pending challenge for this user".into()))?;
        if issued_at.elapsed().as_secs() >= auth::NONCE_TTL_SECS {
            return Err(AppError::BadRequest("Challenge expired, request a new one".into()));
        }
        nonce_bytes
    };

    let public_key = {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;
        let pk: String = conn
            .prepare("SELECT public_key FROM sessions WHERE username = ?1")
            .map_err(|e| AppError::Internal(e.to_string()))?
            .query_row([&username], |r| r.get(0))
            .map_err(|_| AppError::NotFound("User not found".into()))?;
        pk
    };

    if !auth::verify_signature(&public_key, &nonce, &signature) {
        return Err(AppError::Unauthorized("Invalid signature".into()));
    }

    let token = auth::generate_token();
    let token_hash = auth::hash_token(&token);

    {
        let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;
        // Insert new device token (don't overwrite — keeps other devices' tokens valid)
        conn.execute(
            "INSERT OR IGNORE INTO device_tokens (username, token) VALUES (?1, ?2)",
            rusqlite::params![username, token_hash],
        )?;
        conn.execute(
            "UPDATE sessions SET last_seen = datetime('now') WHERE username = ?1",
            rusqlite::params![username],
        )?;
    }

    Ok(Json(json!({
        "ok": true,
        "token": token,
        "username": username,
    })))
}
