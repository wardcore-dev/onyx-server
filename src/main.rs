#![allow(dead_code)]

mod auth;
mod cli;
mod config;
mod db;
mod error;
mod handlers;
mod instance_config;
mod media;
mod models;
mod moderation;
mod pm;
mod server;
mod ws;
mod menu;

use clap::Parser;
use tracing::info;
use colored::*;

use cli::{Cli, Commands, GroupAction, MemberAction, ServerAction};
use config::Config;
use menu::*;

#[tokio::main]
async fn main() {
    // Enable ANSI colors in Windows console
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;

        unsafe {
            let handle = std::io::stdout().as_raw_handle();
            let mut mode: u32 = 0;

            // Get current console mode
            if winapi::um::consoleapi::GetConsoleMode(handle as *mut _, &mut mode) != 0 {
                // Enable virtual terminal processing
                winapi::um::consoleapi::SetConsoleMode(
                    handle as *mut _,
                    mode | winapi::um::wincon::ENABLE_VIRTUAL_TERMINAL_PROCESSING,
                );
            }
        }
    }

    tracing_subscriber::fmt::init();

    let args: Vec<String> = std::env::args().collect();

    // If launched with "serve" command (from launcher), run server directly
    if args.iter().any(|arg| arg == "serve") {
        let cli = Cli::parse();
        if let Commands::Serve { port } = cli.command {
            let config_path = if let Some(server_name) = &cli.server {
                format!(".onyx/configs/{}.toml", server_name)
            } else {
                "config.toml".to_string()
            };
            cmd_serve(&config_path, port).await;
        }
        return;
    }

    // Interactive menu mode
    loop {
        let choice = show_main_menu().await;

        match choice {
            1 => interactive_create_server().await,
            2 => interactive_view_all_servers().await,
            3 => interactive_manage_server().await,
            4 => interactive_guide().await,
            5 => {
                clear_screen();
                println!("\n{}", "Thank you for using ONYX Server!".bright_cyan().bold());
                print_footer();
                break;
            }
            _ => {}
        }
    }
}

fn cmd_init(config_path: &str, group_name: Option<String>, owner: Option<String>) {
    if std::path::Path::new(config_path).exists() {
        println!("Config file '{}' already exists. Delete it first if you want to reinitialize.", config_path);
        return;
    }

    std::fs::write(config_path, Config::default_toml())
        .expect("Failed to write config file");

    std::fs::create_dir_all("./data/media")
        .expect("Failed to create data directories");

    println!("Created config file: {}", config_path);
    println!("Created data directories: ./data/ ./data/media/");

    let config = Config::load(config_path).unwrap();
    let database = db::init(&config.database.path).unwrap();
    let conn = database.lock().unwrap();

    let group_name = group_name.unwrap_or_else(|| {
        println!("\nEnter group name (or press Enter for 'My ONYX Server'): ");
        let input = std::io::stdin().lines().next().unwrap().unwrap().trim().to_string();
        if input.is_empty() { "My ONYX Server".to_string() } else { input }
    });

    let owner = owner.unwrap_or_else(|| "admin".to_string());
    let invite_token = uuid::Uuid::new_v4().to_string();

    conn.execute(
        "INSERT OR REPLACE INTO group_info (id, name, description, is_channel, owner_username, invite_token, avatar_version, created_at)
         VALUES (1, ?1, '', 0, ?2, ?3, 1, datetime('now'))",
        rusqlite::params![group_name, owner, invite_token],
    ).expect("Failed to create group");

    println!("\nInitialized group '{}' (owner: {})", group_name, owner);
    println!("Invite token: {}", invite_token);
    println!("\nRun 'onyx-server serve' to start the server.");
}

async fn cmd_serve(config_path: &str, port_override: Option<u16>) {
    let mut config = Config::load(config_path)
        .unwrap_or_else(|e| {
            eprintln!("Error: {}", e);
            eprintln!("Run 'onyx-server init' to create a default config file.");
            std::process::exit(1);
        });

    if let Some(port) = port_override {
        config.server.port = port;
    }

    config.ensure_directories()
        .unwrap_or_else(|e| {
            eprintln!("Error creating directories: {}", e);
            std::process::exit(1);
        });

    let database = db::init(&config.database.path)
        .unwrap_or_else(|e| {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        });

    info!("Database initialized at {}", config.database.path);

    if let Err(e) = server::start(config, database).await {
        eprintln!("Server error: {}", e);
        std::process::exit(1);
    }
}

fn cmd_group(config_path: &str, action: GroupAction) {
    let config = load_config_or_exit(config_path);
    let database = open_db_or_exit(&config.database.path);
    let conn = database.lock().unwrap();

    match action {
        GroupAction::Info => {
            match conn.query_row(
                "SELECT id, name, description, is_channel, owner_username, invite_token, avatar_version, created_at FROM group_info WHERE id = 1",
                [],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, bool>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, i64>(6)?,
                        r.get::<_, String>(7)?,
                    ))
                },
            ) {
                Ok((id, name, desc, is_channel, owner, token, avatar_ver, created)) => {
                    let member_count: i64 = conn.query_row(
                        "SELECT COUNT(*) FROM members",
                        [],
                        |r| r.get(0),
                    ).unwrap_or(0);

                    let gtype = if is_channel { "Channel" } else { "Group" };
                    println!("Group ID:       {}", id);
                    println!("Name:           {}", name);
                    println!("Type:           {}", gtype);
                    println!("Description:    {}", if desc.is_empty() { "(none)" } else { &desc });
                    println!("Owner:          {}", owner);
                    println!("Members:        {}", member_count);
                    println!("Avatar version: {}", avatar_ver);
                    println!("Created:        {}", created);
                    println!("Invite token:   {}", token);
                }
                Err(_) => println!("Group not initialized. Run 'onyx-server init' first."),
            }
        }

        GroupAction::Setup { name, channel } => {
            let exists: bool = conn.query_row(
                "SELECT COUNT(*) FROM group_info WHERE id = 1",
                [],
                |r| Ok(r.get::<_, i64>(0)? > 0),
            ).unwrap_or(false);

            if exists {
                // If converting to channel, generate public_token if not exists
                if channel {
                    let public_token: Option<String> = conn.query_row(
                        "SELECT public_channel_token FROM group_info WHERE id = 1",
                        [],
                        |r| r.get(0),
                    ).ok().flatten();

                    let token = public_token.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

                    conn.execute(
                        "UPDATE group_info SET name = ?1, is_channel = ?2, public_channel_token = ?3 WHERE id = 1",
                        rusqlite::params![name, channel, token],
                    ).unwrap();

                    println!("Updated channel '{}'", name);
                    println!("\nPublic channel token:");
                    println!("{}", token);
                } else {
                    conn.execute(
                        "UPDATE group_info SET name = ?1, is_channel = ?2 WHERE id = 1",
                        rusqlite::params![name, channel],
                    ).unwrap();
                    println!("Updated group '{}'", name);
                }
            } else {
                let invite_token = uuid::Uuid::new_v4().to_string();
                let public_token = if channel { Some(uuid::Uuid::new_v4().to_string()) } else { None };

                conn.execute(
                    "INSERT INTO group_info (id, name, description, is_channel, owner_username, invite_token, public_channel_token, avatar_version, created_at)
                     VALUES (1, ?1, '', ?2, 'admin', ?3, ?4, 1, datetime('now'))",
                    rusqlite::params![name, channel, invite_token, public_token],
                ).unwrap();

                // Add admin as first member with owner role
                conn.execute(
                    "INSERT OR IGNORE INTO members (username, display_name, public_key, role, joined_at)
                     VALUES ('admin', 'Admin', '', 'owner', datetime('now'))",
                    [],
                ).unwrap();

                println!("Created {} '{}'", if channel { "channel" } else { "group" }, name);
                println!("Invite token: {}", invite_token);

                if let Some(token) = public_token {
                    println!("\nPublic channel token:");
                    println!("{}", token);
                }
            }
        }

        GroupAction::Invite => {
            match conn.query_row(
                "SELECT invite_token FROM group_info WHERE id = 1",
                [],
                |r| r.get::<_, String>(0),
            ) {
                Ok(token) => println!("Invite token: {}", token),
                Err(_) => println!("Group not initialized."),
            }
        }

        GroupAction::PublicToken => {
            match conn.query_row(
                "SELECT is_channel, public_channel_token FROM group_info WHERE id = 1",
                [],
                |r| Ok((r.get::<_, bool>(0)?, r.get::<_, Option<String>>(1)?)),
            ) {
                Ok((is_channel, token)) => {
                    if !is_channel {
                        println!("Error: Public tokens only work for channels.");
                        println!("Convert this group to a channel first:");
                        println!("  onyx-server group setup \"Channel Name\" --channel");
                        return;
                    }

                    let public_token = if let Some(existing) = token {
                        existing
                    } else {
                        // Generate new token
                        let new_token = uuid::Uuid::new_v4().to_string();
                        conn.execute(
                            "UPDATE group_info SET public_channel_token = ?1 WHERE id = 1",
                            [&new_token],
                        ).unwrap();
                        new_token
                    };

                    println!("Public channel token:");
                    println!("{}", public_token);
                }
                Err(_) => println!("Group not initialized. Run 'onyx-server init' first."),
            }
        }

        GroupAction::ToChannel => {
            match conn.query_row(
                "SELECT is_channel, public_channel_token FROM group_info WHERE id = 1",
                [],
                |r| Ok((r.get::<_, bool>(0)?, r.get::<_, Option<String>>(1)?)),
            ) {
                Ok((is_channel, public_token)) => {
                    if is_channel {
                        println!("Already a channel.");
                    } else {
                        // Generate public token if not exists
                        let token = public_token.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

                        conn.execute(
                            "UPDATE group_info SET is_channel = 1, public_channel_token = ?1 WHERE id = 1",
                            [&token],
                        ).unwrap();

                        println!("Converted to channel. Only owner and moderators can now post messages.");
                        println!("\nPublic channel token:");
                        println!("{}", token);
                    }
                }
                Err(_) => println!("Group not initialized. Run 'onyx-server init' first."),
            }
        }

        GroupAction::ToGroup => {
            match conn.query_row(
                "SELECT is_channel FROM group_info WHERE id = 1",
                [],
                |r| r.get::<_, bool>(0),
            ) {
                Ok(is_channel) => {
                    if !is_channel {
                        println!("Already a group.");
                    } else {
                        conn.execute(
                            "UPDATE group_info SET is_channel = 0 WHERE id = 1",
                            [],
                        ).unwrap();
                        println!("Converted to group. All members can now post messages.");
                    }
                }
                Err(_) => println!("Group not initialized. Run 'onyx-server init' first."),
            }
        }
    }

    print_footer();
}

