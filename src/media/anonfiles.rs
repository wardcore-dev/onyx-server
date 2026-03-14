use reqwest::multipart;
use super::UploadResult;

pub async fn upload(filename: &str, data: Vec<u8>) -> Result<UploadResult, String> {
    let part = multipart::Part::bytes(data)
        .file_name(filename.to_string());

    let form = multipart::Form::new()
        .part("file", part);

    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.anonfiles.com/upload")
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("Anonfiles upload failed: {}", e))?;

    let json: serde_json::Value = resp.json().await
        .map_err(|e| format!("Failed to parse anonfiles response: {}", e))?;

    if json.get("status").and_then(|v| v.as_bool()).unwrap_or(false) {
        if let Some(url) = json.pointer("/data/file/url/full").and_then(|v| v.as_str()) {
            return Ok(UploadResult {
                url: url.to_string(),
                provider: "anonfiles".to_string(),
            });
        }
    }

    Err(format!("Anonfiles upload failed: {:?}", json))
}
