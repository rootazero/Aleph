//! Sync Buffer Persistence
//!
//! Persists the `get_updates_buf` for incremental sync across restarts.

use std::path::Path;
const SYNC_FILE_SUFFIX: &str = ".sync.json";

fn sync_path(hermes_home: &str, account_id: &str) -> std::path::PathBuf {
    let base = Path::new(hermes_home).join("wechat").join("accounts");
    base.join(format!("{account_id}{SYNC_FILE_SUFFIX}"))
}

/// Load sync buffer for an account.
///
/// A missing file is not an error (the account simply has no cursor yet);
/// a read failure IS reported — swallowing it would silently restart the
/// sync from the beginning and re-deliver every message.
pub async fn load_sync_buf(hermes_home: &str, account_id: &str) -> Result<String, std::io::Error> {
    let path = sync_path(hermes_home, account_id);
    if !path.exists() {
        return Ok(String::new());
    }
    let content = tokio::fs::read_to_string(&path).await?;
    Ok(serde_json::from_str::<serde_json::Value>(&content)
        .ok()
        .and_then(|data| {
            data.get("get_updates_buf")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default())
}

/// Save sync buffer for an account.
///
/// The error is returned, not logged away: the caller advances the live
/// cursor in memory, so a lost write means the persisted cursor lags behind
/// and the next start re-reads that window. The operator has to see it.
pub async fn save_sync_buf(
    hermes_home: &str,
    account_id: &str,
    sync_buf: &str,
) -> Result<(), std::io::Error> {
    let path = sync_path(hermes_home, account_id);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let data = serde_json::json!({ "get_updates_buf": sync_buf });
    let json = data.to_string();
    let tmp_path = path.with_extension("tmp");
    tokio::fs::write(&tmp_path, json).await?;
    if let Err(e) = tokio::fs::rename(&tmp_path, &path).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_save_load_sync_buf() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().to_str().unwrap();

        save_sync_buf(path, "account1", "sync_buf_abc")
            .await
            .unwrap();
        let loaded = load_sync_buf(path, "account1").await.unwrap();
        assert_eq!(loaded, "sync_buf_abc");
    }

    #[tokio::test]
    async fn test_load_nonexistent() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().to_str().unwrap();
        let loaded = load_sync_buf(path, "nonexistent").await.unwrap();
        assert!(loaded.is_empty());
    }

    #[tokio::test]
    async fn a_failing_persistence_is_reported() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().to_str().unwrap();
        // "wechat/accounts" is a FILE, so the directory creation fails.
        std::fs::create_dir_all(temp_dir.path().join("wechat")).unwrap();
        std::fs::write(
            temp_dir.path().join("wechat").join("accounts"),
            b"not a directory",
        )
        .unwrap();
        assert!(
            save_sync_buf(path, "account1", "buf").await.is_err(),
            "a failed save must be reported, not swallowed"
        );

        // A cursor path that is a directory makes the read fail.
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().to_str().unwrap();
        std::fs::create_dir_all(temp_dir.path().join("wechat/accounts/account1.sync.json"))
            .unwrap();
        assert!(
            load_sync_buf(path, "account1").await.is_err(),
            "a failed load must be reported, not swallowed"
        );
    }
}
