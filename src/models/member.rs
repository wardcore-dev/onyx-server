use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Moderator,
    Member,
}

impl Role {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "owner" => Some(Role::Owner),
            "moderator" => Some(Role::Moderator),
            "member" => Some(Role::Member),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Role::Owner => "owner",
            Role::Moderator => "moderator",
            Role::Member => "member",
        }
    }

    pub fn can_moderate(&self) -> bool {
        matches!(self, Role::Owner | Role::Moderator)
    }

    pub fn is_owner(&self) -> bool {
        matches!(self, Role::Owner)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Member {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub public_key: String,
    pub role: String,
    pub joined_at: String,
}
