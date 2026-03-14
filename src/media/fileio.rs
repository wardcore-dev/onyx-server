use reqwest::multipart;
use super::UploadResult;

pub async fn upload(filename: &str, data: Vec<u8>) -> Result<UploadResult, String> {
    let part = multipart::Part::bytes(data)
        .file_name(filename.to_string());

    let form = multipart::Form::new()
        .part("file", part);

    let client = reqwest::Client::new();
    let resp = client
        .post("https://file.io")
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("File.io upload failed: {}", e))?;

    let json: serde_json::Value = resp.json().await
        .map_err(|e| format!("Failed to parse file.io response: {}", e))?;

    if json.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
        if let Some(link) = json.get("link").and_then(|v| v.as_str()) {
            return Ok(UploadResult {
                url: link.to_string(),
                provider: "fileio".to_string(),
            });
        }
    }

    Err(format!("File.io upload failed: {:?}", json))
}
