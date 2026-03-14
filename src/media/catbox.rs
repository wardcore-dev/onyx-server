use reqwest::multipart;
use super::UploadResult;

pub async fn upload(filename: &str, data: Vec<u8>) -> Result<UploadResult, String> {
    let part = multipart::Part::bytes(data)
        .file_name(filename.to_string());

    let form = multipart::Form::new()
        .text("reqtype", "fileupload")
        .part("fileToUpload", part);

    let client = reqwest::Client::new();
    let resp = client
        .post("https://catbox.moe/user/api.php")
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("Catbox upload failed: {}", e))?;

    let text = resp.text().await
        .map_err(|e| format!("Failed to read catbox response: {}", e))?;

    if text.starts_with("http") {
        Ok(UploadResult {
            url: text.trim().to_string(),
            provider: "catbox".to_string(),
        })
    } else {
        Err(format!("Catbox upload failed: {}", text))
    }
}
