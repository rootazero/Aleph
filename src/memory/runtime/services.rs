use std::path::PathBuf;

use crate::config::types::memory::MemoryConfig;
use crate::error::AlephError;
use crate::memory::embedding_manager::EmbeddingManager;
use crate::memory::extensions::MemoryExtensionRegistry;
use crate::memory::notes::orientation::{FsNoteOrientation, NoteOrientation};
use crate::memory::notes::profile::synthesizer::FsProfileSynthesizer;
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
    pub memory: MemoryConfig,
    pub raw_store: Arc<dyn RawMemoryStore>,
    pub provider: Option<Arc<dyn AiProvider>>,
    pub extensions: Arc<MemoryExtensionRegistry>,
    pub default_agent_id: String,
    pub curated_root: PathBuf,
    pub snapshot_dir: PathBuf,
}

pub(crate) struct MemoryServices {
    pub(crate) config: MemoryConfig,
    pub(crate) raw_store: Arc<dyn RawMemoryStore>,
    pub(crate) default_agent_id: String,
    pub(crate) note_memory_dir: PathBuf,
    pub(crate) context: Arc<MemoryContextProvider>,
    pub(crate) orientation: Arc<dyn NoteOrientation>,
    pub(crate) indexer: Arc<NoteIndexer<SqliteMemoryBackend>>,
    pub(crate) embedding: Arc<EmbeddingManager>,
}

impl MemoryServices {
    pub(super) async fn initialize(
        config: &MemoryRuntimeConfig,
        inputs: &MemoryServicesConfig,
        backend: Arc<SqliteMemoryBackend>,
    ) -> Result<Arc<Self>, AlephError> {
        let embedding = Arc::new(EmbeddingManager::new(inputs.memory.embedding.clone()));
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
            backend,
            inputs.raw_store.clone(),
            embedder,
            inputs.provider.clone(),
            inputs.memory.assembler_config(),
            MemoryContextConfig::default(),
        )
        .with_injection_mode(inputs.memory.injection_mode)
        .with_extensions(inputs.extensions.clone())
        .with_curated_config(inputs.memory.curated.clone().into())
        .with_curated_root(inputs.curated_root.clone())
        .with_orientation(orientation.clone())
        .with_orientation_budget(crate::memory::notes::orientation::types::TokenBudget {
            max_tokens: inputs.memory.orientation.max_tokens,
        });
        let context = match &inputs.provider {
            Some(provider) => context.with_profile(Arc::new(FsProfileSynthesizer::new(
                config.note_memory_dir.clone(),
                provider.clone(),
            ))),
            None => context,
        };
        Ok(Arc::new(Self {
            config: inputs.memory.clone(),
            raw_store: inputs.raw_store.clone(),
            default_agent_id: inputs.default_agent_id.clone(),
            note_memory_dir: config.note_memory_dir.clone(),
            context: Arc::new(context),
            orientation,
            indexer: Arc::new(indexer),
            embedding,
        }))
    }
}
