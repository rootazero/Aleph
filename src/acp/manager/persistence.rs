//! Disk persistence for ACP sessions.
//!
//! Format: a JSON array of [`crate::acp::session::PersistedAcpSession`] entries
//! at `~/.aleph/data/acp_sessions.json`. Best-effort — parse failures fall back
//! to "no persisted sessions" and write failures only warn.

use tracing::{info, warn};

use super::AcpAdapterManager;
use crate::sync_primitives::Arc;

/// Default persistence file path for ACP sessions.
fn acp_sessions_path() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".aleph")
        .join("data")
        .join("acp_sessions.json")
}

/// Load persisted ACP sessions from disk (best-effort).
#[must_use]
pub fn load_persisted_sessions() -> Vec<crate::acp::session::PersistedAcpSession> {
    let path = acp_sessions_path();
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
            warn!("Failed to parse ACP sessions file: {}", e);
            Vec::new()
        }),
        Err(_) => Vec::new(),
    }
}

/// Save persisted ACP sessions to disk (atomic write).
pub fn save_persisted_sessions(sessions: &[crate::acp::session::PersistedAcpSession]) {
    let path = acp_sessions_path();
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            warn!("Failed to create ACP sessions directory: {}", e);
        }
    }
    match serde_json::to_string_pretty(sessions) {
        Ok(json) => {
            if let Err(e) = crate::utils::atomic_io::write_atomic(&path, json.as_bytes()) {
                warn!("Failed to atomic-write ACP sessions: {}", e);
            }
        }
        Err(e) => warn!("Failed to serialize ACP sessions: {}", e),
    }
}

/// Wire up file-based persistence for ACP sessions.
/// Call this after creating the `AcpAdapterManager` at startup.
pub async fn wire_persistence(manager: &AcpAdapterManager) {
    use crate::sync_primitives::Mutex;

    let sessions = Arc::new(Mutex::new(
        tokio::task::spawn_blocking(load_persisted_sessions)
            .await
            .unwrap_or_else(|e| {
                warn!("ACP session load task failed: {}", e);
                Vec::new()
            }),
    ));

    let sessions_ref = Arc::clone(&sessions);
    manager
        .set_persistence_hook(Arc::new(move |event: crate::acp::AcpSessionEvent| {
            let store_clone = {
                let mut store = sessions_ref.lock().unwrap_or_else(|e| e.into_inner());
                match event {
                    crate::acp::AcpSessionEvent::Created {
                        ref harness_id,
                        ref acp_session_id,
                        ref cwd,
                        ref session_name,
                    } => {
                        // Match the full triple so a `backend` name doesn't
                        // displace the unnamed/default entry under the same cwd.
                        // `Created` is re-emitted idempotently after every
                        // prompt, so preserve the original `created_at` instead
                        // of resetting it each turn (only `last_used_at` moves).
                        let prior_created = store
                            .iter()
                            .find(|s| {
                                s.harness_id == *harness_id
                                    && s.cwd == *cwd
                                    && s.session_name == *session_name
                            })
                            .map(|s| s.created_at);
                        store.retain(|s| {
                            !(s.harness_id == *harness_id
                                && s.cwd == *cwd
                                && s.session_name == *session_name)
                        });
                        let now = chrono::Utc::now();
                        store.push(crate::acp::session::PersistedAcpSession {
                            harness_id: harness_id.clone(),
                            acp_session_id: acp_session_id.clone(),
                            cwd: cwd.clone(),
                            created_at: prior_created.unwrap_or(now),
                            last_used_at: now,
                            session_name: session_name.clone(),
                        });
                    }
                    crate::acp::AcpSessionEvent::Removed {
                        ref harness_id,
                        ref cwd,
                        ref session_name,
                    } => {
                        store.retain(|s| {
                            !(s.harness_id == *harness_id
                                && s.cwd == *cwd
                                && s.session_name == *session_name)
                        });
                    }
                }
                store.clone()
            };
            tokio::spawn(async move {
                if let Err(e) = tokio::task::spawn_blocking(move || {
                    save_persisted_sessions(&store_clone);
                })
                .await
                {
                    tracing::warn!(?e, "persist_sessions: spawn_blocking failed");
                }
            });
        }))
        .await;

    // Restore existing sessions
    let persisted = sessions.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if !persisted.is_empty() {
        let restored = manager.restore_sessions(persisted).await;
        if !restored.is_empty() {
            info!(count = restored.len(), "Restored ACP sessions from disk");
        }
    }
}

#[cfg(test)]
pub(super) fn acp_sessions_path_for_test() -> std::path::PathBuf {
    acp_sessions_path()
}
