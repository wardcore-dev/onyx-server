use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    #[serde(default)]
    pub database: DatabaseConfig,
    #[serde(default)]
    pub media: MediaConfig,
    #[serde(default)]
    pub moderation: ModerationConfig,
    #[serde(default)]
    pub security: SecurityConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_name")]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_bind")]
    pub bind_address: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub public_url: Option<String>,
    #[serde(default = "default_max_members")]
    pub max_members_per_group: u32,
    #[serde(default)]
    pub max_groups: u32,
    #[serde(default = "default_max_msg_len")]
    pub max_message_length: u32,
    #[serde(default)]
    pub motd: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    #[serde(default = "default_db_path")]
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaConfig {
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default = "default_max_file_size")]
    pub max_file_size_mb: u32,
    #[serde(default = "default_allowed_types")]
    pub allowed_types: Vec<String>,
    #[serde(default)]
    pub local: MediaLocalConfig,
    #[serde(default)]
    pub custom: MediaCustomConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MediaLocalConfig {
    #[serde(default = "default_storage_path")]
    pub storage_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MediaCustomConfig {
    #[serde(default)]
    pub upload_url: String,
    #[serde(default = "default_field_name")]
    pub upload_field_name: String,
    #[serde(default = "default_jsonpath")]
    pub response_url_jsonpath: String,
    #[serde(default)]
    pub extra_headers: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModerationConfig {
    #[serde(default = "default_true")]
    pub enable_moderators: bool,
    #[serde(default = "default_role")]
    pub default_role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    #[serde(default = "default_rate_msg")]
    pub max_messages_per_minute: u32,
    #[serde(default = "default_rate_join")]
    pub max_joins_per_minute: u32,
    #[serde(default)]
    pub require_approval: bool,
    /// Explicit list of allowed CORS origins (e.g. ["https://app.example.com"]).
    /// If empty, falls back to public_url when set, otherwise allows any origin.
    #[serde(default)]
    pub allowed_origins: Vec<String>,
}

fn default_name() -> String { "My ONYX Server".into() }
fn default_bind() -> String { "0.0.0.0".into() }
fn default_port() -> u16 { 9090 }
fn default_max_members() -> u32 { 500 }
fn default_max_msg_len() -> u32 { 4096 }
fn default_db_path() -> String { "./data/onyx-server.db".into() }
fn default_provider() -> String { "local".into() }
fn default_max_file_size() -> u32 { 50 }
fn default_allowed_types() -> Vec<String> {
    vec!["image".into(), "video".into(), "audio".into(), "file".into()]
}
fn default_storage_path() -> String { "./data/media".into() }
fn default_field_name() -> String { "file".into() }
fn default_jsonpath() -> String { "$.url".into() }
fn default_true() -> bool { true }
fn default_role() -> String { "member".into() }
fn default_rate_msg() -> u32 { 30 }
fn default_rate_join() -> u32 { 10 }

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            name: default_name(),
            description: String::new(),
            bind_address: default_bind(),
            port: default_port(),
            public_url: None,
            max_members_per_group: default_max_members(),
            max_groups: 0,
            max_message_length: default_max_msg_len(),
            motd: String::new(),
        }
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self { path: default_db_path() }
    }
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            provider: default_provider(),
            max_file_size_mb: default_max_file_size(),
            allowed_types: default_allowed_types(),
            local: MediaLocalConfig::default(),
            custom: MediaCustomConfig::default(),
        }
    }
}

impl Default for ModerationConfig {
    fn default() -> Self {
        Self {
            enable_moderators: true,
            default_role: default_role(),
        }
    }
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            max_messages_per_minute: default_rate_msg(),
            max_joins_per_minute: default_rate_join(),
            require_approval: false,
            allowed_origins: Vec::new(),
        }
    }
}

impl Config {
    pub fn load(path: &str) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read config file '{}': {}", path, e))?;
        toml::from_str(&content)
            .map_err(|e| format!("Failed to parse config: {}", e))
    }

    pub fn default_toml() -> String {
        r#"[server]
name = "My ONYX Server"
description = "A self-hosted group server"
bind_address = "0.0.0.0"
port = 9090
max_members_per_group = 500
max_groups = 0
max_message_length = 4096
motd = ""

[database]
path = "./data/onyx-server.db"

[media]
provider = "local"
max_file_size_mb = 50
allowed_types = ["image", "video", "audio", "file"]

[media.local]
storage_path = "./data/media"

[media.custom]
upload_url = ""
upload_field_name = "file"
response_url_jsonpath = "$.url"

[moderation]
enable_moderators = true
default_role = "member"

[security]
max_messages_per_minute = 30
max_joins_per_minute = 10
require_approval = false
# Restrict CORS to specific origins, e.g. ["https://app.example.com"]
# Leave empty to allow any origin (suitable for LAN/desktop-app deployments)
allowed_origins = []
"#.to_string()
    }

    pub fn ensure_directories(&self) -> Result<(), String> {
        let db_dir = Path::new(&self.database.path).parent()
            .ok_or("Invalid database path")?;
        std::fs::create_dir_all(db_dir)
            .map_err(|e| format!("Failed to create database directory: {}", e))?;

        if self.media.provider == "local" {
            std::fs::create_dir_all(&self.media.local.storage_path)
                .map_err(|e| format!("Failed to create media directory: {}", e))?;
        }
        Ok(())
    }

    pub fn save(&self, path: &str) -> Result<(), String> {
        let content = toml::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;
        std::fs::write(path, content)
            .map_err(|e| format!("Failed to write config file '{}': {}", path, e))?;
        Ok(())
    }

    pub fn update_description(&mut self, description: String) {
        self.server.description = description;
    }

    pub fn update_max_message_length(&mut self, length: u32) {
        self.server.max_message_length = length;
    }

    pub fn update_media_provider(&mut self, provider: String) {
        self.media.provider = provider;
    }

    pub fn update_max_file_size(&mut self, size_mb: u32) {
        self.media.max_file_size_mb = size_mb;
    }

    pub fn update_allowed_file_types(&mut self, types: Vec<String>) {
        self.media.allowed_types = types;
    }

    pub fn update_rate_limit(&mut self, limit: u32) {
        self.security.max_messages_per_minute = limit;
    }
}
