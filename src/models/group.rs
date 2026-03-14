use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub id: i64,
    pub name: String,
    pub is_channel: bool,
    pub owner_username: String,
    pub invite_token: String,
    pub avatar_version: i32,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateGroupRequest {
    pub name: String,
    #[serde(default)]
    pub is_channel: bool,
}
