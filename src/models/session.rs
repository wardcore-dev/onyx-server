use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub token: String,
    pub public_key: String,
    pub created_at: String,
    pub last_seen: String,
}

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub display_name: String,
    pub public_key: String,
    #[serde(default)]
    pub password_hash: String,
}

#[derive(Debug, Deserialize)]
pub struct ChallengeRequest {
    pub username: String,
    pub public_key: String,
}

#[derive(Debug, Deserialize)]
pub struct VerifyRequest {
    pub username: String,
    pub signature: String,
}