fn cmd_member(config_path: &str, action: MemberAction) {
    let config = load_config_or_exit(config_path);
    let database = open_db_or_exit(&config.database.path);
    let conn = database.lock().unwrap();

    match action {
        MemberAction::List => {
            let mut stmt = conn.prepare(
                "SELECT username, display_name, role, joined_at FROM members ORDER BY joined_at"
            ).unwrap();

            let rows: Vec<_> = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            }).unwrap().filter_map(|r| r.ok()).collect();

            if rows.is_empty() {
                println!("No members in group.");
            } else {
                println!("{:<20} {:<25} {:<12} {}", "Username", "Display Name", "Role", "Joined");
                println!("{}", "-".repeat(75));
                for (user, display, role, joined) in &rows {
                    println!("{:<20} {:<25} {:<12} {}", user, display, role, joined);
                }
            }
        }

        MemberAction::Add { username_parts, display_name } => {
            let username = username_parts.join(" ");
            if username.is_empty() {
                println!("Error: Username cannot be empty");
            } else {
                let display = display_name.unwrap_or_else(|| username.clone());
                let result = conn.execute(
                    "INSERT INTO members (username, display_name, public_key, role, joined_at)
                     VALUES (?1, ?2, '', 'member', datetime('now'))",
                    rusqlite::params![username, display],
                );
                match result {
                    Ok(_) => println!("Added '{}' to group", username),
                    Err(e) => println!("Error: {} (user may already be a member)", e),
                }
            }
        }

        MemberAction::Kick { username_parts } => {
            let username = username_parts.join(" ");
            if username.is_empty() {
                println!("Error: Username cannot be empty");
            } else {
                let deleted = conn.execute(
                    "DELETE FROM members WHERE username = ?1",
                    [&username],
                ).unwrap();
                if deleted > 0 {
                    println!("Kicked '{}' from group", username);
                } else {
                    println!("'{}' is not a member of the group", username);
                }
            }
        }

        MemberAction::Ban { username_parts, reason } => {
            let username = username_parts.join(" ");
            if username.is_empty() {
                println!("Error: Username cannot be empty");
            } else {
                conn.execute(
                    "DELETE FROM members WHERE username = ?1",
                    [&username],
                ).unwrap();
                conn.execute(
                    "INSERT OR REPLACE INTO bans (username, banned_by, reason, banned_at)
                     VALUES (?1, 'admin', ?2, datetime('now'))",
                    rusqlite::params![username, reason],
                ).unwrap();
                println!("Banned '{}' from group", username);
            }
        }

        MemberAction::Unban { username_parts } => {
            let username = username_parts.join(" ");
            if username.is_empty() {
                println!("Error: Username cannot be empty");
            } else {
                let deleted = conn.execute(
                    "DELETE FROM bans WHERE username = ?1",
                    [&username],
                ).unwrap();
                if deleted > 0 {
                    println!("Unbanned '{}'", username);
                } else {
                    println!("'{}' was not banned in this group", username);
                }
            }
        }

        MemberAction::Role { role, username_parts } => {
            if !["owner", "moderator", "member"].contains(&role.as_str()) {
                println!("Invalid role '{}'. Must be: owner, moderator, member", role);
            } else {
                // Join all username parts with spaces to support usernames with spaces
                let username = username_parts.join(" ");

                if username.is_empty() {
                    println!("Error: Username cannot be empty");
                } else {
                    let updated = conn.execute(
                        "UPDATE members SET role = ?1 WHERE username = ?2",
                        rusqlite::params![role, username],
                    ).unwrap();
                    if updated > 0 {
                        println!("Set '{}' role to '{}'", username, role);
                    } else {
                        println!("'{}' is not a member of the group", username);
                    }
                }
            }
        }
    }

    print_footer();
}

fn cmd_info(_config_path: &str) {
    use std::path::Path;

    // Scan .onyx/data/ directory for all database files
    let data_dir = Path::new(".onyx/data");

    if !data_dir.exists() {
        println!("No servers found. Create a server first:");
        println!("  onyx-server server create <name>");
        return;
    }

    let entries = match std::fs::read_dir(data_dir) {
        Ok(entries) => entries,
        Err(_) => {
            println!("No servers found.");
            return;
        }
    };

    let mut servers = Vec::new();

    // Collect info from all databases
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("db") {
            if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                if let Ok(database) = db::init(path.to_str().unwrap()) {
                    let conn = database.lock().unwrap();

                    let group_info: Option<(String, String, bool, u16)> = conn.query_row(
                        "SELECT name, description, is_channel, 1 FROM group_info WHERE id = 1",
                        [],
                        |r| {
                            Ok((
                                r.get::<_, String>(0)?,
                                r.get::<_, String>(1).unwrap_or_default(),
                                r.get::<_, bool>(2)?,
                                0u16, // placeholder for port
                            ))
                        },
                    ).ok();

                    let members: i64 = conn.query_row(
                        "SELECT COUNT(*) FROM members",
                        [],
                        |r| r.get(0),
                    ).unwrap_or(0);

                    let messages: i64 = conn.query_row(
                        "SELECT COUNT(*) FROM messages",
                        [],
                        |r| r.get(0),
                    ).unwrap_or(0);

                    // Try to get port from config file
                    let config_path = format!(".onyx/configs/{}.toml", name);
                    let port = if let Ok(cfg) = Config::load(&config_path) {
                        cfg.server.port
                    } else {
                        0
                    };

                    if let Some((group_name, _desc, is_channel, _)) = group_info {
                        servers.push((
                            name.to_string(),
                            group_name,
                            is_channel,
                            port,
                            members,
                            messages,
                        ));
                    }
                }
            }
        }
    }

    if servers.is_empty() {
        println!("No servers found. Create a server first:");
        println!("  onyx-server server create <name>");
        return;
    }

    // Display all servers
    println!("\n╔══════════════════════════════════════════════════════════════════════════╗");
    println!("║                        ALL SERVERS STATISTICS                            ║");
    println!("╚══════════════════════════════════════════════════════════════════════════╝\n");

    for (server_name, group_name, is_channel, port, members, messages) in servers {
        let server_type = if is_channel { "Channel" } else { "Group" };

        println!("📁 Server: {}", server_name);
        println!("   Name:     {}", group_name);
        println!("   Type:     {}", server_type);
        println!("   Port:     {}", if port > 0 { port.to_string() } else { "N/A".to_string() });
        println!("   Members:  {}", members);
        println!("   Messages: {}", messages);
        println!();
    }

    println!("💡 To manage a specific server, use:");
    println!("   onyx-server --server <server-name> <command>");

    print_footer();
}

