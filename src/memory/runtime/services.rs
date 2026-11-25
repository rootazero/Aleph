use std::path::PathBuf;

use crate::config::types::memory::MemoryConfig;
use crate::error::AlephError;
use crate::memory::assembler::hybrid::AiProviderSummaryLlm;
use crate::memory::compression::{CompressionConfig, CompressionService};
use crate::memory::dreaming::DreamDaemon;
use crate::memory::embedding_manager::EmbeddingManager;
use crate::memory::events::handler::MemoryCommandHandler;
use crate::memory::extensions::scheduler::MemoryProducerScheduler;
use crate::memory::extensions::MemoryExtensionRegistry;
use crate::memory::notes::governance::gate::{DefaultNoteWriteGate, GateThresholds, NoteWriteGate};
use crate::memory::notes::ingest::ingestor::DefaultCompoundIngestor;
use crate::memory::notes::ingest::retrieve::RelatedBudget;
use crate::memory::notes::orientation::{FsNoteOrientation, NoteOrientation};
use crate::memory::notes::profile::synthesizer::{FsProfileSynthesizer, ProfileSynthesizer};
use crate::memory::notes::query_filer::{DefaultQueryFiler, QueryFiler};
use crate::memory::notes::NoteIndexer;
use crate::memory::reflector::fs_reflector::RecallWriter;
use crate::memory::reflector::recall_signals::SignalRow;
use crate::memory::reflector::MemoryReflector;
use crate::memory::session_reflection::SessionReflector;
use crate::memory::session_resume::reader::SnapshotReader;
use crate::memory::session_search_summary::end_hook::SessionEndSummarizer;
use crate::memory::session_search_summary::synthesizer::SummarySynthesizer;
use crate::memory::store::raw_memory::RawMemoryStore;
use crate::memory::store::sqlite::recall_signals::RecallHit;
use crate::memory::store::sqlite::SqliteMemoryBackend;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use crate::thinker::{MemoryContextConfig, MemoryContextProvider};

use super::MemoryRuntimeConfig;

/// External inputs remain owned by chat/session and their canonical sources.
/// Constructing this value creates no Memory connection, worker or provider.
#[derive(Clone)]
pub struct MemoryServicesConfig {
    pub app_config: Arc<tokio::sync::RwLock<crate::Config>>,
    pub raw_store: Arc<dyn RawMemoryStore>,
    pub session_store: Option<Arc<dyn crate::gateway::session_store::SessionStore>>,
    pub provider: Option<Arc<dyn AiProvider>>,
    /// Resolve the Dream transport's credential when Memory is activated.
    pub provider_secrets: Option<Arc<dyn crate::secrets::injection::AsyncSecretResolver>>,
    pub extensions: Arc<MemoryExtensionRegistry>,
    pub state_db: Option<Arc<crate::resilience::database::StateDatabase>>,
    pub default_agent_id: String,
    pub curated_root: PathBuf,
    pub snapshot_dir: PathBuf,
}

pub(crate) struct MemoryServices {
    pub(crate) config: MemoryConfig,
    pub(crate) state_db: Option<Arc<crate::resilience::database::StateDatabase>>,
    pub(crate) raw_store: Arc<dyn RawMemoryStore>,
    pub(crate) default_agent_id: String,
    pub(crate) note_memory_dir: PathBuf,
    pub(crate) snapshot_dir: PathBuf,
    pub(crate) context: Arc<MemoryContextProvider>,
    pub(crate) orientation: Arc<dyn NoteOrientation>,
    pub(crate) profile: Option<Arc<dyn ProfileSynthesizer>>,
    pub(crate) indexer: Arc<NoteIndexer<SqliteMemoryBackend>>,
    pub(crate) embedding: Arc<EmbeddingManager>,
    pub(crate) dream: Option<Arc<DreamDaemon>>,
    pub(crate) compression: Option<Arc<CompressionService>>,
    pub(crate) summarizer: Option<Arc<SessionEndSummarizer>>,
    pub(crate) reflector: Option<Arc<SessionReflector>>,
    pub(crate) memory_reflector: Option<Arc<MemoryReflector>>,
    pub(crate) query_filer: Option<Arc<dyn QueryFiler>>,
    pub(super) producer: Arc<MemoryProducerScheduler>,
    pub(crate) reconciler: Option<Arc<MemoryCommandHandler>>,
    pub(crate) reembed: Arc<crate::gateway::handlers::memory::ReembedState>,
}

async fn build_dream_transport(
    config: &crate::Config,
    resolver: Option<&dyn crate::secrets::AsyncSecretResolver>,
) -> Option<Arc<dyn AiProvider>> {
    let primary = config.general.default_provider.as_deref()?;
    if let Some(resolver) = resolver.filter(|_| {
        config
            .providers
            .get(primary)
            .is_some_and(|p| p.api_key.is_none())
    }) {
        let key = crate::providers::probe::provider_vault_key(primary);
        if let Ok(secret) = resolver.resolve(&key).await {
            if !secret.expose().trim().is_empty() {
                let mut transport_config = config.clone();
                transport_config.providers.get_mut(primary)?.api_key =
                    Some(secret.expose().to_owned());
                return crate::orchestrator::build_dream_provider(&transport_config, primary);
            }
        }
    }
    crate::orchestrator::build_dream_provider(config, primary)
}

