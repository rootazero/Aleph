//! Sync Buffer Persistence
//!
//! Persists the get_updates_buf for incremental sync across restarts.

use std::path::Path;

const SYNC_FILE_SUFFIX: &str = ".sync.json";

fn sync_path(hermes_home: &str, account_id: &str) -> std::path::PathBuf {
    let base = Path::new(hermes_home).join("wechat").join("accounts");
    base.join(format!("{}{}", account_id, SYNC_FILE_SUFFIX))
}

/// Load sync buffer for an account.
pub async fn load_sync_buf(hermes_home: &str, account_id: &str) -> String {
    let path = sync_path(hermes_home, account_id);
    if !path.exists() {
        return String::new();
    }
    match tokio::fs::read_to_string(&path).await {
        Ok(content) => {
            if let Ok(data) = serde_json::from_str::<serde_json::Value>(&content) {
                data.get("get_updates_buf")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            } else {
                String::new()
            }
        }
        Err(_) => String::new(),
    }
}

/// Save sync buffer for an account.
pub async fn save_sync_buf(hermes_home: &str, account_id: &str, sync_buf: &str) {
    let path = sync_path(hermes_home, account_id);
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let data = serde_json::json!({ "get_updates_buf": sync_buf });
    let tmp_path = path.with_extension("tmp");
    if let Ok(json) = serde_json::to_string_pretty(&data) {
        let _ = tokio::fs::write(&tmp_path, json).await;
        let _ = tokio::fs::rename(&tmp_path, &path).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_save_load_sync_buf() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().to_str().unwrap();

        save_sync_buf(path, "account1", "sync_buf_abc").await;
        let loaded = load_sync_buf(path, "account1").await;
        assert_eq!(loaded, "sync_buf_abc");
    }

    #[tokio::test]
    async fn test_load_nonexistent() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().to_str().unwrap();
        let loaded = load_sync_buf(path, "nonexistent").await;
        assert!(loaded.is_empty());
    }
}