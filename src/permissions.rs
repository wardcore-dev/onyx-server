use rusqlite::{Connection, OptionalExtension};

use crate::error::AppError;
use crate::models::permission::Permission;
use crate::server::AppState;

/// Look up `username`'s role permissions via the roles table and check `perm`.
/// Users with no role_id (or no membership row) have no permissions.
pub fn has_permission(conn: &Connection, username: &str, perm: Permission) -> Result<bool, AppError> {
    let bits: Option<i64> = conn
        .query_row(
            "SELECT r.permissions FROM members m JOIN roles r ON r.id = m.role_id WHERE m.username = ?1",
            [username],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(bits
        .map(|b| Permission::from_bits_truncate(b).contains(perm))
        .unwrap_or(false))
}

/// Like [`has_permission`], but succeeds if `username` holds any one of
/// `perms`. Used where two permissions grant overlapping capability (e.g.
/// VIEW_MUTE_LIST doubles as "can mute", matching the client's "Manage Mute
/// List" checkbox which is meant to fully cover muting, not just viewing).
pub fn has_any_permission(conn: &Connection, username: &str, perms: &[Permission]) -> Result<bool, AppError> {
    let bits: Option<i64> = conn
        .query_row(
            "SELECT r.permissions FROM members m JOIN roles r ON r.id = m.role_id WHERE m.username = ?1",
            [username],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(bits
        .map(|b| {
            let held = Permission::from_bits_truncate(b);
            perms.iter().any(|p| held.contains(*p))
        })
        .unwrap_or(false))
}

/// True if `username` currently holds the system Owner role (role_id = 1).
pub fn is_owner_equivalent(conn: &Connection, username: &str) -> Result<bool, AppError> {
    let role_id: Option<i64> = conn
        .query_row("SELECT role_id FROM members WHERE username = ?1", [username], |r| r.get(0))
        .optional()
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(role_id == Some(1))
}

/// Number of members currently holding the system Owner role (role_id = 1).
pub fn count_owner_equivalent_members(conn: &Connection) -> Result<i64, AppError> {
    conn.query_row("SELECT COUNT(*) FROM members WHERE role_id = 1", [], |r| r.get(0))
        .map_err(|e| AppError::Internal(e.to_string()))
}

/// Lock the DB, check `username` has `perm`, and return a Forbidden error if not.
pub fn require_permission(state: &AppState, username: &str, perm: Permission) -> Result<(), AppError> {
    let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;
    if has_permission(&conn, username, perm)? {
        Ok(())
    } else {
        Err(AppError::Forbidden("Insufficient permissions".into()))
    }
}
