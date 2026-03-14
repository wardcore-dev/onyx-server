use crate::db::Db;
use crate::models::member::Role;

/// Check if a user has at least moderator permissions (single-group, no group_id needed)
pub fn check_moderator(db: &Db, username: &str) -> Result<Role, String> {
    let conn = db.lock().map_err(|_| "db lock".to_string())?;
    let role_str: String = conn
        .prepare("SELECT role FROM members WHERE username = ?1")
        .map_err(|e| e.to_string())?
        .query_row(rusqlite::params![username], |r| r.get(0))
        .map_err(|_| "Not a member".to_string())?;

    let role = Role::from_str(&role_str).ok_or("Invalid role".to_string())?;
    if !role.can_moderate() {
        return Err("Insufficient permissions".to_string());
    }
    Ok(role)
}

/// Check if a user is an owner of the group
pub fn check_owner(db: &Db, username: &str) -> Result<(), String> {
    let conn = db.lock().map_err(|_| "db lock".to_string())?;
    let role_str: String = conn
        .prepare("SELECT role FROM members WHERE username = ?1")
        .map_err(|e| e.to_string())?
        .query_row(rusqlite::params![username], |r| r.get(0))
        .map_err(|_| "Not a member".to_string())?;

    let role = Role::from_str(&role_str).ok_or("Invalid role".to_string())?;
    if !role.is_owner() {
        return Err("Only owners can perform this action".to_string());
    }
    Ok(())
}
