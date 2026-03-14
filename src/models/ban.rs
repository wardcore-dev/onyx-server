use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ban {
    pub id: i64,
    pub username: String,
    pub banned_by: String,
    pub reason: Option<String>,
    pub banned_at: String,
}

#[derive(Debug, Deserialize)]
pub struct BanRequest {
    pub reason: Option<String>,
}
