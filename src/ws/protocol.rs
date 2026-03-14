use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Ping,
    SubscribeGroup { group_id: i64 },
    UnsubscribeGroup { group_id: i64 },
    Typing { group_id: i64 },
    Presence { state: String },
}

#[derive(Debug, Serialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Pong,
    InitComplete,
    Error { message: String },
}