fn load_config_or_exit(path: &str) -> Config {
    Config::load(path).unwrap_or_else(|e| {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    })
}

fn open_db_or_exit(path: &str) -> db::Db {
    db::init(path).unwrap_or_else(|e| {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    })
}

fn print_footer() {
    println!("\n{:>78}", "© 2026 WARDCORE");
    println!("{:>78}", "beta 0.1");
    println!();
}

fn cmd_guide() {
    println!("\n╔══════════════════════════════════════════════════════════════════════════╗");
    println!("║                      ONYX SERVER - QUICK START                          ║");
    println!("╚══════════════════════════════════════════════════════════════════════════╝\n");

    println!("🚀 THE EASIEST WAY - INTERACTIVE MODE");
    println!("   Just run the command and answer the questions:\n");
    println!("   onyx-server server create\n");
    println!("   The program will guide you and ask:");
    println!("   1. Type (group or channel)");
    println!("   2. Server name");
    println!("   3. Port (or automatic)");
    println!("   4. Group/channel name\n");
    println!("   After that, a launcher (.bat or .sh) will be created");
    println!("   Double-click and your server is running!\n");

    println!("⚡ QUICK MODE (NO QUESTIONS)");
    println!("   If you know what you're doing, specify everything at once:\n");
    println!("   onyx-server server create --name gaming --type group --port 3000\n");

    println!("📋 CREATING MULTIPLE SERVERS");
    println!("   Interactive mode:\n");
    println!("   onyx-server server create  (answer the questions)\n");
    println!("   Quick mode:\n");
    println!("   onyx-server server create --name 1 --type group");
    println!("   onyx-server server create --name 2 --type channel\n");
    println!("   Each gets its own launcher. Run them all at once!\n");

    println!("📊 VIEW ALL SERVERS");
    println!("   See statistics for all created servers:\n");
    println!("   onyx-server info\n");

    println!("📖 MANAGING A SPECIFIC SERVER");
    println!("   Use --server to specify the server:\n");
    println!("   onyx-server --server gaming group info");
    println!("   onyx-server --server gaming member list");
    println!("   onyx-server --server gaming member role owner alice\n");



    print_footer();
}

async fn cmd_server(action: ServerAction) {
    use std::path::PathBuf;
    use std::io::{self, Write};

    match action {
        ServerAction::Create { name, type_, port, group_name, os } => {
            println!("\n╔══════════════════════════════════════════════════════════════════════════╗");
            println!("║                 CREATE NEW SERVER (INTERACTIVE MODE)                    ║");
            println!("╚══════════════════════════════════════════════════════════════════════════╝\n");

            // Question 1: Type (group or channel)
            let is_channel = if let Some(t) = type_ {
                t.to_lowercase() == "channel"
            } else {
                println!("1️⃣  Server type:");
                println!("    1 - Group (all members can send messages)");
                println!("    2 - Channel (only owner can send messages)");
                print!("\n    Your choice (1 or 2): ");
                io::stdout().flush().unwrap();

                let mut input = String::new();
                io::stdin().read_line(&mut input).unwrap();
                let choice = input.trim();

                match choice {
                    "2" => true,
                    _ => false,
                }
            };

            let server_type_name = if is_channel { "Channel" } else { "Group" };
            println!("    ✓ Selected: {}\n", server_type_name);

            // Question 2: Server name
            let server_name = if let Some(n) = name {
                n
            } else {
                println!("2️⃣  Server name:");
                println!("    Used for config files and launcher");
                print!("\n    Enter name: ");
                io::stdout().flush().unwrap();

                let mut input = String::new();
                io::stdin().read_line(&mut input).unwrap();
                let name = input.trim().to_string();

                if name.is_empty() {
                    eprintln!("Error: Server name cannot be empty");
                    return;
                }
                name
            };
            println!("    ✓ Server name: {}\n", server_name);

            // Question 3: Port
            let server_port = if let Some(p) = port {
                p
            } else {
                let auto_port = {
                    let mut p = 3000;
                    if let Ok(entries) = std::fs::read_dir(".onyx/configs") {
                        for _ in entries.filter_map(Result::ok) {
                            p += 1;
                        }
                    }
                    p
                };

                println!("3️⃣  Server port:");
                println!("    Port where the server will run");
                print!("\n    Enter port (Enter for auto-select {}): ", auto_port);
                io::stdout().flush().unwrap();

                let mut input = String::new();
                io::stdin().read_line(&mut input).unwrap();
                let input = input.trim();

                if input.is_empty() {
                    auto_port
                } else {
                    input.parse::<u16>().unwrap_or(auto_port)
                }
            };
            println!("    ✓ Port: {}\n", server_port);

            // Question 4: Group/Channel name
            let group_channel_name = if let Some(gn) = group_name {
                gn
            } else {
                println!("4️⃣  {} name:", if is_channel { "Channel" } else { "Group" });
                println!("    This name will be visible to users");
                print!("\n    Enter name (Enter = '{}'): ", server_name);
                io::stdout().flush().unwrap();

                let mut input = String::new();
                io::stdin().read_line(&mut input).unwrap();
                let name = input.trim().to_string();

                if name.is_empty() {
                    server_name.clone()
                } else {
                    name
                }
            };
            println!("    ✓ {} name: {}\n", if is_channel { "Channel" } else { "Group" }, group_channel_name);

            println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
            println!("⚙️  Creating server...\n");

            let config_path = format!(".onyx/configs/{}.toml", server_name);
            let config_dir = PathBuf::from(".onyx/configs");
            std::fs::create_dir_all(&config_dir).ok();

            // Initialize config if it doesn't exist
            if !std::path::Path::new(&config_path).exists() {
                // Create config file
                std::fs::write(&config_path, Config::default_toml())
                    .expect("Failed to write config file");

                // Initialize database and group
                let mut config = Config::load(&config_path).unwrap();
                config.server.port = server_port;
                config.server.name = server_name.clone();
                config.database.path = format!(".onyx/data/{}.db", server_name);

                // Save updated config
                std::fs::write(&config_path, toml::to_string_pretty(&config).unwrap()).ok();

                // Create data directory
                std::fs::create_dir_all(format!(".onyx/data")).ok();

                // Initialize database
                let database = db::init(&config.database.path).unwrap();
                let conn = database.lock().unwrap();
                let invite_token = uuid::Uuid::new_v4().to_string();

                conn.execute(
                    "INSERT OR REPLACE INTO group_info (id, name, description, is_channel, owner_username, invite_token, avatar_version, created_at)
                     VALUES (1, ?1, '', ?2, 'admin', ?3, 1, datetime('now'))",
                    rusqlite::params![group_channel_name, is_channel, invite_token],
                ).ok();

                println!("✓ Configuration created: {}", config_path);
                println!("✓ Database initialized");
                println!("✓ Port: {}", server_port);
            } else {
                println!("Server '{}' already exists, creating launcher...", server_name);
            }

            // Get current executable path
            let exe_path = std::env::current_exe()
                .expect("Failed to get executable path");
            let exe_dir = exe_path.parent().unwrap();

            // Determine target OS
            let target_os = if os == "auto" {
                if cfg!(windows) { "windows" } else { "linux" }
            } else {
                os.as_str()
            };

            // Create launcher file based on target OS
            match target_os {
                "windows" => {
                    let launcher_path = exe_dir.join(format!("{}.bat", server_name));
                    let launcher_content = format!(
                        r#"@echo off
title ONYX Server - {}
cd /d "%~dp0"
echo Starting ONYX server: {}
echo Port: {}
echo.
echo Press Ctrl+C to stop the server
echo.
onyx-server.exe --server {} serve
pause
"#,
                        server_name, server_name, server_port, server_name
                    );

                    std::fs::write(&launcher_path, launcher_content)
                        .expect("Failed to create launcher file");

                    println!("\n╔══════════════════════════════════════════════════════════════════════════╗");
                    println!("║                         ✅ SUCCESSFULLY CREATED!                         ║");
                    println!("╚══════════════════════════════════════════════════════════════════════════╝\n");
                    println!("🎉 Created Windows launcher: {}", launcher_path.display());
                    println!("\n📝 How to use:");
                    println!("   1. Double-click on '{}.bat' to start the server", server_name);
                    println!("   2. The server will run in its own window");
                    println!("   3. Close the window to stop the server");
                    println!("\n💡 You can create as many servers as you want!");

                    print_footer();
                }

                "linux" => {
                    let launcher_path = exe_dir.join(format!("{}.sh", server_name));
                    let launcher_content = format!(
                        r#"#!/bin/bash
cd "$(dirname "$0")"
echo "Starting ONYX server: {}"
echo "Port: {}"
echo ""
echo "Press Ctrl+C to stop the server"
echo ""
./onyx-server --server {} serve
"#,
                        server_name, server_port, server_name
                    );

                    std::fs::write(&launcher_path, launcher_content)
                        .expect("Failed to create launcher file");

                    // Make executable on Unix systems
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let mut perms = std::fs::metadata(&launcher_path)
                            .expect("Failed to get metadata")
                            .permissions();
                        perms.set_mode(0o755);
                        std::fs::set_permissions(&launcher_path, perms)
                            .expect("Failed to set permissions");
                    }

                    println!("\n╔══════════════════════════════════════════════════════════════════════════╗");
                    println!("║                         ✅ SUCCESSFULLY CREATED!                         ║");
                    println!("╚══════════════════════════════════════════════════════════════════════════╝\n");
                    println!("🎉 Created Linux launcher: {}", launcher_path.display());
                    println!("\n📝 How to use:");
                    println!("   1. Run './{}.sh' to start the server", server_name);
                    println!("   2. Press Ctrl+C to stop the server");
                    println!("\n💡 You can create as many servers as you want!");

                    print_footer();
                }

                _ => {
                    eprintln!("Error: Unknown OS '{}'. Use 'windows', 'linux', or 'auto'", os);
                    return;
                }
            }
        }

    }
}

