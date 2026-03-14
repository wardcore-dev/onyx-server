use axum::{
    extract::DefaultBodyLimit,
    middleware,
    routing::{get, post, delete, patch},
    Router,
};
use std::net::IpAddr;
use tower_http::cors::CorsLayer;
use tracing::info;

use crate::auth::{self, NonceStore};
use crate::config::Config;
use crate::db::Db;
use crate::handlers::{auth_handlers, avatars, groups, info, media, members, messages};
use crate::ws::connection::{ws_upgrade, ws_public_upgrade};
use crate::ws::hub::Hub;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub config: Config,
    pub hub: Hub,
    pub nonces: NonceStore,
    pub group_public_key: String,
}

pub async fn start(config: Config, db: Db) -> Result<(), String> {
    let hub = Hub::new();
    let nonces = auth::new_nonce_store();

    let (group_public_key, _secret_key) = auth::generate_group_keypair();

    let addr = format!("{}:{}", config.server.bind_address, config.server.port);
    let name = config.server.name.clone();

    let port = config.server.port;
    let state = AppState { db, config, hub, nonces, group_public_key };

    let app = build_router(state);

    info!("Starting ONYX group server '{}' on {}", name, addr);

    let listener = tokio::net::TcpListener::bind(&addr).await
        .map_err(|e| format!("Failed to bind to {}: {}", addr, e))?;

    print_available_addresses(port).await;

    axum::serve(listener, app).await
        .map_err(|e| format!("Server error: {}", e))
}

async fn print_available_addresses(port: u16) {
    println!();
    println!("  Available addresses to connect:");
    println!("  ─────────────────────────────────");

    // Localhost
    println!("  Local:      127.0.0.1:{}", port);

    // Get all network interface addresses
    #[cfg(target_os = "windows")]
    {
        if let Ok(hostname) = hostname::get() {
            if let Ok(addrs) = std::net::ToSocketAddrs::to_socket_addrs(
                &(hostname.to_string_lossy().to_string(), port)
            ) {
                for addr in addrs {
                    match addr.ip() {
                        IpAddr::V4(ip) if !ip.is_loopback() => {
                            let label = if ip.is_private() { "LAN" } else { "Public" };
                            println!("  {:<10}  {}:{}", label, ip, port);
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        use std::process::Command;
        if let Ok(output) = Command::new("hostname").arg("-I").output() {
            let ips = String::from_utf8_lossy(&output.stdout);
            for ip_str in ips.split_whitespace() {
                if let Ok(ip) = ip_str.parse::<std::net::Ipv4Addr>() {
                    if !ip.is_loopback() {
                        let label = if ip.is_private() { "LAN" } else { "Public" };
                        println!("  {:<10}  {}:{}", label, ip, port);
                    }
                }
            }
        }
    }

    // Fetch public IP from external service (useful for VPS)
    match reqwest::Client::new()
        .get("https://api.ipify.org")
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
    {
        Ok(resp) => {
            if let Ok(public_ip) = resp.text().await {
                let public_ip = public_ip.trim();
                if !public_ip.is_empty() {
                    println!("  {:<10}  {}:{}", "Public", public_ip, port);
                }
            }
        }
        Err(_) => {
            println!("  Public:     (could not detect — no internet?)");
        }
    }

    println!("  ─────────────────────────────────");
    println!();
}

fn build_router(state: AppState) -> Router {
    // Public routes (no auth required)
    let public_routes = Router::new()
        .route("/info", get(info::get_info))
        .route("/group", get(info::get_group))
        .route("/channels/:public_token", get(info::get_public_channel))
        .route("/channels/:public_token/history", get(messages::get_public_channel_history))
        .route("/auth/register", post(auth_handlers::register))
        .route("/auth/challenge", post(auth_handlers::challenge))
        .route("/auth/verify", post(auth_handlers::verify))
        .route("/avatar", get(avatars::get_avatar))
        .route("/groups/:id/avatar", get(groups::get_group_avatar))
        .route("/ws", get(ws_upgrade))
        .route("/ws/public/:public_token", get(ws_public_upgrade));

    // Authenticated routes
    let auth_routes = Router::new()
        // Group listing (client expects GET /groups returning array)
        .route("/groups", get(info::get_groups))
        // Group-prefixed routes (client uses these)
        .route("/groups/{group_id}/history", get(messages::get_group_history))
        .route("/groups/{group_id}/send", post(messages::send_group_message))
        .route("/groups/join/{invite_token}", post(messages::join_group))
        .route("/groups/{group_id}/leave", post(messages::leave_group))
        .route("/groups/{group_id}/messages/{message_id}", delete(messages::delete_message))
        .route("/groups/{group_id}/messages/{message_id}", patch(messages::edit_message))
        // User info
        .route("/my-role", get(messages::get_my_role))
        // Legacy routes (kept for compatibility)
        .route("/members", get(members::list_members))
        .route("/bans", get(members::list_bans))
        .route("/ban-status", get(members::check_ban_status))
        .route("/history", get(messages::get_history))
        .route("/send", post(messages::send_message))
        .route("/upload_avatar", post(avatars::upload_avatar))
        .route("/delete_avatar", delete(avatars::delete_avatar))
        .route("/data/media/upload", post(media::upload_media))  // Upload requires auth
        .route("/data/media/:filename", get(media::download_media))  // Download requires auth + membership check
        .route("/members/:username/kick", post(members::kick_member))
        .route("/members/:username/ban", post(members::ban_member))
        .route("/members/:username/unban", post(members::unban_member))
        .route("/members/:username/role", post(members::set_role))
        // Group management (owner only)
        .route("/groups/:id/rename", post(groups::rename_group))
        .route("/groups/:id/avatar", post(groups::upload_group_avatar))
        .route("/groups/:id/avatar", delete(groups::delete_group_avatar))
        .layer(middleware::from_fn_with_state(state.clone(), auth::auth_middleware));

    Router::new()
        .merge(public_routes)
        .merge(auth_routes)
        .layer(CorsLayer::permissive())
        .layer(DefaultBodyLimit::max(100 * 1024 * 1024))  // 100MB limit for file uploads
        .with_state(state)
}
