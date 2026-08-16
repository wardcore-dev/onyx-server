use serde::Deserialize;
use std::time::Duration;

pub const CURRENT_VERSION: &str = "v0.8-beta";
const GITHUB_API: &str = "https://api.github.com/repos/wardcore-dev/onyx-server/releases/latest";

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
}

pub struct UpdateInfo {
    pub tag: String,
    pub page_url: String,
}

pub async fn check() -> Option<UpdateInfo> {
    let client = reqwest::Client::builder()
        .user_agent("onyx-server-updater/1.0")
        .timeout(Duration::from_secs(5))
        .build()
        .ok()?;

    let release: GithubRelease = client
        .get(GITHUB_API)
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    if !is_newer(&release.tag_name, CURRENT_VERSION) {
        return None;
    }

    Some(UpdateInfo {
        tag: release.tag_name,
        page_url: release.html_url,
    })
}

fn is_newer(tag: &str, current: &str) -> bool {
    tag.trim_start_matches('v') != current.trim_start_matches('v')
}
