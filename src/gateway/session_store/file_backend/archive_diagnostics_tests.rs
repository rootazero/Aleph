use super::{FileSessionStore, FileSessionStoreConfig, MessageRecord, SessionKey, SessionStore};
use tokio::io::AsyncWriteExt;

// Diagnostic experiment, run before changing production. No writer is left
// intentionally open between phases; all moves use the real store API.
#[tokio::test]
async fn measure_windows_handle_attribution() {
    use std::os::windows::fs::OpenOptionsExt;
    use std::time::Instant;

    let dir = tempfile::tempdir().unwrap();
    let store = FileSessionStore::new(FileSessionStoreConfig {
        base_dir: dir.path().to_path_buf(),
        ..Default::default()
    })
    .unwrap();
    let message: MessageRecord = serde_json::from_value(serde_json::json!({
        "id":"measurement", "role":"user", "content":"transcript bytes",
        "timestamp":0, "metadata":null, "input_tokens":0, "output_tokens":0,
        "tool_call_id":null, "tool_name":null
    }))
    .unwrap();
    for mode in [
        "no_write",
        "production_append",
        "production_append_handle_probe",
        "explicit_close",
    ] {
        let keys = (0..100)
            .map(|index| SessionKey::main(format!("{mode}-{index}")))
            .collect::<Vec<_>>();
        for key in &keys {
            store.get_or_create(key).await.unwrap();
        }
        let mut failures = Vec::new();
        let mut exclusive_open_failures = 0;
        for key in &keys {
            let text = key.to_key_string();
            if mode.starts_with("production_append") {
                store.append_transcript(&text, &message).await.unwrap();
            }
            if mode == "production_append_handle_probe" {
                // An exclusive reopen is evidence that no conflicting handle
                // survives append's return at this observed instant.
                match std::fs::OpenOptions::new()
                    .read(true)
                    .share_mode(0)
                    .open(store.transcript_path(&text))
                {
                    Ok(file) => drop(file),
                    Err(_) => exclusive_open_failures += 1,
                }
            }
            if mode == "explicit_close" {
                let mut file = tokio::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(store.transcript_path(&text))
                    .await
                    .unwrap();
                let row = format!("{}\n", serde_json::to_string(&message).unwrap());
                file.write_all(row.as_bytes()).await.unwrap();
                file.flush().await.unwrap();
                drop(file.into_std().await);
            }
            let started = Instant::now();
            if let Err(error) = store.delete_session(key).await {
                let mut recovery = None;
                for attempt in 1..=5 {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    if store.delete_session(key).await.is_ok() {
                        recovery = Some((attempt, started.elapsed().as_millis()));
                        break;
                    }
                }
                failures.push((text, error.to_string(), recovery));
            }
        }
        println!("ATTRIBUTION mode={mode} rounds=100 exclusive_open_failures={exclusive_open_failures} first_failures={} details={failures:?}", failures.len());
    }
}
