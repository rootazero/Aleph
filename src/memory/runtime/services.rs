use std::path::PathBuf;

use crate::config::types::memory::MemoryConfig;
use crate::error::AlephError;
use crate::memory::dreaming::DreamDaemon;
use crate::memory::embedding_manager::EmbeddingManager;
use crate::memory::extensions::MemoryExtensionRegistry;
use crate::memory::notes::orientation::{FsNoteOrientation, NoteOrientation};
use crate::memory::notes::profile::synthesizer::{FsProfileSynthesizer, ProfileSynthesizer};
use crate::memory::notes::NoteIndexer;
use crate::memory::session_resume::reader::SnapshotReader;
use crate::memory::store::raw_memory::RawMemoryStore;
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
    pub provider: Option<Arc<dyn AiProvider>>,
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
    pub(crate) context: Arc<MemoryContextProvider>,
    pub(crate) orientation: Arc<dyn NoteOrientation>,
    pub(crate) profile: Option<Arc<dyn ProfileSynthesizer>>,
    pub(crate) indexer: Arc<NoteIndexer<SqliteMemoryBackend>>,
    pub(crate) embedding: Arc<EmbeddingManager>,
    pub(crate) dream: Option<Arc<DreamDaemon>>,
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
        let indexer = match &embedder {
            Some(embedder) => indexer.with_embedder(embedder.clone()),
            None => indexer,
        };
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
        .with_extensions(inputs.extensions.clone())
        .with_curated_config(memory.curated.clone().into())
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
        let context = match &profile {
            Some(profile) => context.with_profile(profile.clone()),
            None => context,
        };
        let dream = if memory.dreaming.enabled {
            let daemon = DreamDaemon::from_config(backend, memory)?
                .with_note_memory_dir(config.note_memory_dir.clone())
                .with_optional_raw_store(Some(inputs.raw_store.clone()))
                .with_orientation(orientation.clone());
            let provider = crate::orchestrator::build_dream_provider(
                &snapshot,
                snapshot
                    .general
                    .default_provider
                    .as_deref()
                    .unwrap_or_default(),
            )
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
        Ok(Arc::new(Self {
            config: snapshot.memory,
            state_db: inputs.state_db.clone(),
            raw_store: inputs.raw_store.clone(),
            default_agent_id: inputs.default_agent_id.clone(),
            note_memory_dir: config.note_memory_dir.clone(),
            context: Arc::new(context),
            orientation,
            profile,
            indexer: Arc::new(indexer),
            embedding,
            dream,
        }))
    }
}