// ========== INTERACTIVE MENU FUNCTIONS ==========

async fn interactive_create_server() {
    clear_screen();
    print_header("CREATE NEW SERVER");

    println!("{}", "Server type:".bright_white().bold());
    println!();
    print_option(1, "Group (all members can send messages)");
    print_option(2, "Channel (only owner can send messages)");
    println!();

    let is_channel = get_choice(2) == 2;
    print_success(&format!("Selected: {}", if is_channel { "Channel" } else { "Group" }));

    println!();
    let server_name = get_input(&"Enter server name:".to_string());

    if server_name.is_empty() {
        print_error("Server name cannot be empty");
        wait_for_enter();
        return;
    }

    print_success(&format!("Server name: {}", server_name));

    println!();
    println!("{}", "Port configuration:".bright_white().bold());
    println!();
    print_option(1, "Auto-assign port");
    print_option(2, "Enter custom port");
    println!();

    let server_port = if get_choice(2) == 1 {
        let mut p = 3000;
        if let Ok(entries) = std::fs::read_dir(".onyx/configs") {
            for _ in entries.filter_map(Result::ok) {
                p += 1;
            }
        }
        p
    } else {
        loop {
            let port_str = get_input(&"Enter port number:".to_string());
            if let Ok(port) = port_str.parse::<u16>() {
                break port;
            }
            print_error("Invalid port number. Please try again.");
        }
    };

    print_success(&format!("Port: {}", server_port));

    println!();
    let default_name = server_name.clone();
    println!("{}", format!("Group/Channel name (press Enter for '{}'):", default_name).cyan());
    let group_channel_name = get_input(&"".to_string());
    let group_channel_name = if group_channel_name.is_empty() {
        server_name.clone()
    } else {
        group_channel_name
    };

    print_success(&format!("{} name: {}", if is_channel { "Channel" } else { "Group" }, group_channel_name));

    println!();
    println!("{}", "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_black());
    print_info("Creating server...");
    println!();

    // Create server logic (same as before but without cmd_server wrapper)
    use std::path::PathBuf;

    let config_path = format!(".onyx/configs/{}.toml", server_name);
    let config_dir = PathBuf::from(".onyx/configs");
    std::fs::create_dir_all(&config_dir).ok();

    if !std::path::Path::new(&config_path).exists() {
        std::fs::write(&config_path, Config::default_toml())
            .expect("Failed to write config file");

        let mut config = Config::load(&config_path).unwrap();
        config.server.port = server_port;
        config.server.name = server_name.clone();
        config.database.path = format!(".onyx/data/{}.db", server_name);

        std::fs::write(&config_path, toml::to_string_pretty(&config).unwrap()).ok();
        std::fs::create_dir_all(format!(".onyx/data")).ok();

        let database = db::init(&config.database.path).unwrap();
        let conn = database.lock().unwrap();
        let invite_token = uuid::Uuid::new_v4().to_string();

        conn.execute(
            "INSERT OR REPLACE INTO group_info (id, name, description, is_channel, owner_username, invite_token, avatar_version, created_at)
             VALUES (1, ?1, '', ?2, 'admin', ?3, 1, datetime('now'))",
            rusqlite::params![group_channel_name, is_channel, invite_token],
        ).ok();

        print_success(&format!("Configuration created: {}", config_path));
        print_success("Database initialized");
        print_success(&format!("Port: {}", server_port));

        // Ask for owners (up to 3)
        println!();
        println!("{}", "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_black());
        print_info("Add owners (up to 3)");
        println!();

        let mut owners = Vec::new();
        let mut first_owner: Option<String> = None;

        for i in 1..=3 {
            println!("{}", format!("Owner {} of 3:", i).bright_white().bold());
            let owner_username = get_input(&format!("Enter username (or press Enter to {}): ", if i == 1 { "skip" } else { "finish" }));

            if owner_username.is_empty() {
                if i == 1 {
                    print_info("No owners added. First registered user will become owner.");
                }
                break;
            }

            let display_name = get_input(&format!("Display name for '{}' (press Enter for same): ", owner_username));
            let display = if display_name.is_empty() { owner_username.clone() } else { display_name };

            // Add owner to members table
            conn.execute(
                "INSERT OR IGNORE INTO members (username, display_name, public_key, role, joined_at)
                 VALUES (?1, ?2, '', 'owner', datetime('now'))",
                rusqlite::params![owner_username, display],
            ).ok();

            if i == 1 {
                first_owner = Some(owner_username.clone());
                // Update group_info with first owner
                conn.execute(
                    "UPDATE group_info SET owner_username = ?1 WHERE id = 1",
                    [&owner_username],
                ).ok();
            }

            owners.push(owner_username.clone());
            print_success(&format!("Added {} as owner", owner_username));
            println!();
        }

        if !owners.is_empty() {
            println!();
            print_success(&format!("Total owners added: {}", owners.len()));
        }
    } else {
        print_info(&format!("Server '{}' already exists, creating launcher...", server_name));
    }

    let exe_path = std::env::current_exe().expect("Failed to get executable path");
    let exe_dir = exe_path.parent().unwrap();

    let target_os = if cfg!(windows) { "windows" } else { "linux" };

    match target_os {
        "windows" => {
            let launcher_path = exe_dir.join(format!("{}.bat", server_name));
            let launcher_content = format!(
                r#"@echo off
title ONYX Server - {}
cd /d "%~dp0"
echo Starting ONYX server: {}
echo Port: {}
echo.
echo Press Ctrl+C to stop the server
echo.
onyx-server.exe --server {} serve
pause
"#,
                server_name, server_name, server_port, server_name
            );

            std::fs::write(&launcher_path, launcher_content)
                .expect("Failed to create launcher file");

            println!();
            println!("{}", "╔══════════════════════════════════════════════════════════════════════════╗".bright_green());
            println!("{}", "║                         ✅ SUCCESSFULLY CREATED!                         ║".bright_green());
            println!("{}", "╚══════════════════════════════════════════════════════════════════════════╝".bright_green());
            println!();
            print_success(&format!("Created Windows launcher: {}", launcher_path.display()));
            println!();
            println!("{}", "How to use:".bright_white().bold());
            println!("  {} Double-click on '{}.bat' to start the server", "1.".yellow(), server_name);
            println!("  {} The server will run in its own window", "2.".yellow());
            println!("  {} Close the window to stop the server", "3.".yellow());
        }

        "linux" => {
            let launcher_path = exe_dir.join(format!("{}.sh", server_name));
            let launcher_content = format!(
                r#"#!/bin/bash
cd "$(dirname "$0")"
echo "Starting ONYX server: {}"
echo "Port: {}"
echo ""
echo "Press Ctrl+C to stop the server"
echo ""
./onyx-server --server {} serve
"#,
                server_name, server_port, server_name
            );

            std::fs::write(&launcher_path, launcher_content)
                .expect("Failed to create launcher file");

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&launcher_path)
                    .expect("Failed to get metadata")
                    .permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&launcher_path, perms)
                    .expect("Failed to set permissions");
            }

            println!();
            println!("{}", "╔══════════════════════════════════════════════════════════════════════════╗".bright_green());
            println!("{}", "║                         ✅ SUCCESSFULLY CREATED!                         ║".bright_green());
            println!("{}", "╚══════════════════════════════════════════════════════════════════════════╝".bright_green());
            println!();
            print_success(&format!("Created Linux launcher: {}", launcher_path.display()));
            println!();
            println!("{}", "How to use:".bright_white().bold());
            println!("  {} Run './{}.sh' to start the server", "1.".yellow(), server_name);
            println!("  {} Press Ctrl+C to stop the server", "2.".yellow());
        }

        _ => {}
    }

    print_footer();
    wait_for_enter();
}

