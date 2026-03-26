use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::Response,
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signature, SigningKey, Verifier, VerifyingKey};
use rand::Rng;
use sha2::{Sha256, Digest};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::db::Db;
use crate::server::AppState;

/// (nonce bytes, time issued) — entries expire after NONCE_TTL_SECS seconds
pub type NonceStore = Arc<Mutex<HashMap<String, (Vec<u8>, Instant)>>>;

pub const NONCE_TTL_SECS: u64 = 60;
pub const NONCE_MAX_ENTRIES: usize = 10_000;

pub fn new_nonce_store() -> NonceStore {
    Arc::new(Mutex::new(HashMap::new()))
}

pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill(&mut bytes);
    hex_encode(&bytes)
}

pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex_encode(&hasher.finalize())
}

pub fn generate_nonce() -> Vec<u8> {
    let mut nonce = vec![0u8; 32];
    rand::thread_rng().fill(&mut nonce[..]);
    nonce
}

pub fn generate_group_keypair() -> (String, Vec<u8>) {
    let mut rng = rand::thread_rng();
    let signing_key = SigningKey::generate(&mut rng);
    let verifying_key = signing_key.verifying_key();
    let public_key_b64 = BASE64.encode(verifying_key.to_bytes());
    (public_key_b64, signing_key.to_bytes().to_vec())
}

pub fn verify_signature(public_key_b64: &str, nonce: &[u8], signature_b64: &str) -> bool {
    let pk_bytes = match BASE64.decode(public_key_b64) {
        Ok(b) => b,
        Err(_) => return false,
    };
    let sig_bytes = match BASE64.decode(signature_b64) {
        Ok(b) => b,
        Err(_) => return false,
    };

    let pk_array: [u8; 32] = match pk_bytes.try_into() {
        Ok(a) => a,
        Err(_) => return false,
    };
    let sig_array: [u8; 64] = match sig_bytes.try_into() {
        Ok(a) => a,
        Err(_) => return false,
    };

    let verifying_key = match VerifyingKey::from_bytes(&pk_array) {
        Ok(k) => k,
        Err(_) => return false,
    };
    let signature = Signature::from_bytes(&sig_array);

    verifying_key.verify(nonce, &signature).is_ok()
}

pub fn resolve_token(db: &Db, token: &str) -> Option<String> {
    let hashed = hash_token(token);
    let conn = db.lock().ok()?;

    // Check device_tokens first (multi-device: each device has its own token)
    if let Ok(mut stmt) = conn.prepare("SELECT username FROM device_tokens WHERE token = ?1") {
        if let Ok(username) = stmt.query_row([&hashed], |row| row.get::<_, String>(0)) {
            return Some(username);
        }
    }

    // Fallback: legacy sessions.token (single-device, kept for backward compat)
    let mut stmt = conn
        .prepare("SELECT username FROM sessions WHERE token = ?1")
        .ok()?;
    stmt.query_row([&hashed], |row| row.get::<_, String>(0)).ok()
}

pub async fn auth_middleware(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // HTTP requests must authenticate via the Authorization header only.
    // Query-string tokens are intentionally not accepted here — they appear in
    // proxy logs, browser history, and Referer headers.
    // (WebSocket upgrades use ?token= separately in connection.rs because the
    // browser WS API does not support custom request headers.)
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string());

    let token = match token {
        Some(t) => t,
        None => {
            println!("[auth] 401: no token provided");
            return Err(StatusCode::UNAUTHORIZED);
        }
    };

    let username = match resolve_token(&state.db, &token) {
        Some(u) => {
            println!("[auth] OK: user={}", u);
            u
        }
        None => {
            println!("[auth] 401: token not found");
            return Err(StatusCode::UNAUTHORIZED);
        }
    };

    req.extensions_mut().insert(AuthUser(username));
    Ok(next.run(req).await)
}

#[derive(Clone, Debug)]
pub struct AuthUser(pub String);

fn hex_encode(bytes: impl AsRef<[u8]>) -> String {
    bytes.as_ref().iter().map(|b| format!("{:02x}", b)).collect()
}
