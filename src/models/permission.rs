use bitflags::bitflags;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Permission: i64 {
        const KICK_MEMBERS          = 1 << 0;
        const BAN_MEMBERS           = 1 << 1;
        const MUTE_MEMBERS          = 1 << 2;
        const MANAGE_ROLES          = 1 << 3;
        const MANAGE_SETTINGS       = 1 << 4;
        const MANAGE_DONATIONS      = 1 << 5;
        const CREATE_POLLS          = 1 << 6;
        const POST_IN_CHANNEL       = 1 << 7;
        const DELETE_MESSAGES       = 1 << 8;
        const MANAGE_MEMBERS        = 1 << 10;
        const MANAGE_SLOW_MODE      = 1 << 11;
        const VIEW_BAN_LIST         = 1 << 12;
        const VIEW_MUTE_LIST        = 1 << 13;
        const VIEW_INVITE_LINK      = 1 << 14;
    }
}

impl Permission {
    /// Parse a permission from its lowercase snake_case wire name (e.g. "kick_members").
    /// Named `from_wire_name` (not `from_name`) to avoid clashing with the inherent
    /// `from_name` that bitflags 2.x's `Flags` trait already generates for this type.
    pub fn from_wire_name(s: &str) -> Option<Self> {
        match s {
            "kick_members" => Some(Self::KICK_MEMBERS),
            "ban_members" => Some(Self::BAN_MEMBERS),
            "mute_members" => Some(Self::MUTE_MEMBERS),
            "manage_roles" => Some(Self::MANAGE_ROLES),
            "manage_settings" => Some(Self::MANAGE_SETTINGS),
            "manage_donations" => Some(Self::MANAGE_DONATIONS),
            "create_polls" => Some(Self::CREATE_POLLS),
            "post_in_channel" => Some(Self::POST_IN_CHANNEL),
            "delete_messages" => Some(Self::DELETE_MESSAGES),
            "manage_members" => Some(Self::MANAGE_MEMBERS),
            "manage_slow_mode" => Some(Self::MANAGE_SLOW_MODE),
            "view_ban_list" => Some(Self::VIEW_BAN_LIST),
            "view_mute_list" => Some(Self::VIEW_MUTE_LIST),
            "view_invite_link" => Some(Self::VIEW_INVITE_LINK),
            _ => None,
        }
    }

    /// All flags with their wire names, for JSON serialization of a permission set.
    pub const ALL_NAMED: &'static [(&'static str, Permission)] = &[
        ("kick_members", Self::KICK_MEMBERS),
        ("ban_members", Self::BAN_MEMBERS),
        ("mute_members", Self::MUTE_MEMBERS),
        ("manage_roles", Self::MANAGE_ROLES),
        ("manage_settings", Self::MANAGE_SETTINGS),
        ("manage_donations", Self::MANAGE_DONATIONS),
        ("create_polls", Self::CREATE_POLLS),
        ("post_in_channel", Self::POST_IN_CHANNEL),
        ("delete_messages", Self::DELETE_MESSAGES),
        ("manage_members", Self::MANAGE_MEMBERS),
        ("manage_slow_mode", Self::MANAGE_SLOW_MODE),
        ("view_ban_list", Self::VIEW_BAN_LIST),
        ("view_mute_list", Self::VIEW_MUTE_LIST),
        ("view_invite_link", Self::VIEW_INVITE_LINK),
    ];

    /// Convert to a JSON array of wire names for API responses.
    pub fn to_names(self) -> Vec<String> {
        Self::ALL_NAMED
            .iter()
            .filter(|(_, flag)| self.contains(*flag))
            .map(|(name, _)| name.to_string())
            .collect()
    }

    /// Parse a JSON array of wire names into a permission set. Returns Err on any unknown name.
    pub fn from_names(names: &[String]) -> Result<Self, String> {
        let mut result = Self::empty();
        for name in names {
            match Self::from_wire_name(name) {
                Some(flag) => result |= flag,
                None => return Err(format!("Unknown permission: {}", name)),
            }
        }
        Ok(result)
    }
}