async fn interactive_view_all_servers() {
    use std::path::Path;

    clear_screen();
    print_header("ALL SERVERS STATISTICS");

    let data_dir = Path::new(".onyx/data");

    if !data_dir.exists() {
        print_error("No servers found");
        print_info("Create a server first from the main menu");
        print_footer();
        wait_for_enter();
        return;
    }

    let entries = match std::fs::read_dir(data_dir) {
        Ok(entries) => entries,
        Err(_) => {
            print_error("No servers found");
            print_footer();
            wait_for_enter();
            return;
        }
    };

    let mut servers = Vec::new();

    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("db") {
            if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                if let Ok(database) = db::init(path.to_str().unwrap()) {
                    let conn = database.lock().unwrap();

                    let group_info: Option<(String, String, bool, u16)> = conn.query_row(
                        "SELECT name, description, is_channel, 1 FROM group_info WHERE id = 1",
                        [],
                        |r| {
                            Ok((
                                r.get::<_, String>(0)?,
                                r.get::<_, String>(1).unwrap_or_default(),
                                r.get::<_, bool>(2)?,
                                0u16,
                            ))
                        },
                    ).ok();

                    let members: i64 = conn.query_row(
                        "SELECT COUNT(*) FROM members",
                        [],
                        |r| r.get(0),
                    ).unwrap_or(0);

                    let messages: i64 = conn.query_row(
                        "SELECT COUNT(*) FROM messages",
                        [],
                        |r| r.get(0),
                    ).unwrap_or(0);

                    let config_path = format!(".onyx/configs/{}.toml", name);
                    let port = if let Ok(cfg) = Config::load(&config_path) {
                        cfg.server.port
                    } else {
                        0
                    };

                    if let Some((group_name, _desc, is_channel, _)) = group_info {
                        servers.push((
                            name.to_string(),
                            group_name,
                            is_channel,
                            port,
                            members,
                            messages,
                        ));
                    }
                }
            }
        }
    }

    if servers.is_empty() {
        print_error("No servers found");
        print_info("Create a server first from the main menu");
    } else {
        for (server_name, group_name, is_channel, port, members, messages) in servers {
            let server_type = if is_channel { "Channel" } else { "Group" };

            println!("{} {}", "📁 Server:".bright_cyan().bold(), server_name.white().bold());
            println!("   {} {}", "Name:".bright_black(), group_name.white());
            println!("   {} {}", "Type:".bright_black(), server_type.cyan());
            println!("   {} {}", "Port:".bright_black(), if port > 0 { port.to_string() } else { "N/A".to_string() }.yellow());
            println!("   {} {}", "Members:".bright_black(), members.to_string().green());
            println!("   {} {}", "Messages:".bright_black(), messages.to_string().blue());
            println!();
        }

        print_info("To manage a specific server, select 'Manage server' from the main menu");
    }

    print_footer();
    wait_for_enter();
}

async fn interactive_manage_server() {
    // Get list of servers for selection
    use std::path::Path;
    let data_dir = Path::new(".onyx/data");

    let mut servers = Vec::new();

    if data_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(data_dir) {
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("db") {
                    if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                        if let Ok(database) = db::init(path.to_str().unwrap()) {
                            let conn = database.lock().unwrap();
                            let group_info: Option<(String, bool)> = conn.query_row(
                                "SELECT name, is_channel FROM group_info WHERE id = 1",
                                [],
                                |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)),
                            ).ok();

                            let members: i64 = conn.query_row(
                                "SELECT COUNT(*) FROM members",
                                [],
                                |r| r.get(0),
                            ).unwrap_or(0);

                            let messages: i64 = conn.query_row(
                                "SELECT COUNT(*) FROM messages",
                                [],
                                |r| r.get(0),
                            ).unwrap_or(0);

                            let config_path = format!(".onyx/configs/{}.toml", name);
                            let port = if let Ok(cfg) = Config::load(&config_path) {
                                cfg.server.port
                            } else {
                                0
                            };

                            if let Some((group_name, is_channel)) = group_info {
                                servers.push((
                                    name.to_string(),
                                    group_name,
                                    is_channel,
                                    port,
                                    members,
                                    messages,
                                ));
                            }
                        }
                    }
                }
            }
        }
    }

    loop {
        let server_name = match select_server(&servers) {
            Some(name) => name,
            None => return,
        };

        let config_path = format!(".onyx/configs/{}.toml", server_name);

        loop {
            let choice = show_manage_menu().await;

            match choice {
                1 => interactive_group_menu(&config_path).await,
                2 => interactive_member_menu(&config_path).await,
                3 => return,
                _ => {}
            }
        }
    }
}

