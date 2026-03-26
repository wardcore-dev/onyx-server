use std::path::PathBuf;
use tokio::fs;

fn validate_filename(filename: &str) -> Result<(), String> {
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return Err("Invalid filename".into());
    }
    Ok(())
}

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

    validate_filename(&filename)?;

    let base = std::fs::canonicalize(storage_path)
        .map_err(|e| format!("Storage path error: {}", e))?;
    let full_path = base.join(&filename);

    // Ensure the resolved path stays within the storage directory
    if !full_path.starts_with(&base) {
        return Err("Access denied".into());
    }

    fs::write(&full_path, data).await
        .map_err(|e| format!("Failed to write file: {}", e))?;

    Ok(filename)
}

pub async fn read_file(storage_path: &str, filename: &str) -> Result<Vec<u8>, String> {
    validate_filename(filename)?;

    let base = std::fs::canonicalize(storage_path)
        .map_err(|e| format!("Storage path error: {}", e))?;
    let full_path = base.join(filename);
    let canonical = std::fs::canonicalize(&full_path)
        .map_err(|_| "File not found".to_string())?;

    if !canonical.starts_with(&base) {
        return Err("Access denied".into());
    }

    fs::read(&canonical).await
        .map_err(|e| format!("Failed to read file: {}", e))
}
