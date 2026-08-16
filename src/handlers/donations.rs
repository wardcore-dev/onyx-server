use axum::extract::{Extension, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::models::permission::Permission;
use crate::permissions::has_permission;
use crate::server::AppState;

const MAX_ADDRESSES: usize = 20;

/// GET /donations - Public list of donation addresses (any authenticated member can view)
pub async fn get_donations(
    State(state): State<AppState>,
    Extension(_auth): Extension<AuthUser>,
) -> Result<Json<Value>, AppError> {
    let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

    let mut stmt = conn.prepare(
        "SELECT id, label, address FROM donation_addresses ORDER BY sort_order, id"
    )?;
    let addresses: Vec<Value> = stmt
        .query_map([], |row| {
            Ok(json!({
                "id": row.get::<_, i64>(0)?,
                "label": row.get::<_, String>(1)?,
                "address": row.get::<_, String>(2)?,
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(Json(json!(addresses)))
}

#[derive(Deserialize)]
pub struct DonationAddressInput {
    pub label: String,
    pub address: String,
}

#[derive(Deserialize)]
pub struct SetDonationsRequest {
    pub addresses: Vec<DonationAddressInput>,
}

/// PUT /donations - Replace the full donation address list (owner only)
pub async fn set_donations(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Json(req): Json<SetDonationsRequest>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;

    if req.addresses.len() > MAX_ADDRESSES {
        return Err(AppError::BadRequest(format!("Too many addresses (max {})", MAX_ADDRESSES)));
    }
    for a in &req.addresses {
        let label = a.label.trim();
        let address = a.address.trim();
        if label.is_empty() || label.len() > 40 {
            return Err(AppError::BadRequest("label must be 1-40 characters".into()));
        }
        if address.is_empty() || address.len() > 200 {
            return Err(AppError::BadRequest("address must be 1-200 characters".into()));
        }
    }

    let mut conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

    if !has_permission(&conn, &username, Permission::MANAGE_DONATIONS)? {
        return Err(AppError::Forbidden("Insufficient permissions".into()));
    }

    let tx = conn.transaction()?;
    tx.execute("DELETE FROM donation_addresses", [])?;
    for (i, a) in req.addresses.iter().enumerate() {
        tx.execute(
            "INSERT INTO donation_addresses (label, address, sort_order) VALUES (?1, ?2, ?3)",
            rusqlite::params![a.label.trim(), a.address.trim(), i as i64],
        )?;
    }
    tx.commit()?;

    Ok(Json(json!({ "ok": true, "count": req.addresses.len() })))
}