async fn interactive_group_menu(config_path: &str) {
    loop {
        let choice = show_group_menu().await;

        match choice {
            1 => {
                // View group info
                let config = load_config_or_exit(config_path);
                let database = open_db_or_exit(&config.database.path);
                let conn = database.lock().unwrap();

                clear_screen();
                print_header("GROUP INFORMATION");

                match conn.query_row(
                    "SELECT id, name, description, is_channel, owner_username, invite_token, avatar_version, created_at,
                            max_message_length, media_provider, max_file_size, allowed_file_types, rate_limit
                     FROM group_info WHERE id = 1",
                    [],
                    |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, bool>(3)?,
                            r.get::<_, String>(4)?,
                            r.get::<_, String>(5)?,
                            r.get::<_, i64>(6)?,
                            r.get::<_, String>(7)?,
                            r.get::<_, i64>(8).unwrap_or(5000),
                            r.get::<_, String>(9).unwrap_or_else(|_| "local".to_string()),
                            r.get::<_, i64>(10).unwrap_or(10485760),
                            r.get::<_, String>(11).unwrap_or_else(|_| "image/*,video/*,audio/*,application/pdf".to_string()),
                            r.get::<_, i64>(12).unwrap_or(0),
                        ))
                    },
                ) {
                    Ok((id, name, desc, is_channel, owner, token, avatar_ver, created, max_msg_len, media_prov, max_file, allowed_types, rate_lim)) => {
                        let member_count: i64 = conn.query_row(
                            "SELECT COUNT(*) FROM members",
                            [],
                            |r| r.get(0),
                        ).unwrap_or(0);

                        // Get all owners
                        let mut stmt = conn.prepare("SELECT username FROM members WHERE role = 'owner'").unwrap();
                        let owners: Vec<String> = stmt.query_map([], |r| r.get(0))
                            .unwrap()
                            .filter_map(|r| r.ok())
                            .collect();

                        let gtype = if is_channel { "Channel" } else { "Group" };
                        println!("{} {}", "Group ID:".bright_black(), id.to_string().white());
                        println!("{} {}", "Name:".bright_black(), name.cyan().bold());
                        println!("{} {}", "Type:".bright_black(), gtype.yellow());
                        println!("{} {}", "Description:".bright_black(), if desc.is_empty() { "(none)".bright_black().to_string() } else { desc.white().to_string() });

                        if owners.is_empty() {
                            println!("{} {}", "Main Owner:".bright_black(), owner.green());
                        } else {
                            println!("{} {}", "Owners:".bright_black(), owners.join(", ").green());
                        }

                        println!("{} {}", "Members:".bright_black(), member_count.to_string().blue());
                        println!("{} {}", "Avatar version:".bright_black(), avatar_ver.to_string().white());
                        println!("{} {}", "Created:".bright_black(), created.white());
                        println!("{} {}", "Invite token:".bright_black(), token.yellow());

                        println!();
                        println!("{}", "Settings:".bright_white().bold());
                        println!("{} {}", "  Max message length:".bright_black(), if max_msg_len == 0 { "No limit".white().to_string() } else { format!("{} characters", max_msg_len).white().to_string() });
                        println!("{} {}", "  Media provider:".bright_black(), media_prov.white());
                        let file_mb = max_file as f64 / 1024.0 / 1024.0;
                        println!("{} {}", "  Max file size:".bright_black(), if max_file == 0 { "No limit".white().to_string() } else { format!("{:.2} MB", file_mb).white().to_string() });
                        println!("{} {}", "  Allowed file types:".bright_black(), allowed_types.white());
                        println!("{} {}", "  Rate limit:".bright_black(), if rate_lim == 0 { "No limit".white().to_string() } else { format!("{} msg/min", rate_lim).white().to_string() });
                    }
                    Err(_) => print_error("Group not initialized"),
                }

                print_footer();
                wait_for_enter();
            }
            2 => {
                // Setup/rename group
                let config = load_config_or_exit(config_path);
                let database = open_db_or_exit(&config.database.path);
                let conn = database.lock().unwrap();

                clear_screen();
                print_header("SETUP/RENAME GROUP");

                let name = get_input("Enter new group/channel name:");
                if name.is_empty() {
                    print_error("Name cannot be empty");
                    wait_for_enter();
                    continue;
                }

                print_info("Is this a channel? (only owner can post)");
                println!("  {} Group (all members can post)", "1.".yellow());
                println!("  {} Channel (only owner can post)", "2.".yellow());
                let is_channel = get_choice(2) == 2;

                let exists: bool = conn.query_row(
                    "SELECT COUNT(*) FROM group_info WHERE id = 1",
                    [],
                    |r| Ok(r.get::<_, i64>(0)? > 0),
                ).unwrap_or(false);

                if exists {
                    if is_channel {
                        let public_token: Option<String> = conn.query_row(
                            "SELECT public_channel_token FROM group_info WHERE id = 1",
                            [],
                            |r| r.get(0),
                        ).ok().flatten();

                        let token = public_token.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

                        conn.execute(
                            "UPDATE group_info SET name = ?1, is_channel = ?2, public_channel_token = ?3 WHERE id = 1",
                            rusqlite::params![name, is_channel, token],
                        ).unwrap();

                        println!();
                        print_success(&format!("Updated channel '{}'", name));
                        println!();
                        print_info("Public channel token:");
                        println!("{}", token.yellow());
                    } else {
                        conn.execute(
                            "UPDATE group_info SET name = ?1, is_channel = ?2 WHERE id = 1",
                            rusqlite::params![name, is_channel],
                        ).unwrap();
                        println!();
                        print_success(&format!("Updated group '{}'", name));
                    }
                } else {
                    let invite_token = uuid::Uuid::new_v4().to_string();
                    let public_token = if is_channel { Some(uuid::Uuid::new_v4().to_string()) } else { None };

                    conn.execute(
                        "INSERT INTO group_info (id, name, description, is_channel, owner_username, invite_token, public_channel_token, avatar_version, created_at)
                         VALUES (1, ?1, '', ?2, 'admin', ?3, ?4, 1, datetime('now'))",
                        rusqlite::params![name, is_channel, invite_token, public_token],
                    ).unwrap();

                    println!();
                    print_success(&format!("Created {} '{}'", if is_channel { "channel" } else { "group" }, name));
                    println!();
                    print_info("Invite token:");
                    println!("{}", invite_token.yellow());

                    if let Some(token) = public_token {
                        println!();
                        print_info("Public channel token:");
                        println!("{}", token.yellow());
                    }

                    // Ask for owners (up to 3)
                    println!();
                    println!("{}", "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_black());
                    print_info("Add owners (up to 3)");
                    println!();

                    let mut owners = Vec::new();
                    let mut first_owner: Option<String> = None;

                    for i in 1..=3 {
                        println!("{}", format!("Owner {} of 3:", i).bright_white().bold());
                        let owner_username = get_input(&format!("Enter username (or press Enter to {}): ", if i == 1 { "skip" } else { "finish" }));

                        if owner_username.is_empty() {
                            if i == 1 {
                                print_info("No owners added. First registered user will become owner.");
                            }
                            break;
                        }

                        let display_name = get_input(&format!("Display name for '{}' (press Enter for same): ", owner_username));
                        let display = if display_name.is_empty() { owner_username.clone() } else { display_name };

                        // Add owner to members table
                        conn.execute(
                            "INSERT OR IGNORE INTO members (username, display_name, public_key, role, joined_at)
                             VALUES (?1, ?2, '', 'owner', datetime('now'))",
                            rusqlite::params![owner_username, display],
                        ).ok();

                        if i == 1 {
                            first_owner = Some(owner_username.clone());
                            // Update group_info with first owner
                            conn.execute(
                                "UPDATE group_info SET owner_username = ?1 WHERE id = 1",
                                [&owner_username],
                            ).ok();
                        }

                        owners.push(owner_username.clone());
                        print_success(&format!("Added {} as owner", owner_username));
                        println!();
                    }

                    if !owners.is_empty() {
                        println!();
                        print_success(&format!("Total owners added: {}", owners.len()));
                    }
                }

                print_footer();
                wait_for_enter();
            }
            3 => {
                // Show invite token
                let config = load_config_or_exit(config_path);
                let database = open_db_or_exit(&config.database.path);
                let conn = database.lock().unwrap();

                clear_screen();
                print_header("INVITE TOKEN");

                match conn.query_row(
                    "SELECT invite_token FROM group_info WHERE id = 1",
                    [],
                    |r| r.get::<_, String>(0),
                ) {
                    Ok(token) => {
                        print_info("Share this token with users to invite them:");
                        println!();
                        println!("{}", token.yellow().bold());
                    }
                    Err(_) => print_error("Group not initialized"),
                }

                print_footer();
                wait_for_enter();
            }
            4 => {
                // Show public token (channels only)
                let config = load_config_or_exit(config_path);
                let database = open_db_or_exit(&config.database.path);
                let conn = database.lock().unwrap();

                clear_screen();
                print_header("PUBLIC CHANNEL TOKEN");

                match conn.query_row(
                    "SELECT is_channel, public_channel_token FROM group_info WHERE id = 1",
                    [],
                    |r| Ok((r.get::<_, bool>(0)?, r.get::<_, Option<String>>(1)?)),
                ) {
                    Ok((is_channel, token)) => {
                        if !is_channel {
                            print_error("Public tokens only work for channels");
                            println!();
                            print_info("Convert this group to a channel first");
                        } else {
                            let public_token = if let Some(existing) = token {
                                existing
                            } else {
                                let new_token = uuid::Uuid::new_v4().to_string();
                                conn.execute(
                                    "UPDATE group_info SET public_channel_token = ?1 WHERE id = 1",
                                    [&new_token],
                                ).unwrap();
                                new_token
                            };

                            print_info("Share this token for public access:");
                            println!();
                            println!("{}", public_token.yellow().bold());
                        }
                    }
                    Err(_) => print_error("Group not initialized"),
                }

                print_footer();
                wait_for_enter();
            }
            5 => {
                // Convert to channel
                let config = load_config_or_exit(config_path);
                let database = open_db_or_exit(&config.database.path);
                let conn = database.lock().unwrap();

                clear_screen();
                print_header("CONVERT TO CHANNEL");

                match conn.query_row(
                    "SELECT is_channel, public_channel_token FROM group_info WHERE id = 1",
                    [],
                    |r| Ok((r.get::<_, bool>(0)?, r.get::<_, Option<String>>(1)?)),
                ) {
                    Ok((is_channel, public_token)) => {
                        if is_channel {
                            print_error("Already a channel");
                        } else {
                            let token = public_token.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

                            conn.execute(
                                "UPDATE group_info SET is_channel = 1, public_channel_token = ?1 WHERE id = 1",
                                [&token],
                            ).unwrap();

                            print_success("Converted to channel");
                            print_info("Only owner and moderators can now post messages");
                            println!();
                            print_info("Public channel token:");
                            println!("{}", token.yellow());
                        }
                    }
                    Err(_) => print_error("Group not initialized"),
                }

                print_footer();
                wait_for_enter();
            }
            6 => {
                // Convert to group
                let config = load_config_or_exit(config_path);
                let database = open_db_or_exit(&config.database.path);
                let conn = database.lock().unwrap();

                clear_screen();
                print_header("CONVERT TO GROUP");

                match conn.query_row(
                    "SELECT is_channel FROM group_info WHERE id = 1",
                    [],
                    |r| r.get::<_, bool>(0),
                ) {
                    Ok(is_channel) => {
                        if !is_channel {
                            print_error("Already a group");
                        } else {
                            conn.execute(
                                "UPDATE group_info SET is_channel = 0 WHERE id = 1",
                                [],
                            ).unwrap();
                            print_success("Converted to group");
                            print_info("All members can now post messages");
                        }
                    }
                    Err(_) => print_error("Group not initialized"),
                }

                print_footer();
                wait_for_enter();
            }
            7 => {
                // Change description
                let mut config = load_config_or_exit(config_path);
                let database = open_db_or_exit(&config.database.path);
                let conn = database.lock().unwrap();

                clear_screen();
                print_header("CHANGE DESCRIPTION");

                let description = get_input("Enter new description (or press Enter to clear):");

                conn.execute(
                    "UPDATE group_info SET description = ?1 WHERE id = 1",
                    [&description],
                ).unwrap();

                config.update_description(description.clone());
                if let Err(e) = config.save(config_path) {
                    print_error(&format!("Failed to save config: {}", e));
                }

                if description.is_empty() {
                    print_success("Description cleared and saved to config");
                } else {
                    print_success("Description updated and saved to config");
                }

                print_footer();
                wait_for_enter();
            }
            8 => {
                // Set max message length
                let mut config = load_config_or_exit(config_path);
                let database = open_db_or_exit(&config.database.path);
                let conn = database.lock().unwrap();

                clear_screen();
                print_header("SET MAX MESSAGE LENGTH");

                let current: i64 = conn.query_row(
                    "SELECT max_message_length FROM group_info WHERE id = 1",
                    [],
                    |r| r.get(0),
                ).unwrap_or(5000);

                print_info(&format!("Current max message length: {} characters", current));
                println!();

                let input = get_input("Enter new max message length (0 = no limit):");
                if let Ok(length) = input.parse::<i64>() {
                    conn.execute(
                        "UPDATE group_info SET max_message_length = ?1 WHERE id = 1",
                        [length],
                    ).unwrap();

                    config.update_max_message_length(length as u32);
                    if let Err(e) = config.save(config_path) {
                        print_error(&format!("Failed to save config: {}", e));
                    }

                    print_success(&format!("Max message length set to {} and saved to config", length));
                } else {
                    print_error("Invalid number");
                }

                print_footer();
                wait_for_enter();
            }
            9 => {
                // Set media provider
                let mut config = load_config_or_exit(config_path);
                let database = open_db_or_exit(&config.database.path);
                let conn = database.lock().unwrap();

                clear_screen();
                print_header("SET MEDIA PROVIDER");

                let current: String = conn.query_row(
                    "SELECT media_provider FROM group_info WHERE id = 1",
                    [],
                    |r| r.get(0),
                ).unwrap_or_else(|_| "local".to_string());

                print_info(&format!("Current media provider: {}", current));
                println!();

                let provider = get_input("Enter media provider (local/s3/custom):");
                if !provider.is_empty() {
                    conn.execute(
                        "UPDATE group_info SET media_provider = ?1 WHERE id = 1",
                        [&provider],
                    ).unwrap();

                    config.update_media_provider(provider.clone());
                    if let Err(e) = config.save(config_path) {
                        print_error(&format!("Failed to save config: {}", e));
                    }

                    print_success(&format!("Media provider set to '{}' and saved to config", provider));
                } else {
                    print_error("Provider cannot be empty");
                }

                print_footer();
                wait_for_enter();
            }
            10 => {
                // Set max file size
                let mut config = load_config_or_exit(config_path);
                let database = open_db_or_exit(&config.database.path);
                let conn = database.lock().unwrap();

                clear_screen();
                print_header("SET MAX FILE SIZE");

                let current: i64 = conn.query_row(
                    "SELECT max_file_size FROM group_info WHERE id = 1",
                    [],
                    |r| r.get(0),
                ).unwrap_or(10485760);

                let current_mb = current as f64 / 1024.0 / 1024.0;
                print_info(&format!("Current max file size: {:.2} MB ({} bytes)", current_mb, current));
                println!();

                let input = get_input("Enter new max file size in MB (0 = no limit):");
                if let Ok(mb) = input.parse::<f64>() {
                    let bytes = (mb * 1024.0 * 1024.0) as i64;
                    conn.execute(
                        "UPDATE group_info SET max_file_size = ?1 WHERE id = 1",
                        [bytes],
                    ).unwrap();

                    config.update_max_file_size(mb as u32);
                    if let Err(e) = config.save(config_path) {
                        print_error(&format!("Failed to save config: {}", e));
                    }

                    print_success(&format!("Max file size set to {:.2} MB and saved to config", mb));
                } else {
                    print_error("Invalid number");
                }

                print_footer();
                wait_for_enter();
            }
            11 => {
                // Set allowed file types
                let mut config = load_config_or_exit(config_path);
                let database = open_db_or_exit(&config.database.path);
                let conn = database.lock().unwrap();

                clear_screen();
                print_header("SET ALLOWED FILE TYPES");

                let current: String = conn.query_row(
                    "SELECT allowed_file_types FROM group_info WHERE id = 1",
                    [],
                    |r| r.get(0),
                ).unwrap_or_else(|_| "image/*,video/*,audio/*,application/pdf".to_string());

                print_info("Current allowed file types:");
                println!("{}", current.white());
                println!();
                print_info("Enter MIME types separated by commas");
                print_info("Examples: image/*,video/*,audio/*,application/pdf");
                println!();

                let types = get_input("Enter allowed file types (or press Enter to keep current):");
                if !types.is_empty() {
                    conn.execute(
                        "UPDATE group_info SET allowed_file_types = ?1 WHERE id = 1",
                        [&types],
                    ).unwrap();

                    let types_vec: Vec<String> = types.split(',').map(|s| s.trim().to_string()).collect();
                    config.update_allowed_file_types(types_vec);
                    if let Err(e) = config.save(config_path) {
                        print_error(&format!("Failed to save config: {}", e));
                    }

                    print_success("Allowed file types updated and saved to config");
                } else {
                    print_info("No changes made");
                }

                print_footer();
                wait_for_enter();
            }
            12 => {
                // Set rate limit
                let mut config = load_config_or_exit(config_path);
                let database = open_db_or_exit(&config.database.path);
                let conn = database.lock().unwrap();

                clear_screen();
                print_header("SET RATE LIMIT");

                let current: i64 = conn.query_row(
                    "SELECT rate_limit FROM group_info WHERE id = 1",
                    [],
                    |r| r.get(0),
                ).unwrap_or(0);

                print_info(&format!("Current rate limit: {} messages per minute{}", current, if current == 0 { " (no limit)" } else { "" }));
                println!();

                let input = get_input("Enter max messages per minute (0 = no limit):");
                if let Ok(limit) = input.parse::<i64>() {
                    conn.execute(
                        "UPDATE group_info SET rate_limit = ?1 WHERE id = 1",
                        [limit],
                    ).unwrap();

                    config.update_rate_limit(limit as u32);
                    if let Err(e) = config.save(config_path) {
                        print_error(&format!("Failed to save config: {}", e));
                    }

                    if limit == 0 {
                        print_success("Rate limit removed and saved to config");
                    } else {
                        print_success(&format!("Rate limit set to {} messages per minute and saved to config", limit));
                    }
                } else {
                    print_error("Invalid number");
                }

                print_footer();
                wait_for_enter();
            }
            13 => break,
            _ => {}
        }
    }
}

