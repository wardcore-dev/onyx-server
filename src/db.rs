use rusqlite::Connection;
use std::sync::{Arc, Mutex};

use crate::models::permission::Permission;

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

        CREATE TABLE IF NOT EXISTS mutes (
            username TEXT PRIMARY KEY,
            muted_by TEXT NOT NULL,
            reason TEXT,
            muted_at TEXT NOT NULL DEFAULT (datetime('now')),
            expires_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS donation_addresses (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            label TEXT NOT NULL,
            address TEXT NOT NULL,
            sort_order INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS polls (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            question TEXT NOT NULL,
            multi_choice INTEGER NOT NULL DEFAULT 0,
            closes_at INTEGER,
            created_by TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            message_id INTEGER
        );

        CREATE TABLE IF NOT EXISTS poll_options (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            poll_id INTEGER NOT NULL,
            text TEXT NOT NULL,
            sort_order INTEGER NOT NULL DEFAULT 0,
            FOREIGN KEY (poll_id) REFERENCES polls(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_poll_options_poll ON poll_options(poll_id);

        CREATE TABLE IF NOT EXISTS poll_votes (
            poll_id INTEGER NOT NULL,
            option_id INTEGER NOT NULL,
            username TEXT NOT NULL,
            voted_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (poll_id, option_id, username),
            FOREIGN KEY (poll_id) REFERENCES polls(id) ON DELETE CASCADE,
            FOREIGN KEY (option_id) REFERENCES poll_options(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_poll_votes_poll ON poll_votes(poll_id);

        CREATE TABLE IF NOT EXISTS roles (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            color TEXT NOT NULL DEFAULT '#99AAB5',
            permissions INTEGER NOT NULL DEFAULT 0,
            is_system INTEGER NOT NULL DEFAULT 0,
            sort_order INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_roles_name ON roles(name);

        CREATE TABLE IF NOT EXISTS join_requests (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            username TEXT NOT NULL UNIQUE,
            display_name TEXT NOT NULL,
            public_key TEXT NOT NULL DEFAULT '',
            requested_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- Telegram-style comment threads under a channel post. Kept
        -- separate from `messages` so channel history queries never need to
        -- filter comments out of the main feed.
        CREATE TABLE IF NOT EXISTS comments (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            post_id INTEGER NOT NULL,
            sender_username TEXT NOT NULL,
            content TEXT NOT NULL,
            reply_to_id INTEGER,
            reply_to_sender TEXT,
            reply_to_content TEXT,
            timestamp TEXT NOT NULL DEFAULT (datetime('now')),
            timestamp_ms INTEGER NOT NULL DEFAULT 0,
            FOREIGN KEY (post_id) REFERENCES messages(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_comments_post ON comments(post_id, timestamp_ms);

        CREATE TABLE IF NOT EXISTS comment_reactions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            comment_id INTEGER NOT NULL,
            reactor_username TEXT NOT NULL,
            emoji TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(comment_id, reactor_username, emoji),
            FOREIGN KEY (comment_id) REFERENCES comments(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_comment_reactions_comment ON comment_reactions(comment_id);
        "

    ).map_err(|e| format!("Failed to create tables: {}", e))?;

    Ok(())
}

/// Check whether `table` has a column named `column`. Table/column names here
/// are always Rust string literals we control, never user input, so this
/// format! usage is safe — no SQL injection risk.
fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, String> {
    conn.prepare(&format!("SELECT COUNT(*) FROM pragma_table_info('{}') WHERE name = '{}'", table, column))
        .and_then(|mut s| s.query_row([], |r| r.get::<_, i64>(0)))
        .map(|c| c > 0)
        .map_err(|e| e.to_string())
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

    // Add slow_mode_seconds column (min seconds between messages per non-mod member, 0 = off)
    let has_slow_mode: bool = conn
        .prepare("SELECT COUNT(*) FROM pragma_table_info('group_info') WHERE name = 'slow_mode_seconds'")
        .and_then(|mut s| s.query_row([], |r| r.get::<_, i64>(0)))
        .map(|c| c > 0)
        .unwrap_or(false);

    if !has_slow_mode {
        conn.execute_batch("ALTER TABLE group_info ADD COLUMN slow_mode_seconds INTEGER NOT NULL DEFAULT 0;")
            .map_err(|e| format!("Migration failed (slow_mode_seconds): {}", e))?;
    }

    // Seed the 3 system roles (Owner, Moderator, Member)
    let moderator_permissions = (Permission::KICK_MEMBERS
        | Permission::BAN_MEMBERS
        | Permission::MUTE_MEMBERS
        | Permission::CREATE_POLLS
        | Permission::POST_IN_CHANNEL
        | Permission::DELETE_MESSAGES
        | Permission::MANAGE_MEMBERS
        | Permission::MANAGE_SLOW_MODE
        | Permission::VIEW_BAN_LIST
        | Permission::VIEW_MUTE_LIST
        | Permission::VIEW_INVITE_LINK)
        .bits();
    conn.execute(
        "INSERT OR IGNORE INTO roles (id, name, color, permissions, is_system, sort_order) VALUES (1, 'Owner', '#e67e22', ?1, 1, 0)",
        rusqlite::params![Permission::all().bits()],
    ).map_err(|e| format!("Migration failed (seed owner role): {}", e))?;
    conn.execute(
        "INSERT OR IGNORE INTO roles (id, name, color, permissions, is_system, sort_order) VALUES (2, 'Moderator', '#3498db', ?1, 1, 1)",
        rusqlite::params![moderator_permissions],
    ).map_err(|e| format!("Migration failed (seed moderator role): {}", e))?;
    conn.execute(
        "INSERT OR IGNORE INTO roles (id, name, color, permissions, is_system, sort_order) VALUES (3, 'Member', '#99AAB5', 0, 1, 2)",
        [],
    ).map_err(|e| format!("Migration failed (seed member role): {}", e))?;

    // Add role_id column to members, backed by the roles table
    if !has_column(conn, "members", "role_id")? {
        conn.execute_batch("ALTER TABLE members ADD COLUMN role_id INTEGER REFERENCES roles(id);")
            .map_err(|e| format!("Migration failed (role_id): {}", e))?;
    }

    // Backfill role_id from the legacy role string column (idempotent — only touches NULL rows)
    conn.execute_batch(
        "UPDATE members SET role_id = 1 WHERE role_id IS NULL AND role = 'owner';
         UPDATE members SET role_id = 2 WHERE role_id IS NULL AND role = 'moderator';
         UPDATE members SET role_id = 3 WHERE role_id IS NULL;"
    ).map_err(|e| format!("Migration failed (role_id backfill): {}", e))?;

    // Add new group_info columns for server-managed settings
    if !has_column(conn, "group_info", "require_approval")? {
        conn.execute_batch("ALTER TABLE group_info ADD COLUMN require_approval INTEGER NOT NULL DEFAULT 0;")
            .map_err(|e| format!("Migration failed (require_approval): {}", e))?;
    }
    if !has_column(conn, "group_info", "default_role_id")? {
        conn.execute_batch("ALTER TABLE group_info ADD COLUMN default_role_id INTEGER REFERENCES roles(id);")
            .map_err(|e| format!("Migration failed (default_role_id): {}", e))?;
    }
    conn.execute_batch("UPDATE group_info SET default_role_id = 3 WHERE default_role_id IS NULL;")
        .map_err(|e| format!("Migration failed (default_role_id backfill): {}", e))?;
    if !has_column(conn, "group_info", "max_members")? {
        conn.execute_batch("ALTER TABLE group_info ADD COLUMN max_members INTEGER NOT NULL DEFAULT 500;")
            .map_err(|e| format!("Migration failed (max_members): {}", e))?;
    }
    if !has_column(conn, "group_info", "motd")? {
        conn.execute_batch("ALTER TABLE group_info ADD COLUMN motd TEXT NOT NULL DEFAULT '';")
            .map_err(|e| format!("Migration failed (motd): {}", e))?;
    }
    if !has_column(conn, "group_info", "max_messages_per_minute")? {
        conn.execute_batch("ALTER TABLE group_info ADD COLUMN max_messages_per_minute INTEGER NOT NULL DEFAULT 30;")
            .map_err(|e| format!("Migration failed (max_messages_per_minute): {}", e))?;
    }

    Ok(())
}
