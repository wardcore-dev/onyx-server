use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "onyx-server", about = "ONYX self-hosted group/channel server (one group per server instance)")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Server name (e.g., 'gaming' instead of full path)
    #[arg(long, global = true)]
    pub server: Option<String>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Create server launchers - the easiest way to run ONYX servers!
    ///
    /// Creates a simple launcher file (.bat or .sh) that you can double-click to start your server.
    /// Each server runs in its own window.
    ///
    /// Example: onyx-server server create gaming
    /// Example: onyx-server server create "My Server" --port 3000
    Server {
        #[command(subcommand)]
        action: ServerAction,
    },

    /// [INTERNAL] Start server (used by launchers)
    #[command(hide = true)]
    Serve {
        /// Override port from config
        #[arg(long)]
        port: Option<u16>,
    },

    /// Manage group settings
    ///
    /// View/edit group name, get invite tokens, convert between group/channel mode.
    /// Use --config to manage a specific server.
    ///
    /// Example: onyx-server --config .onyx/configs/gaming.toml group info
    Group {
        #[command(subcommand)]
        action: GroupAction,
    },

    /// Manage server members
    ///
    /// List, add, remove, ban members and assign roles (owner, moderator, member).
    /// Use --config to manage a specific server/group/channel.
    ///
    /// Examples:
    ///   onyx-server --config .onyx/configs/gaming.toml member list
    ///   onyx-server --config .onyx/configs/gaming.toml member role owner alice
    ///   onyx-server --config .onyx/configs/tech.toml member role moderator bob
    Member {
        #[command(subcommand)]
        action: MemberAction,
    },

    /// Show server statistics
    ///
    /// Displays group name, member count, message count, etc.
    /// Use --config to view a specific server.
    ///
    /// Example: onyx-server --config .onyx/configs/gaming.toml info
    Info,

    /// Show step-by-step setup guide for beginners
    ///
    /// This command displays a complete guide on how to set up and use ONYX server.
    /// Perfect for first-time users!
    Guide,

}

#[derive(Subcommand)]
pub enum ServerAction {
    /// Create a new server launcher (interactive mode)
    ///
    /// Run without arguments for interactive setup with step-by-step questions.
    ///
    /// Interactive: onyx-server server create
    /// Direct:      onyx-server server create --name gaming --type group --port 3000
    Create {
        /// Server name (will be asked if not provided)
        #[arg(long)]
        name: Option<String>,
        /// Type: group or channel (will be asked if not provided)
        #[arg(long)]
        type_: Option<String>,
        /// Port for this server (auto-assigned if not specified)
        #[arg(long)]
        port: Option<u16>,
        /// Group/channel name (defaults to server name if not specified)
        #[arg(long)]
        group_name: Option<String>,
        /// Target OS for launcher (windows, linux, auto). Default: auto (current OS)
        #[arg(long, default_value = "auto")]
        os: String,
    },

}

#[derive(Subcommand)]
pub enum GroupAction {
    /// Show group info
    Info,
    /// Create or rename a group/channel
    ///
    /// Groups: All members can post messages
    /// Channels: Only the owner can post messages (broadcast-style)
    ///
    /// Example: onyx-server group setup "My Channel" --channel
    Setup {
        /// Group or channel name
        name: String,
        /// Create as channel (only owner can post).
        /// In channel mode, only the owner can send messages while all members can read.
        #[arg(long)]
        channel: bool,
    },
    /// Show invite token for joining the group
    Invite,
    /// Generate or show public channel token (allows viewing channel without registration)
    ///
    /// Public channels can be viewed by anyone with the token, without requiring authentication.
    /// This only works if the group is configured as a channel (--channel flag).
    ///
    /// Example: onyx-server group public-token
    PublicToken,
    /// Convert group to channel (only owner can post)
    ///
    /// Example: onyx-server group to-channel
    ToChannel,
    /// Convert channel to group (all members can post)
    ///
    /// Example: onyx-server group to-group
    ToGroup,
}

#[derive(Subcommand)]
pub enum MemberAction {
    /// List members
    List,
    /// Add member manually (usually auto-added on register)
    Add {
        /// Username (supports spaces)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        username_parts: Vec<String>,
        /// Display name
        #[arg(long)]
        display_name: Option<String>,
    },
    /// Kick a member
    Kick {
        /// Username to kick (supports spaces)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        username_parts: Vec<String>,
    },
    /// Ban a member
    Ban {
        /// Username to ban (supports spaces)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        username_parts: Vec<String>,
        /// Reason for ban
        #[arg(long)]
        reason: Option<String>,
    },
    /// Unban a member
    Unban {
        /// Username to unban (supports spaces)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        username_parts: Vec<String>,
    },
    /// Set member role for a specific group/channel
    ///
    /// Assign owner, moderator, or member role to users.
    /// Use --config to specify which server/group/channel.
    ///
    /// Example: onyx-server --config .onyx/configs/gaming.toml member role owner alice
    ///          onyx-server --config .onyx/configs/tech.toml member role moderator "user 2"
    Role {
        /// Role: owner, moderator, member
        role: String,
        /// Username (supports spaces if quoted, or captures all remaining args)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        username_parts: Vec<String>,
    },
}

