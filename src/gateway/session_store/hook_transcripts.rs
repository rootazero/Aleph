//! The live session store, as the hook executor's transcript source.
//!
//! `extension::hooks` declares [`TranscriptSource`] and reads it; this is its
//! one implementation, published around every run
//! (`execution_engine/execute.rs`, the one `run_agent_loop` call). Asking the
//! LIVE store rather than re-deriving the default file-backend location is what
//! lets the SQLite backend answer honestly (`None`) instead of naming a stale
//! `transcript.jsonl` an earlier file-backend install left on disk.

use std::path::PathBuf;

use crate::extension::hooks::TranscriptSource;
use crate::gateway::router::SessionKey;
use crate::gateway::session_store::SessionStore;
use crate::sync_primitives::Arc;

/// [`SessionStore::transcript_file`], keyed by the hook's `session_id` string.
pub struct StoreTranscripts {
    store: Arc<dyn SessionStore>,
}

impl StoreTranscripts {
    #[must_use]
    pub fn new(store: Arc<dyn SessionStore>) -> Self {
        Self { store }
    }
}

impl TranscriptSource for StoreTranscripts {
    /// A `session_id` that is not a session key (a synthetic or legacy id) has
    /// no row in any store, so it has no transcript.
    fn transcript_path(&self, session_id: &str) -> Option<PathBuf> {
        let key = SessionKey::from_key_string(session_id)?;
        self.store.transcript_file(&key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use crate::gateway::session_store::types::MessageRecord;
    use tempfile::TempDir;

    fn line(content: &str) -> MessageRecord {
        MessageRecord {
            id: uuid::Uuid::new_v4().to_string(),
            role: "user".into(),
            content: content.into(),
            timestamp: chrono::Utc::now().timestamp(),
            metadata: None,
            input_tokens: 0,
            output_tokens: 0,
            tool_call_id: None,
            tool_name: None,
        }
    }

    async fn written(store: &Arc<dyn SessionStore>, key: &SessionKey) {
        store.get_or_create(key).await.unwrap();
        store.append_message(key, line("hello")).await.unwrap();
    }

    /// The whole chain a production hook rides: the store writes its
    /// transcript, the run publishes the store, a command hook fired inside
    /// reads `transcript_path` from its stdin and finds that very file.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_hook_fired_under_the_published_store_is_handed_the_file_it_wrote() {
        use crate::extension::hooks::{with_transcript_source, HookContext, HookExecutor};
        use crate::extension::{HookAction, HookConfig, HookEvent, HookKind};

        let data = TempDir::new().unwrap();
        let store: Arc<dyn SessionStore> = Arc::new(
            FileSessionStore::new(FileSessionStoreConfig {
                base_dir: data.path().to_path_buf(),
                ..FileSessionStoreConfig::default()
            })
            .unwrap(),
        );
        let key = SessionKey::main("hooks");
        written(&store, &key).await;

        let out = data.path().join("stdin.json");
        let executor = HookExecutor::new(vec![HookConfig {
            event: HookEvent::SessionStart,
            kind: HookKind::Observer,
            priority: Default::default(),
            matcher: None,
            actions: vec![HookAction::Command {
                command: format!("cat > '{}'", out.display()),
            }],
            plugin_name: "test".into(),
            plugin_root: data.path().to_path_buf(),
            handler: None,
            timeout_secs: None,
            declared_event: None,
            scope_key: crate::extension::visibility::ScopeKey::Global,
        }]);
        let ctx = HookContext::new(key.to_key_string());
        with_transcript_source(
            Arc::new(StoreTranscripts::new(Arc::clone(&store))),
            executor.execute_observers(HookEvent::SessionStart, &ctx),
        )
        .await;

        let seen: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&out).expect("the hook ran")).unwrap();
        let want = store.transcript_file(&key).expect("the store wrote a file");
        assert_eq!(seen["transcript_path"], want.to_string_lossy().as_ref());
    }

    /// SQLite keeps rows, not a file: after real writes it still answers
    /// `None` — through the trait and through the hook-facing adapter.
    #[tokio::test]
    async fn the_sqlite_store_names_no_transcript_even_after_writing_one() {
        let temp = TempDir::new().unwrap();
        let store: Arc<dyn SessionStore> = Arc::new(
            SessionManager::new(SessionManagerConfig {
                db_path: temp.path().join("sessions.db"),
                ..Default::default()
            })
            .unwrap(),
        );
        let key = SessionKey::main("hooks");
        written(&store, &key).await;
        assert_eq!(store.transcript_file(&key), None);
        assert_eq!(
            StoreTranscripts::new(store).transcript_path(&key.to_key_string()),
            None
        );
    }

    /// The publish site. There is no engine-level test that drives a run with
    /// hooks (hooks come from the process-global extension manager), so the
    /// wiring is pinned by shape: the ONE `run_agent_loop(` call in
    /// `execute.rs` sits inside the argument list of `with_transcript_source(`.
    /// Deleting the wrap leaves every executor test green — this is the red.
    #[test]
    fn every_run_is_published_with_the_live_store_as_its_transcript_source() {
        use crate::utils::source_scan::{code_text, production_text};
        let rel = "src/gateway/execution_engine/execute.rs";
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
        let code = code_text(&production_text(
            std::path::Path::new(rel),
            &std::fs::read_to_string(&path).unwrap(),
        ));
        let calls: Vec<usize> = code
            .match_indices(".run_agent_loop(")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(calls.len(), 1, "{rel}: one run_agent_loop call");
        let wrap = code
            .get(..calls[0])
            .and_then(|head| head.rfind("with_transcript_source("))
            .expect("run_agent_loop is published under with_transcript_source(");
        let open = wrap + "with_transcript_source".len();
        let mut depth = 0usize;
        let close = code
            .get(open..)
            .and_then(|s| {
                s.char_indices().find_map(|(i, c)| {
                    match c {
                        '(' => depth += 1,
                        ')' => depth -= 1,
                        _ => {}
                    }
                    (depth == 0).then_some(open + i)
                })
            })
            .expect("balanced parentheses");
        assert!(
            calls[0] < close,
            "{rel}: run_agent_loop must be INSIDE with_transcript_source(..), not after it"
        );
        let args = code.get(open..close).unwrap();
        assert!(
            args.contains("StoreTranscripts::new("),
            "{rel}: the published source is the live store"
        );
    }
}
