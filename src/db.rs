use rusqlite::Connection;
use std::sync::{Arc, Mutex};

pub type Db = Arc<Mutex<Connection>>;

pub fn init(path: &str) -> Result<Db, String> {
    let conn = Connection::open(path)
        .map_err(|e| format!("Failed to open database '{}': {}", path, e))?;

    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
        .map_err(|e| format!("Failed to set pragmas: {}", e))?;

    create_tables(&conn)?;
    run_migrations(&conn)?;
    Ok(Arc::new(Mutex::new(conn)))
}

fn create_tables(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS group_info (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            name TEXT NOT NULL,
            description TEXT DEFAULT '',
            is_channel INTEGER NOT NULL DEFAULT 0,
            owner_username TEXT NOT NULL,
            invite_token TEXT NOT NULL UNIQUE,
            public_channel_token TEXT UNIQUE,
            avatar_version INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS members (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            username TEXT NOT NULL UNIQUE,
            display_name TEXT NOT NULL,
            public_key TEXT NOT NULL DEFAULT '',
            role TEXT NOT NULL DEFAULT 'member',
            joined_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS sessions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            username TEXT NOT NULL UNIQUE,
            display_name TEXT NOT NULL DEFAULT '',
            token TEXT NOT NULL UNIQUE,
            public_key TEXT NOT NULL,
            password_hash TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_seen TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_sessions_token ON sessions(token);

        CREATE TABLE IF NOT EXISTS device_tokens (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            username TEXT NOT NULL,
            token TEXT NOT NULL UNIQUE,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_seen TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_device_tokens_token ON device_tokens(token);
        CREATE INDEX IF NOT EXISTS idx_device_tokens_username ON device_tokens(username);

        CREATE TABLE IF NOT EXISTS messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            sender_username TEXT NOT NULL,
            content TEXT NOT NULL,
            reply_to_id INTEGER,
            reply_to_sender TEXT,
            reply_to_content TEXT,
            timestamp TEXT NOT NULL DEFAULT (datetime('now')),
            timestamp_ms INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_messages_ts ON messages(timestamp_ms DESC);

        CREATE TABLE IF NOT EXISTS bans (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            username TEXT NOT NULL UNIQUE,
            banned_by TEXT NOT NULL,
            reason TEXT,
            banned_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS media (
            id TEXT PRIMARY KEY,
            uploader_username TEXT NOT NULL,
            original_filename TEXT NOT NULL,
            mime_type TEXT NOT NULL,
            size_bytes INTEGER NOT NULL,
            storage_path TEXT NOT NULL,
            provider TEXT NOT NULL,
            uploaded_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS group_avatar (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            data BLOB NOT NULL,
            mime_type TEXT NOT NULL DEFAULT 'image/png',
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS post_views (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            message_id INTEGER NOT NULL,
            viewer_identifier TEXT NOT NULL,
            viewed_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(message_id, viewer_identifier)
        );
        CREATE INDEX IF NOT EXISTS idx_post_views_msg ON post_views(message_id);

        CREATE TABLE IF NOT EXISTS message_reactions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            message_id INTEGER NOT NULL,
            reactor_username TEXT NOT NULL,
            emoji TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(message_id, reactor_username, emoji),
            FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_reactions_msg ON message_reactions(message_id);
        "

    ).map_err(|e| format!("Failed to create tables: {}", e))?;

    Ok(())
}

fn run_migrations(conn: &Connection) -> Result<(), String> {
    // Add password_hash column to existing sessions tables
    let has_pw_hash: bool = conn
        .prepare("SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'password_hash'")
        .and_then(|mut s| s.query_row([], |r| r.get::<_, i64>(0)))
        .map(|c| c > 0)
        .unwrap_or(false);

    if !has_pw_hash {
        conn.execute_batch("ALTER TABLE sessions ADD COLUMN password_hash TEXT NOT NULL DEFAULT '';")
            .map_err(|e| format!("Migration failed (password_hash): {}", e))?;
    }

    // Add public_channel_token column to existing group_info tables
    let has_public_token: bool = conn
        .prepare("SELECT COUNT(*) FROM pragma_table_info('group_info') WHERE name = 'public_channel_token'")
        .and_then(|mut s| s.query_row([], |r| r.get::<_, i64>(0)))
        .map(|c| c > 0)
        .unwrap_or(false);

    if !has_public_token {
        conn.execute_batch("ALTER TABLE group_info ADD COLUMN public_channel_token TEXT UNIQUE;")
            .map_err(|e| format!("Migration failed (public_channel_token): {}", e))?;
    }

    // Add max_message_length column
    let has_max_msg_len: bool = conn
        .prepare("SELECT COUNT(*) FROM pragma_table_info('group_info') WHERE name = 'max_message_length'")
        .and_then(|mut s| s.query_row([], |r| r.get::<_, i64>(0)))
        .map(|c| c > 0)
        .unwrap_or(false);

    if !has_max_msg_len {
        conn.execute_batch("ALTER TABLE group_info ADD COLUMN max_message_length INTEGER DEFAULT 5000;")
            .map_err(|e| format!("Migration failed (max_message_length): {}", e))?;
    }

    // Add media_provider column
    let has_media_provider: bool = conn
        .prepare("SELECT COUNT(*) FROM pragma_table_info('group_info') WHERE name = 'media_provider'")
        .and_then(|mut s| s.query_row([], |r| r.get::<_, i64>(0)))
        .map(|c| c > 0)
        .unwrap_or(false);

    if !has_media_provider {
        conn.execute_batch("ALTER TABLE group_info ADD COLUMN media_provider TEXT DEFAULT 'local';")
            .map_err(|e| format!("Migration failed (media_provider): {}", e))?;
    }

    // Add max_file_size column (in bytes)
    let has_max_file_size: bool = conn
        .prepare("SELECT COUNT(*) FROM pragma_table_info('group_info') WHERE name = 'max_file_size'")
        .and_then(|mut s| s.query_row([], |r| r.get::<_, i64>(0)))
        .map(|c| c > 0)
        .unwrap_or(false);

    if !has_max_file_size {
        conn.execute_batch("ALTER TABLE group_info ADD COLUMN max_file_size INTEGER DEFAULT 10485760;")
            .map_err(|e| format!("Migration failed (max_file_size): {}", e))?;
    }

    // Add allowed_file_types column
    let has_allowed_types: bool = conn
        .prepare("SELECT COUNT(*) FROM pragma_table_info('group_info') WHERE name = 'allowed_file_types'")
        .and_then(|mut s| s.query_row([], |r| r.get::<_, i64>(0)))
        .map(|c| c > 0)
        .unwrap_or(false);

    if !has_allowed_types {
        conn.execute_batch("ALTER TABLE group_info ADD COLUMN allowed_file_types TEXT DEFAULT 'image/*,video/*,audio/*,application/pdf';")
            .map_err(|e| format!("Migration failed (allowed_file_types): {}", e))?;
    }

    // Add rate_limit column (messages per minute, 0 = no limit)
    let has_rate_limit: bool = conn
        .prepare("SELECT COUNT(*) FROM pragma_table_info('group_info') WHERE name = 'rate_limit'")
        .and_then(|mut s| s.query_row([], |r| r.get::<_, i64>(0)))
        .map(|c| c > 0)
        .unwrap_or(false);

    if !has_rate_limit {
        conn.execute_batch("ALTER TABLE group_info ADD COLUMN rate_limit INTEGER DEFAULT 0;")
            .map_err(|e| format!("Migration failed (rate_limit): {}", e))?;
    }

    Ok(())
}