async fn interactive_member_menu(config_path: &str) {
    loop {
        let choice = show_member_menu().await;

        match choice {
            1 => {
                // List members
                let config = load_config_or_exit(config_path);
                let database = open_db_or_exit(&config.database.path);
                let conn = database.lock().unwrap();

                clear_screen();
                print_header("MEMBERS LIST");

                let mut stmt = conn.prepare(
                    "SELECT username, display_name, role, joined_at FROM members ORDER BY joined_at"
                ).unwrap();

                let rows: Vec<_> = stmt.query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                }).unwrap().filter_map(|r| r.ok()).collect();

                if rows.is_empty() {
                    print_error("No members in group");
                } else {
                    println!("{:<20} {:<25} {:<12} {}",
                        "Username".bright_cyan().bold(),
                        "Display Name".bright_cyan().bold(),
                        "Role".bright_cyan().bold(),
                        "Joined".bright_cyan().bold()
                    );
                    println!("{}", "-".repeat(75).bright_black());
                    for (user, display, role, joined) in &rows {
                        let role_color = match role.as_str() {
                            "owner" => role.red().bold(),
                            "moderator" => role.yellow().bold(),
                            _ => role.white(),
                        };
                        println!("{:<20} {:<25} {:<12} {}",
                            user.white(),
                            display.bright_white(),
                            role_color.to_string(),
                            joined.bright_black()
                        );
                    }
                }

                print_footer();
                wait_for_enter();
            }
            2 => {
                // Add member
                let config = load_config_or_exit(config_path);
                let database = open_db_or_exit(&config.database.path);
                let conn = database.lock().unwrap();

                clear_screen();
                print_header("ADD MEMBER");

                let username = get_input("Enter username:");
                if username.is_empty() {
                    print_error("Username cannot be empty");
                    wait_for_enter();
                    continue;
                }

                let display_name = get_input("Enter display name (press Enter for same as username):");
                let display = if display_name.is_empty() { username.clone() } else { display_name };

                let result = conn.execute(
                    "INSERT INTO members (username, display_name, public_key, role, joined_at)
                     VALUES (?1, ?2, '', 'member', datetime('now'))",
                    rusqlite::params![username, display],
                );

                println!();
                match result {
                    Ok(_) => print_success(&format!("Added '{}' to group", username)),
                    Err(e) => print_error(&format!("Error: {} (user may already be a member)", e)),
                }

                print_footer();
                wait_for_enter();
            }
            3 => {
                // Kick member
                let config = load_config_or_exit(config_path);
                let database = open_db_or_exit(&config.database.path);
                let conn = database.lock().unwrap();

                clear_screen();
                print_header("KICK MEMBER");

                let username = get_input("Enter username to kick:");
                if username.is_empty() {
                    print_error("Username cannot be empty");
                    wait_for_enter();
                    continue;
                }

                let deleted = conn.execute(
                    "DELETE FROM members WHERE username = ?1",
                    [&username],
                ).unwrap();

                println!();
                if deleted > 0 {
                    print_success(&format!("Kicked '{}' from group", username));
                } else {
                    print_error(&format!("'{}' is not a member of the group", username));
                }

                print_footer();
                wait_for_enter();
            }
            4 => {
                // Ban member
                let config = load_config_or_exit(config_path);
                let database = open_db_or_exit(&config.database.path);
                let conn = database.lock().unwrap();

                clear_screen();
                print_header("BAN MEMBER");

                let username = get_input("Enter username to ban:");
                if username.is_empty() {
                    print_error("Username cannot be empty");
                    wait_for_enter();
                    continue;
                }

                let reason = get_input("Enter ban reason (optional):");
                let reason_opt = if reason.is_empty() { None } else { Some(reason) };

                conn.execute(
                    "DELETE FROM members WHERE username = ?1",
                    [&username],
                ).unwrap();
                conn.execute(
                    "INSERT OR REPLACE INTO bans (username, banned_by, reason, banned_at)
                     VALUES (?1, 'admin', ?2, datetime('now'))",
                    rusqlite::params![username, reason_opt],
                ).unwrap();

                println!();
                print_success(&format!("Banned '{}' from group", username));

                print_footer();
                wait_for_enter();
            }
            5 => {
                // Unban member
                let config = load_config_or_exit(config_path);
                let database = open_db_or_exit(&config.database.path);
                let conn = database.lock().unwrap();

                clear_screen();
                print_header("UNBAN MEMBER");

                let username = get_input("Enter username to unban:");
                if username.is_empty() {
                    print_error("Username cannot be empty");
                    wait_for_enter();
                    continue;
                }

                let deleted = conn.execute(
                    "DELETE FROM bans WHERE username = ?1",
                    [&username],
                ).unwrap();

                println!();
                if deleted > 0 {
                    print_success(&format!("Unbanned '{}'", username));
                } else {
                    print_error(&format!("'{}' was not banned in this group", username));
                }

                print_footer();
                wait_for_enter();
            }
            6 => {
                // Change member role
                let config = load_config_or_exit(config_path);
                let database = open_db_or_exit(&config.database.path);
                let conn = database.lock().unwrap();

                clear_screen();
                print_header("CHANGE MEMBER ROLE");

                let username = get_input("Enter username:");
                if username.is_empty() {
                    print_error("Username cannot be empty");
                    wait_for_enter();
                    continue;
                }

                println!();
                print_info("Select role:");
                print_option(1, "Owner");
                print_option(2, "Moderator");
                print_option(3, "Member");
                println!();

                let role_choice = get_choice(3);
                let role = match role_choice {
                    1 => "owner",
                    2 => "moderator",
                    3 => "member",
                    _ => "member",
                };

                let updated = conn.execute(
                    "UPDATE members SET role = ?1 WHERE username = ?2",
                    rusqlite::params![role, username],
                ).unwrap();

                println!();
                if updated > 0 {
                    print_success(&format!("Set '{}' role to '{}'", username, role));
                } else {
                    print_error(&format!("'{}' is not a member of the group", username));
                }

                print_footer();
                wait_for_enter();
            }
            7 => break,
            _ => {}
        }
    }
}

