use std::path::PathBuf;
use tokio::fs;

pub async fn save_file(
    storage_path: &str,
    media_id: &str,
    original_filename: &str,
    data: &[u8],
) -> Result<String, String> {
    let ext = PathBuf::from(original_filename)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();

    let filename = format!("{}{}", media_id, ext);
    let full_path = PathBuf::from(storage_path).join(&filename);

    fs::write(&full_path, data).await
        .map_err(|e| format!("Failed to write file: {}", e))?;

    Ok(filename)
}

pub async fn read_file(storage_path: &str, filename: &str) -> Result<Vec<u8>, String> {
    let full_path = PathBuf::from(storage_path).join(filename);
    fs::read(&full_path).await
        .map_err(|e| format!("Failed to read file: {}", e))
}
