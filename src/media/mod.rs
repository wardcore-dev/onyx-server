pub mod local;
pub mod catbox;
pub mod pixeldrain;
pub mod fileio;
pub mod anonfiles;
pub mod custom;

use crate::config::MediaConfig;

pub struct UploadResult {
    pub url: String,
    pub provider: String,
}

pub async fn upload_to_provider(
    config: &MediaConfig,
    filename: &str,
    data: Vec<u8>,
    mime_type: &str,
) -> Result<UploadResult, String> {
    match config.provider.as_str() {
        "local" => Err("Local uploads handled separately via handlers::media".into()),
        "catbox" => catbox::upload(filename, data).await,
        "pixeldrain" => pixeldrain::upload(filename, data).await,
        "fileio" => fileio::upload(filename, data).await,
        "anonfiles" => anonfiles::upload(filename, data).await,
        "custom" => custom::upload(&config.custom, filename, data, mime_type).await,
        other => Err(format!("Unknown media provider: {}", other)),
    }
}
