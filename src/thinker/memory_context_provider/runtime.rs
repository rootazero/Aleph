use crate::config::types::memory::MemoryInjectionMode;
use crate::error::AlephError;
use crate::memory::curated::store::{CuratedError, WriteOutcome};
use crate::memory::runtime::MemoryRuntime;
use crate::providers::message::UnifiedMessage;
use crate::sync_primitives::Arc;

#[derive(Debug)]
pub enum CuratedRuntimeError {
    Unavailable,
    Service(AlephError),
}

/// A stable caller-facing reference; concrete services remain bundle-owned.
#[derive(Clone)]
pub struct RuntimeMemoryContext {
    runtime: Arc<MemoryRuntime>,
}

impl RuntimeMemoryContext {
    pub fn new(runtime: Arc<MemoryRuntime>) -> Self {
        Self { runtime }
    }

    pub fn injection_mode(&self) -> MemoryInjectionMode {
        self.runtime.injection_mode()
    }

    pub fn curated_limit(&self) -> usize {
        self.runtime
            .memory_config(|config| config.curated.memory_char_limit)
            .unwrap_or(0)
    }

    pub(crate) fn resolve_storage_id(&self, agent: &str) -> String {
        crate::memory::project_scope::session_write_id(
            agent,
            self.runtime
                .memory_config(|config| config.project_scoped)
                .unwrap_or(false),
            crate::projects::current_project_root().as_deref(),
        )
    }

    pub async fn curated_list(&self, agent: &str) -> Result<WriteOutcome, CuratedRuntimeError> {
        let operation = self
            .runtime
            .operation()
            .map_err(|_| CuratedRuntimeError::Unavailable)?;
        let context = operation
            .services()
            .map_err(CuratedRuntimeError::Service)?
            .context
            .clone();
        let agent = agent.to_owned();
        operation
            .run(async move {
                let store = context.get_or_load_curated_store(&agent).await?;
                Ok(store.snapshot_outcome(String::new()))
            })
            .await
            .map_err(CuratedRuntimeError::Service)
    }

    pub async fn curated_replace(
        &self,
        agent: &str,
        old_text: String,
        content: String,
    ) -> Result<Result<WriteOutcome, CuratedError>, CuratedRuntimeError> {
        self.curated_edit(agent, old_text, Some(content)).await
    }

    pub async fn curated_remove(
        &self,
        agent: &str,
        old_text: String,
    ) -> Result<Result<WriteOutcome, CuratedError>, CuratedRuntimeError> {
        self.curated_edit(agent, old_text, None).await
    }

    async fn curated_edit(
        &self,
        agent: &str,
        old_text: String,
        content: Option<String>,
    ) -> Result<Result<WriteOutcome, CuratedError>, CuratedRuntimeError> {
        let operation = self
            .runtime
            .operation()
            .map_err(|_| CuratedRuntimeError::Unavailable)?;
        let context = operation
            .services()
            .map_err(CuratedRuntimeError::Service)?
            .context
            .clone();
        let agent = agent.to_owned();
        operation
            .run(async move {
                let store = context.get_or_load_curated_store(&agent).await?;
                let outcome = match content {
                    Some(content) => store.replace(&old_text, &content).await,
                    None => store.remove(&old_text).await,
                };
                if outcome.is_ok() {
                    context.invalidate_curated_for_agent(&store.agent_id).await;
                }
                Ok(outcome)
            })
            .await
            .map_err(CuratedRuntimeError::Service)
    }

    pub async fn build_memory_user_message(
        &self,
        agent_id: &str,
        query: &str,
        session_id: Option<&str>,
        available_tokens: Option<u32>,
    ) -> Result<Option<UnifiedMessage>, AlephError> {
        let Ok(operation) = self.runtime.operation() else {
            return Ok(None);
        };
        let context = operation.services()?.context.clone();
        let agent_id = agent_id.to_owned();
        let query = query.to_owned();
        let session_id = session_id.map(str::to_owned);
        operation
            .run(async move {
                context
                    .build_memory_user_message(
                        &agent_id,
                        &query,
                        session_id.as_deref(),
                        available_tokens,
                    )
                    .await
            })
            .await
    }

    pub async fn build_curated_message(
        &self,
        base_agent_id: &str,
        session_key: &str,
    ) -> Result<Option<UnifiedMessage>, AlephError> {
        let Ok(operation) = self.runtime.operation() else {
            return Ok(None);
        };
        let context = operation.services()?.context.clone();
        let agent = base_agent_id.to_owned();
        let session = session_key.to_owned();
        operation
            .run(async move { context.build_curated_message(&agent, &session).await })
            .await
    }

