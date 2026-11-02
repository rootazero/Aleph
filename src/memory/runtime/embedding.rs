//! Semantic embedding entry for consumers that must not retain a provider.

use crate::error::AlephError;
use crate::sync_primitives::Arc;

use super::MemoryRuntime;

/// Vectors and the model that produced them belong to the same admission.
pub struct MemoryEmbeddingBatch {
    pub model: String,
    pub embeddings: Vec<Vec<f32>>,
}

impl MemoryRuntime {
    pub async fn embed_batch(
        self: &Arc<Self>,
        texts: Vec<String>,
    ) -> Result<Option<MemoryEmbeddingBatch>, AlephError> {
        let operation = match self.operation() {
            Ok(operation) => operation,
            Err(_) => return Ok(None),
        };
        let services = match operation.services() {
            Ok(services) => services,
            Err(_) => return Ok(None),
        };
        operation
            .run(async move {
                let Some(provider) = services.embedding.get_active_provider().await else {
                    return Ok(None);
                };
                let model = provider.model_name().to_owned();
                let inputs: Vec<&str> = texts.iter().map(String::as_str).collect();
                let embeddings = provider.embed_batch(&inputs).await?;
                Ok(Some(MemoryEmbeddingBatch { model, embeddings }))
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::MemoryRuntime;
    use crate::capability::ownership::OwnershipTree;
    use crate::error::AlephError;
    use crate::memory::extensions::MemoryExtensionRegistry;
    use crate::memory::runtime::{MemoryRuntimeConfig, MemoryServicesConfig, MemoryState};
    use crate::memory::store::markdown::MarkdownSessionStore;
    use crate::memory::EmbeddingProvider;
    use crate::sync_primitives::{Arc, AtomicBool, Ordering};
    use async_trait::async_trait;
    use futures::FutureExt;
    use tokio::sync::Notify;

    struct Provider {
        entered: Notify,
        release: Notify,
        admitted: AtomicBool,
        effect: std::path::PathBuf,
    }

    #[async_trait]
    impl EmbeddingProvider for Provider {
        async fn embed(&self, text: &str) -> Result<Vec<f32>, AlephError> {
            Ok(vec![text.len() as f32, 1.0])
        }

        async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
            self.admitted.store(
                crate::memory::runtime::MemoryOperation::current().is_some(),
                Ordering::SeqCst,
            );
            self.entered.notify_one();
            self.release.notified().await;
            tokio::fs::write(&self.effect, "provider completed")
                .await
                .map_err(|error| AlephError::other(error.to_string()))?;
            Ok(texts
                .iter()
                .map(|text| vec![text.len() as f32, 1.0])
                .collect())
        }

        fn dimensions(&self) -> usize {
            2
        }

        fn model_name(&self) -> &str {
            "owned-probe"
        }

        fn provider_id(&self) -> &str {
            "local-test"
        }
    }

    #[tokio::test]
    async fn embedding_entry_retains_the_model_and_actual_provider_work() {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let root = tempfile::tempdir().unwrap();
            let raw = Arc::new(
                MarkdownSessionStore::new(&root.path().join("memory"))
                    .await
                    .unwrap(),
            );
            let mut config = crate::Config::default();
            config.memory.embedding.providers.clear();
            config.memory.dreaming.enabled = false;
            config.memory.profile.enabled = false;
            let runtime = MemoryRuntime::new_with_services(
                MemoryRuntimeConfig {
                    metadata_path: root.path().join("memory/metadata.db"),
                    note_memory_dir: root.path().join("notes"),
                },
                Arc::new(OwnershipTree::new()),
                MemoryServicesConfig {
                    app_config: Arc::new(tokio::sync::RwLock::new(config)),
                    raw_store: raw,
                    session_store: None,
                    provider: None,
                    extensions: Arc::new(MemoryExtensionRegistry::new()),
                    state_db: None,
                    default_agent_id: crate::routing::DEFAULT_AGENT_ID.into(),
                    curated_root: root.path().join("agents"),
                    snapshot_dir: root.path().join("snapshots"),
                },
            );
            assert!(runtime
                .embed_batch(vec!["cold".into()])
                .await
                .unwrap()
                .is_none());
            runtime.start().await.unwrap();
            let operation = runtime.operation().unwrap();
            let provider = Arc::new(Provider {
                entered: Notify::new(),
                release: Notify::new(),
                admitted: AtomicBool::new(false),
                effect: root.path().join("completed"),
            });
            operation
                .services()
                .unwrap()
                .embedding
                .install_provider_for_test(provider.clone())
                .await;
            drop(operation);
            let weak = Arc::downgrade(&provider);
            let mut work = Box::pin(runtime.embed_batch(vec!["real input".into()]));
            if work.as_mut().now_or_never().is_some() {
                drop(work);
                runtime.stop().await.unwrap();
                panic!("the embedding entry must run and retain the actual provider call");
            }
            provider.entered.notified().await;
            assert!(provider.admitted.load(Ordering::SeqCst));
            let mut stop = Box::pin(runtime.stop());
            assert!(stop.as_mut().now_or_never().is_none());
            drop(stop);
            drop(work);
            assert_eq!(runtime.status().state, MemoryState::Draining);
            provider.release.notify_one();
            drop(provider);
            runtime.stop().await.unwrap();
            assert_eq!(
                tokio::fs::read_to_string(root.path().join("completed"))
                    .await
                    .unwrap(),
                "provider completed"
            );
            assert!(weak.upgrade().is_none());
            assert!(runtime
                .embed_batch(vec!["off".into()])
                .await
                .unwrap()
                .is_none());
            runtime.start().await.unwrap();
            let provider = Arc::new(Provider {
                entered: Notify::new(),
                release: Notify::new(),
                admitted: AtomicBool::new(false),
                effect: root.path().join("second-completed"),
            });
            let operation = runtime.operation().unwrap();
            operation
                .services()
                .unwrap()
                .embedding
                .install_provider_for_test(provider.clone())
                .await;
            drop(operation);
            provider.release.notify_one();
            let batch = runtime
                .embed_batch(vec!["two".into()])
                .await
                .unwrap()
                .unwrap();
            assert_eq!(batch.model, "owned-probe");
            assert_eq!(batch.embeddings, vec![vec![3.0, 1.0]]);
            let conn = rusqlite::Connection::open_in_memory().unwrap();
            crate::tasks::heartbeat::dedup::init_dedup_schema(&conn).unwrap();
            let conn = Arc::new(tokio::sync::Mutex::new(conn));
            let dedup = crate::tasks::heartbeat::dedup::DedupEngine::from_runtime(
                crate::tasks::heartbeat::config::DedupConfig::default(),
                conn.clone(),
                runtime.clone(),
            );
            provider.release.notify_one();
            let crate::tasks::heartbeat::dedup::DedupVerdict::Fresh(embedding) =
                dedup.check("task", "delivered after Memory stopped").await
            else {
                panic!("the admitted provider must supply the fresh vector");
            };
            drop(provider);
            runtime.stop().await.unwrap();
            dedup
                .record_fresh("task", "delivered after Memory stopped", &embedding)
                .await;
            let model: String = conn
                .lock()
                .await
                .query_row("SELECT model FROM heartbeat_dedup", [], |row| row.get(0))
                .unwrap();
            assert_eq!(model, "owned-probe");
            assert!(matches!(
                dedup.check("task", "while Off").await,
                crate::tasks::heartbeat::dedup::DedupVerdict::Unavailable
            ));
        })
        .await
        .unwrap();
    }
}
