use reqwest::multipart;
use super::UploadResult;
use crate::config::MediaCustomConfig;

pub async fn upload(
    config: &MediaCustomConfig,
    filename: &str,
    data: Vec<u8>,
    _mime_type: &str,
) -> Result<UploadResult, String> {
    if config.upload_url.is_empty() {
        return Err("Custom media provider upload_url not configured".into());
    }

    let part = multipart::Part::bytes(data)
        .file_name(filename.to_string());

    let form = multipart::Form::new()
        .part(config.upload_field_name.clone(), part);

    let mut builder = reqwest::Client::new()
        .post(&config.upload_url)
        .multipart(form);

    for (key, value) in &config.extra_headers {
        builder = builder.header(key.as_str(), value.as_str());
    }

    let resp = builder.send().await
        .map_err(|e| format!("Custom upload failed: {}", e))?;

    let json: serde_json::Value = resp.json().await
        .map_err(|e| format!("Failed to parse custom response: {}", e))?;

    let url = extract_jsonpath(&json, &config.response_url_jsonpath)
        .ok_or_else(|| format!("Could not extract URL from response using '{}': {:?}",
            config.response_url_jsonpath, json))?;

    Ok(UploadResult {
        url,
        provider: "custom".to_string(),
    })
}

fn extract_jsonpath(value: &serde_json::Value, path: &str) -> Option<String> {
    let path = path.strip_prefix("$.").unwrap_or(path);
    let mut current = value;

    for key in path.split('.') {
        current = current.get(key)?;
    }

    current.as_str().map(|s| s.to_string())
}