async fn interactive_guide() {
    clear_screen();
    print_header("ONYX SERVER - QUICK START GUIDE");

    println!("{}", "🚀 THE EASIEST WAY - INTERACTIVE MODE".bright_cyan().bold());
    println!();
    print_info("Just run the program and follow the menus!");
    println!();

    println!("{}", "📋 CREATING A SERVER".bright_white().bold());
    println!("  {} Select 'Create new server' from the main menu", "1.".yellow());
    println!("  {} Choose server type (Group or Channel)", "2.".yellow());
    println!("  {} Enter server name", "3.".yellow());
    println!("  {} Configure port (auto or custom)", "4.".yellow());
    println!("  {} Enter group/channel name", "5.".yellow());
    println!();

    println!("{}", "🎮 STARTING A SERVER".bright_white().bold());
    println!("  {} Find the launcher file ({} or {})", "1.".yellow(), "name.bat".cyan(), "name.sh".cyan());
    println!("  {} Double-click to start", "2.".yellow());
    println!("  {} Close window to stop", "3.".yellow());
    println!();

    println!("{}", "📊 MANAGING SERVERS".bright_white().bold());
    println!("  {} Select 'Manage server' from the main menu", "1.".yellow());
    println!("  {} Choose which server to manage", "2.".yellow());
    println!("  {} Select what you want to do", "3.".yellow());
    println!();

    print_footer();
    wait_for_enter();
}
