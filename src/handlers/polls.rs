use axum::extract::{Extension, Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::models::permission::Permission;
use crate::permissions::has_permission;
use crate::server::AppState;

/// Verify the caller is a member of the group (any role).
fn require_member(conn: &rusqlite::Connection, username: &str) -> Result<(), AppError> {
    let is_member: bool = conn
        .prepare("SELECT COUNT(*) FROM members WHERE username = ?1")?
        .query_row(rusqlite::params![username], |r| r.get::<_, i64>(0))
        .map(|c| c > 0)
        .unwrap_or(false);
    if is_member { Ok(()) } else { Err(AppError::NotFound("Not a member of this group".into())) }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Build the results JSON for one poll: option list with vote counts (never voters —
/// polls are always anonymous), total vote count, and the requesting user's own choices.
fn poll_results(conn: &rusqlite::Connection, poll_id: i64, requester: &str) -> Result<Value, AppError> {
    let (question, multi_choice, closes_at, created_by): (String, bool, Option<i64>, String) = conn
        .prepare("SELECT question, multi_choice, closes_at, created_by FROM polls WHERE id = ?1")?
        .query_row(rusqlite::params![poll_id], |r| {
            Ok((r.get(0)?, r.get::<_, i64>(1)? != 0, r.get(2)?, r.get(3)?))
        })
        .map_err(|_| AppError::NotFound("Poll not found".into()))?;

    let mut opt_stmt = conn.prepare(
        "SELECT id, text FROM poll_options WHERE poll_id = ?1 ORDER BY sort_order, id"
    )?;
    let option_rows: Vec<(i64, String)> = opt_stmt
        .query_map(rusqlite::params![poll_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();

    let mut count_stmt = conn.prepare(
        "SELECT COUNT(*) FROM poll_votes WHERE poll_id = ?1 AND option_id = ?2"
    )?;
    let mut my_votes_stmt = conn.prepare(
        "SELECT option_id FROM poll_votes WHERE poll_id = ?1 AND username = ?2"
    )?;
    let my_option_ids: Vec<i64> = my_votes_stmt
        .query_map(rusqlite::params![poll_id, requester], |r| r.get::<_, i64>(0))?
        .filter_map(|r| r.ok())
        .collect();

    let mut total_votes: i64 = 0;
    let options: Vec<Value> = option_rows
        .iter()
        .map(|(id, text)| {
            let count: i64 = count_stmt
                .query_row(rusqlite::params![poll_id, id], |r| r.get(0))
                .unwrap_or(0);
            total_votes += count;
            json!({
                "id": id,
                "text": text,
                "votes": count,
                "selected_by_me": my_option_ids.contains(id),
            })
        })
        .collect();

    Ok(json!({
        "id": poll_id,
        "question": question,
        "multi_choice": multi_choice,
        "closes_at": closes_at,
        "created_by": created_by,
        "total_votes": total_votes,
        "options": options,
    }))
}

#[derive(Deserialize)]
pub struct CreatePollRequest {
    pub question: String,
    pub options: Vec<String>,
    #[serde(default)]
    pub multi_choice: bool,
    /// Optional close time as unix ms; null/absent = never closes automatically.
    pub closes_at: Option<i64>,
}

/// POST /polls - Create a poll. Moderator+ in channels (read-only audience);
/// any member in groups.
pub async fn create_poll(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Json(req): Json<CreatePollRequest>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;

    let question = req.question.trim().to_string();
    if question.is_empty() || question.len() > 300 {
        return Err(AppError::BadRequest("question must be 1-300 characters".into()));
    }
    let options: Vec<String> = req.options.iter().map(|o| o.trim().to_string()).filter(|o| !o.is_empty()).collect();
    if options.len() < 2 || options.len() > 10 {
        return Err(AppError::BadRequest("poll needs between 2 and 10 options".into()));
    }
    if options.iter().any(|o| o.len() > 100) {
        return Err(AppError::BadRequest("option text too long (max 100 characters)".into()));
    }

    let results = {
        let mut conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;

        require_member(&conn, &username)?;

        let is_channel: bool = conn
            .prepare("SELECT is_channel FROM group_info WHERE id = 1")?
            .query_row([], |r| r.get(0))
            .unwrap_or(false);

        if is_channel && !has_permission(&conn, &username, Permission::CREATE_POLLS)? {
            return Err(AppError::Forbidden("Only channel admins can create polls here".into()));
        }

        let poll_id = {
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO polls (question, multi_choice, closes_at, created_by) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![question, req.multi_choice as i64, req.closes_at, username],
            )?;
            let poll_id = tx.last_insert_rowid();
            for (i, opt) in options.iter().enumerate() {
                tx.execute(
                    "INSERT INTO poll_options (poll_id, text, sort_order) VALUES (?1, ?2, ?3)",
                    rusqlite::params![poll_id, opt, i as i64],
                )?;
            }
            tx.commit()?;
            poll_id
        };

        poll_results(&conn, poll_id, &username)?
    };

    let poll_id = results["id"].as_i64().unwrap_or_default();

    let packet = json!({
        "type": "poll_created",
        "group_id": 1,
        "poll": results,
    });
    state.hub.broadcast_to_all_subscribed(&packet).await;

    Ok(Json(json!({ "ok": true, "poll_id": poll_id })))
}

/// GET /polls - List recent polls (newest first, capped at 50) so clients can
/// show active/past polls after (re)joining a chat instead of relying solely
/// on the live poll_created WS broadcast.
pub async fn list_polls(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;
    let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;
    require_member(&conn, &username)?;

    let poll_ids: Vec<i64> = conn
        .prepare("SELECT id FROM polls ORDER BY id DESC LIMIT 50")?
        .query_map([], |r| r.get::<_, i64>(0))?
        .filter_map(|r| r.ok())
        .collect();

    let polls: Vec<Value> = poll_ids
        .into_iter()
        .filter_map(|id| poll_results(&conn, id, &username).ok())
        .collect();

    Ok(Json(json!(polls)))
}

/// GET /polls/:id - Poll results (any member)
pub async fn get_poll(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(poll_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;
    let conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;
    require_member(&conn, &username)?;

    let results = poll_results(&conn, poll_id, &username)?;
    Ok(Json(results))
}

#[derive(Deserialize)]
pub struct VotePollRequest {
    pub option_id: i64,
}

/// POST /polls/:id/vote - Cast (or change, for single-choice) a vote (any member)
pub async fn vote_poll(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(poll_id): Path<i64>,
    Json(req): Json<VotePollRequest>,
) -> Result<Json<Value>, AppError> {
    let username = auth.0;

    let results = {
        let mut conn = state.db.lock().map_err(|_| AppError::Internal("db lock".into()))?;
        require_member(&conn, &username)?;

        let (multi_choice, closes_at): (bool, Option<i64>) = conn
            .prepare("SELECT multi_choice, closes_at FROM polls WHERE id = ?1")?
            .query_row(rusqlite::params![poll_id], |r| Ok((r.get::<_, i64>(0)? != 0, r.get(1)?)))
            .map_err(|_| AppError::NotFound("Poll not found".into()))?;

        if let Some(closes) = closes_at {
            if now_ms() >= closes {
                return Err(AppError::Forbidden("This poll is closed".into()));
            }
        }

        let option_belongs: bool = conn
            .prepare("SELECT COUNT(*) FROM poll_options WHERE id = ?1 AND poll_id = ?2")?
            .query_row(rusqlite::params![req.option_id, poll_id], |r| r.get::<_, i64>(0))
            .map(|c| c > 0)
            .unwrap_or(false);
        if !option_belongs {
            return Err(AppError::BadRequest("Invalid option for this poll".into()));
        }

        {
            let tx = conn.transaction()?;
            if !multi_choice {
                tx.execute(
                    "DELETE FROM poll_votes WHERE poll_id = ?1 AND username = ?2",
                    rusqlite::params![poll_id, username],
                )?;
            }
            tx.execute(
                "INSERT OR IGNORE INTO poll_votes (poll_id, option_id, username) VALUES (?1, ?2, ?3)",
                rusqlite::params![poll_id, req.option_id, username],
            )?;
            tx.commit()?;
        }

        poll_results(&conn, poll_id, &username)?
    };

    // Broadcast counts only — never who voted, polls are always anonymous.
    let packet = json!({
        "type": "poll_vote_update",
        "group_id": 1,
        "poll_id": poll_id,
        "options": results["options"],
        "total_votes": results["total_votes"],
    });
    state.hub.broadcast_to_all_subscribed(&packet).await;

    Ok(Json(results))
}
