use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: i64,
    pub sender_username: String,
    pub content: String,
    pub reply_to_id: Option<i64>,
    pub reply_to_sender: Option<String>,
    pub reply_to_content: Option<String>,
    pub timestamp: String,
    pub timestamp_ms: i64,
}

#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    pub content: String,
    pub reply_to_id: Option<i64>,
    pub reply_to_sender: Option<String>,
    pub reply_to_content: Option<String>,
}