#[cfg(test)]
mod tests {
    use super::build_dream_transport;
    use crate::secrets::{AsyncSecretResolver, DecryptedSecret, SecretError};

    struct FixtureVault;

    #[async_trait::async_trait]
    impl AsyncSecretResolver for FixtureVault {
        async fn resolve(&self, name: &str) -> Result<DecryptedSecret, SecretError> {
            assert_eq!(name, "ai:relay");
            Ok(DecryptedSecret::new("dream-fixture-key"))
        }
    }

    #[tokio::test]
    async fn dream_transport_uses_vault_without_changing_the_config() {
        let mut config = crate::Config::default();
        config.general.default_provider = Some("relay".into());
        config.memory.dreaming.enabled = true;
        config.memory.dreaming.model = Some("org/dream".into());
        let mut provider = crate::config::ProviderConfig::test_config("org/chat");
        provider.api_key = None;
        provider.base_url = Some("https://relay.invalid/v1".into());
        config.providers.insert("relay".into(), provider);

        let dream = build_dream_transport(&config, Some(&FixtureVault))
            .await
            .unwrap();
        assert_eq!(dream.serving_model_hint().as_deref(), Some("org/dream"));
        assert!(
            crate::providers::model_catalog::discovery::transport_matches_listing(
                &config,
                "relay",
                dream.as_ref(),
                |_, _| Some("dream-fixture-key".into()),
            )
        );
        assert!(config.providers["relay"].api_key.is_none());
    }
}