    pub async fn build_orientation_message_cached(
        &self,
        base_agent_id: &str,
        session_key: &str,
        mode: MemoryInjectionMode,
    ) -> Result<Option<UnifiedMessage>, AlephError> {
        let Ok(operation) = self.runtime.operation() else {
            return Ok(None);
        };
        let context = operation.services()?.context.clone();
        let agent = base_agent_id.to_owned();
        let session = session_key.to_owned();
        operation
            .run(async move {
                context
                    .build_orientation_message_cached(&agent, &session, mode)
                    .await
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::ownership::OwnershipTree;
    use crate::memory::runtime::{MemoryRuntimeConfig, MemoryState};
    use crate::memory::store::markdown::MarkdownSessionStore;

    #[tokio::test]
    async fn restart_rebuilds_owned_context_and_discards_session_cache() {
        use crate::config::types::memory::MemoryConfig;
        use crate::memory::extensions::MemoryExtensionRegistry;
        use crate::memory::runtime::MemoryServicesConfig;

        let root = tempfile::tempdir().unwrap();
        let raw = Arc::new(MarkdownSessionStore::new(root.path()).await.unwrap());
        let curated = root.path().join("curated");
        std::fs::create_dir_all(curated.join("main")).unwrap();
        let memory_file = curated.join("main/MEMORY.md");
        std::fs::write(&memory_file, "first bundle fact\n§\n").unwrap();
        let mut memory = MemoryConfig::default();
        memory.injection_mode = MemoryInjectionMode::Context;
        memory.project_scoped = false;
        memory.embedding.providers.clear();
        let runtime = MemoryRuntime::new_with_services(
            MemoryRuntimeConfig {
                metadata_path: root.path().join("metadata.db"),
                note_memory_dir: root.path().join("note"),
            },
            Arc::new(OwnershipTree::new()),
            MemoryServicesConfig {
                app_config: Arc::new(tokio::sync::RwLock::new(crate::Config {
                    memory: memory.clone(),
                    ..Default::default()
                })),
                raw_store: raw,
                session_store: None,
                provider: None,
                extensions: Arc::new(MemoryExtensionRegistry::new()),
                state_db: None,
                default_agent_id: "main".into(),
                curated_root: curated,
                snapshot_dir: root.path().join("snapshots"),
            },
        );
        let context = RuntimeMemoryContext::new(runtime.clone());
        assert_eq!(context.curated_limit(), 0);
        assert!(context
            .build_curated_message("main", "session")
            .await
            .unwrap()
            .is_none());
        assert!(!root.path().join("note").exists());
        runtime.start().await.unwrap();
        assert_eq!(context.injection_mode(), MemoryInjectionMode::Context);
        let first = context
            .build_curated_message("main", "session")
            .await
            .unwrap()
            .unwrap();
        assert!(format!("{first:?}").contains("first bundle fact"));
        std::fs::write(&memory_file, "second bundle fact\n§\n").unwrap();
        let cached = context
            .build_curated_message("main", "session")
            .await
            .unwrap()
            .unwrap();
        assert!(format!("{cached:?}").contains("first bundle fact"));
        runtime.stop().await.unwrap();
        assert!(context
            .build_curated_message("main", "session")
            .await
            .unwrap()
            .is_none());
        runtime.start().await.unwrap();
        let second = context
            .build_curated_message("main", "session")
            .await
            .unwrap()
            .unwrap();
        assert!(format!("{second:?}").contains("second bundle fact"));
        assert!(!format!("{second:?}").contains("first bundle fact"));
        runtime.stop().await.unwrap();
    }

    #[tokio::test]
    async fn off_chat_context_skips_memory_without_opening_bundle() {
        let root = tempfile::tempdir().unwrap();
        let _raw = MarkdownSessionStore::new(root.path()).await.unwrap();
        let runtime = MemoryRuntime::new(
            MemoryRuntimeConfig {
                metadata_path: root.path().join("metadata.db"),
                note_memory_dir: root.path().join("note"),
            },
            Arc::new(OwnershipTree::new()),
        );
        let context = RuntimeMemoryContext::new(runtime.clone());
        let result = context
            .build_memory_user_message("main", "hello", Some("chat"), Some(2000))
            .await;
        assert!(
            result.unwrap().is_none(),
            "optional enrichment must skip unavailable Memory"
        );
        assert_eq!(runtime.status().state, MemoryState::Off);
        assert!(!root.path().join("note").exists());
        let connection = rusqlite::Connection::open(root.path().join("metadata.db")).unwrap();
        let notes: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='notes_index'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            notes, 0,
            "the raw connection must not initialize derived Memory tables"
        );
    }
}
