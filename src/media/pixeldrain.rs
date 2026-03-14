use reqwest::multipart;
use super::UploadResult;

pub async fn upload(filename: &str, data: Vec<u8>) -> Result<UploadResult, String> {
    let part = multipart::Part::bytes(data)
        .file_name(filename.to_string());

    let form = multipart::Form::new()
        .part("file", part);

    let client = reqwest::Client::new();
    let resp = client
        .post("https://pixeldrain.com/api/file")
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("Pixeldrain upload failed: {}", e))?;

    let json: serde_json::Value = resp.json().await
        .map_err(|e| format!("Failed to parse pixeldrain response: {}", e))?;

    if let Some(id) = json.get("id").and_then(|v| v.as_str()) {
        Ok(UploadResult {
            url: format!("https://pixeldrain.com/api/file/{}", id),
            provider: "pixeldrain".to_string(),
        })
    } else {
        Err(format!("Pixeldrain upload failed: {:?}", json))
    }
}