impl MemoryServices {
    pub(super) async fn initialize(
        config: &MemoryRuntimeConfig,
        inputs: &MemoryServicesConfig,
        backend: Arc<SqliteMemoryBackend>,
    ) -> Result<Arc<Self>, AlephError> {
        // One activation reads one canonical snapshot, then owns that snapshot.
        let snapshot = inputs.app_config.read().await.clone();
        let memory = &snapshot.memory;
        let embedding = Arc::new(EmbeddingManager::new(memory.embedding.clone()));
        embedding.init().await?;
        let embedder = embedding.get_active_provider().await;
        let orientation: Arc<dyn NoteOrientation> = Arc::new(FsNoteOrientation::new(
            config.note_memory_dir.clone(),
            backend.clone(),
        ));
        let indexer = NoteIndexer::new(config.note_memory_dir.clone(), backend.clone());
        let indexer = Arc::new(match &embedder {
            Some(embedder) => indexer.with_embedder(embedder.clone()),
            None => indexer,
        });
        let context = MemoryContextProvider::assemble_at(
            config.note_memory_dir.clone(),
            Arc::new(SnapshotReader::new(inputs.snapshot_dir.clone())),
            backend.clone(),
            inputs.raw_store.clone(),
            embedder.clone(),
            inputs.provider.clone(),
            memory.assembler_config(),
            MemoryContextConfig::default(),
        )
        .with_injection_mode(memory.injection_mode)
        .with_open_loop_injection(
            memory.reflection.enabled && memory.reflection.open_loop_inject_prompt,
        )
        .with_extensions(inputs.extensions.clone())
        .with_curated_config(memory.curated.into())
        .with_curated_root(inputs.curated_root.clone())
        .with_orientation(orientation.clone())
        .with_orientation_budget(crate::memory::notes::orientation::types::TokenBudget {
            max_tokens: memory.orientation.max_tokens,
        });
        let profile = inputs
            .provider
            .as_ref()
            .filter(|_| memory.profile.enabled)
            .map(|provider| {
                Arc::new(
                    FsProfileSynthesizer::new(config.note_memory_dir.clone(), provider.clone())
                        .with_orientation(orientation.clone())
                        .with_config(&memory.profile),
                ) as Arc<dyn ProfileSynthesizer>
            });
        let context = Arc::new(match &profile {
            Some(profile) => context.with_profile(profile.clone()),
            None => context,
        });
        let compression = match (&inputs.provider, &embedder) {
            (Some(provider), Some(embedder)) => {
                let mut service = CompressionService::new(
                    backend.clone(),
                    inputs.raw_store.clone(),
                    provider.clone(),
                    embedder.clone(),
                    CompressionConfig::from_policy(&snapshot.policies.memory.compression),
                )
                .require_runtime()
                .with_extension_registry(inputs.extensions.clone());
                if memory.compound_ingest.enabled {
                    let compound = &memory.compound_ingest;
                    service = service.with_compound_ingestor(Arc::new(DefaultCompoundIngestor {
                        store: backend.clone(),
                        indexer: indexer.clone(),
                        provider: provider.clone(),
                        embedder: embedder.clone(),
                        orientation: Some(orientation.clone()),
                        memory_dir: config.note_memory_dir.clone(),
                        budget: RelatedBudget {
                            max_related_pages: compound.max_related_pages,
                            preview_char_cap: compound.related_preview_char_cap,
                            total_byte_cap: compound.related_total_byte_cap,
                            dedup_enabled: compound.dedup_enabled,
                            dedup_similarity_threshold: compound.dedup_similarity_threshold,
                            dedup_noop_threshold: compound.dedup_noop_threshold,
                        },
                        tx_residue_gc_seconds: compound.tx_residue_gc_seconds,
                        embedding_manager: Some(embedding.clone()),
                        gate: compound.governance_enabled.then(|| {
                            Arc::new(DefaultNoteWriteGate::new(
                                backend.clone(),
                                GateThresholds::default(),
                            )) as Arc<dyn NoteWriteGate>
                        }),
                    }));
                }
                if let Some(profile) = &profile {
                    service = service.with_profile_synthesizer(profile.clone());
                }
                let service = Arc::new(service);
                service.add_post_hook(context.clone()).await;
                Some(service)
            }
            _ => None,
        };
        let (summarizer, reflector) = match (&inputs.provider, &inputs.session_store) {
            (Some(provider), Some(sessions)) => {
                let llm = AiProviderSummaryLlm::new(provider.clone());
                let summarizer = Arc::new(SessionEndSummarizer {
                    store: inputs.raw_store.clone(),
                    synthesizer: Arc::new(SummarySynthesizer::new(
                        inputs.raw_store.clone(),
                        sessions.clone(),
                        llm.clone(),
                    )),
                });
                let reflector = memory.reflection.enabled.then(|| {
                    Arc::new(
                        SessionReflector::new(
                            inputs.raw_store.clone(),
                            sessions.clone(),
                            llm,
                            memory.reflection.clone(),
                        )
                        .with_cooldown_store(backend.clone()),
                    )
                });
                (Some(summarizer), reflector)
            }
            _ => (None, None),
        };
        let memory_reflector = inputs.provider.as_ref().map(|provider| {
            let store = backend.clone();
            let writer: RecallWriter = Arc::new(move |row: SignalRow| {
                let store = store.clone();
                Box::pin(async move {
                    store
                        .record_signals(
                            &row.query_text,
                            &row.channel,
                            &[RecallHit {
                                note_path: row.note_path,
                                score: f64::from(row.score),
                            }],
                            row.session_id.as_deref(),
                            &row.agent_id,
                            &row.namespace,
                        )
                        .map(|_| ())
                })
            });
            Arc::new(MemoryReflector::new(
                context.assembler(),
                provider.clone(),
                writer,
            ))
        });
        let query_filer = inputs
            .provider
            .as_ref()
            .filter(|_| memory.query_filer.enabled)
            .map(|provider| {
                Arc::new(DefaultQueryFiler {
                    store: backend.clone(),
                    indexer: indexer.clone(),
                    provider: provider.clone(),
                    orientation: Some(orientation.clone()),
                    memory_dir: config.note_memory_dir.clone(),
                    config: memory.query_filer.clone(),
                }) as Arc<dyn QueryFiler>
            });
        let dream = if memory.dreaming.enabled {
            let daemon = DreamDaemon::from_config(backend, memory)?
                .with_note_memory_dir(config.note_memory_dir.clone())
                .with_optional_raw_store(Some(inputs.raw_store.clone()))
                .with_orientation(orientation.clone());
            let provider = build_dream_transport(&snapshot, inputs.provider_secrets.as_deref())
                .await
                .or_else(|| inputs.provider.clone());
            let daemon = match provider {
                Some(provider) => daemon.with_provider(provider),
                None => daemon,
            };
            let daemon = match embedder {
                Some(embedder) => daemon.with_embedder(embedder),
                None => daemon,
            };
            let daemon = match &inputs.state_db {
                Some(database) => daemon.with_command_handler(Arc::new(
                    crate::memory::events::handler::MemoryCommandHandler::new(database.clone()),
                )),
                None => daemon,
            };
            Some(Arc::new(daemon))
        } else {
            None
        };
        let reconciler = inputs.state_db.as_ref().map(|database| {
            Arc::new(MemoryCommandHandler::new(database.clone()).with_note_indexer(indexer.clone()))
        });
        let producer = Arc::new(MemoryProducerScheduler::new(
            inputs.extensions.clone(),
            inputs.raw_store.clone(),
        ));
        Ok(Arc::new(Self {
            config: snapshot.memory,
            state_db: inputs.state_db.clone(),
            raw_store: inputs.raw_store.clone(),
            default_agent_id: inputs.default_agent_id.clone(),
            note_memory_dir: config.note_memory_dir.clone(),
            snapshot_dir: inputs.snapshot_dir.clone(),
            context,
            orientation,
            profile,
            indexer,
            embedding,
            dream,
            compression,
            summarizer,
            reflector,
            memory_reflector,
            query_filer,
            producer,
            reconciler,
            reembed: Arc::new(crate::gateway::handlers::memory::ReembedState::new()),
        }))
    }
}
