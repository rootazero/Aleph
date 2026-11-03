use super::{CompressionConfig, CompressionService};
use crate::error::AlephError;
use crate::memory::notes::ingest::{ApplyReport, CompoundIngestor};
use crate::memory::notes::profile::{
    ProfileSynthesizer, SessionSignal, UpdateOutcome, UserProfile,
};
use crate::memory::runtime::{MemoryOperation, MemoryRuntime, MemoryRuntimeConfig, MemoryState};
use crate::memory::store::markdown::MarkdownSessionStore;
use crate::memory::store::raw_memory::{
    RawMemory, RawMemorySource, RawMemoryStore, SessionEndReason,
};
use crate::memory::store::sqlite::SqliteMemoryBackend;
use crate::sync_primitives::{Arc, AtomicBool, Ordering};
use std::pin::pin;
use std::task::Poll;
use std::time::Duration;
use tokio::sync::Notify;

#[derive(Default)]
struct Phase {
    entered: Notify,
    release: Notify,
    admitted: AtomicBool,
}

impl Phase {
    async fn wait(&self) {
        self.admitted
            .store(MemoryOperation::current().is_some(), Ordering::SeqCst);
        self.entered.notify_one();
        self.release.notified().await;
    }
}

struct Ingestor {
    phase: Option<Arc<Phase>>,
}

#[async_trait::async_trait]
impl CompoundIngestor for Ingestor {
    async fn ingest_batch(
        &self,
        _agent_id: &str,
        _raws: Vec<RawMemory>,
        _extra_context: Option<&str>,
    ) -> Result<ApplyReport, AlephError> {
        if let Some(phase) = &self.phase {
            phase.wait().await;
        }
        effect("compression_tick_effect").await?;
        Ok(ApplyReport::default())
    }
}

struct Profile {
    phase: Arc<Phase>,
    path: std::path::PathBuf,
}

#[async_trait::async_trait]
impl ProfileSynthesizer for Profile {
    async fn bootstrap(&self, _agent_id: &str) -> Result<UserProfile, AlephError> {
        Err(AlephError::other("this fixture exercises update only"))
    }

    async fn current(&self, _agent_id: &str) -> Result<Option<UserProfile>, AlephError> {
        Ok(None)
    }

    async fn update(
        &self,
        _agent_id: &str,
        signal: SessionSignal,
    ) -> Result<UpdateOutcome, AlephError> {
        self.phase.wait().await;
        effect("compression_profile_effect").await?;
        tokio::fs::write(&self.path, signal.digest_text)
            .await
            .map_err(|error| AlephError::other(error.to_string()))?;
        Ok(UpdateOutcome::Bootstrapped { revision: 1 })
    }
}

async fn effect(table: &'static str) -> Result<(), AlephError> {
    let operation = MemoryOperation::current()
        .ok_or_else(|| AlephError::other("compression child lost its source operation"))?;
    operation
        .backend()
        .with_conn(move |connection| {
            connection
                .execute_batch(&format!(
                    "CREATE TABLE IF NOT EXISTS {table}(value INTEGER); INSERT INTO {table} VALUES(1);"
                ))
                .map_err(|error| AlephError::other(error.to_string()))?;
            Ok(())
        })
        .await
}

struct Fixture {
    dir: tempfile::TempDir,
    runtime: Arc<MemoryRuntime>,
    raw: Arc<MarkdownSessionStore>,
    operation: MemoryOperation,
}

impl Fixture {
    async fn new(source: RawMemorySource) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let raw = Arc::new(MarkdownSessionStore::new(dir.path()).await.unwrap());
        let runtime = MemoryRuntime::new(
            MemoryRuntimeConfig {
                metadata_path: dir.path().join("metadata.db"),
                note_memory_dir: dir.path().join("note"),
            },
            Arc::new(crate::capability::ownership::OwnershipTree::new()),
        );
        runtime.start().await.unwrap();
        let operation = runtime.operation().unwrap();
        let mut memory = RawMemory::new("A substantive session digest".into(), source);
        memory.agent_id = "default".into();
        raw.insert_raw_memory(&memory).await.unwrap();
        Self {
            dir,
            runtime,
            raw,
            operation,
        }
    }

    fn service(&self, phase: Option<Arc<Phase>>) -> CompressionService {
        let mut config = CompressionConfig::default();
        config.scheduler.turn_threshold = 1;
        config.background_interval_seconds = 1;
        CompressionService::new(
            self.operation.backend().clone(),
            self.raw.clone(),
            crate::providers::create_mock_provider(),
            Arc::new(
                crate::memory::embedding_provider::tests::MockEmbeddingProvider::new(8, "unused"),
            ),
            config,
        )
        .with_compound_ingestor(Arc::new(Ingestor { phase }))
    }

    fn standalone_runtime_bound_service(&self) -> CompressionService {
        let mut config = CompressionConfig::default();
        config.scheduler.turn_threshold = 1;
        CompressionService::new(
            Arc::new(SqliteMemoryBackend::new(self.dir.path()).unwrap()),
            self.raw.clone(),
            crate::providers::create_mock_provider(),
            Arc::new(
                crate::memory::embedding_provider::tests::MockEmbeddingProvider::new(8, "unused"),
            ),
            config,
        )
        .require_runtime()
    }
}

#[tokio::test]
async fn runtime_bound_compression_rejects_unadmitted_fallback() {
    let fixture = Fixture::new(RawMemorySource::Transcript).await;
    let service = Arc::new(fixture.standalone_runtime_bound_service());

    drop(fixture.operation);
    fixture.runtime.stop().await.unwrap();
    service.record_turn_and_maybe_compress();

    assert_eq!(
        service.get_scheduler().get_pending_turns(),
        0,
        "runtime-bound compression must not consume a turn or spawn detached derived work without admission"
    );
    assert_eq!(fixture.raw.count_unprocessed("default").await.unwrap(), 1);
}

#[tokio::test]
async fn compression_threshold_and_profile_children_are_observed() {
    for background in [false, true] {
        let fixture = Fixture::new(RawMemorySource::Transcript).await;
        let phase = Arc::new(Phase::default());
        let service = Arc::new(fixture.service(Some(phase.clone())));
        if background {
            fixture
                .runtime
                .retain_worker(service.start_owned_background_task(
                    fixture.runtime.clone(),
                    fixture.runtime.worker_stop().unwrap(),
                ));
        } else {
            fixture
                .operation
                .clone()
                .scope(async {
                    service.record_turn_and_maybe_compress();
                })
                .await;
        }
        tokio::time::timeout(Duration::from_secs(10), phase.entered.notified())
            .await
            .unwrap();
        assert!(
            phase.admitted.load(Ordering::SeqCst),
            "each compression trigger must carry a source operation"
        );
        drop(fixture.operation);
        let mut stop = pin!(fixture.runtime.stop());
        assert!(matches!(futures::poll!(&mut stop), Poll::Pending));
        assert_eq!(fixture.runtime.status().state, MemoryState::Draining);
        phase.release.notify_one();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(10), &mut stop)
                .await
                .unwrap()
                .unwrap()
                .state,
            MemoryState::Off
        );
        assert_eq!(fixture.raw.count_unprocessed("default").await.unwrap(), 0);
        let database = rusqlite::Connection::open(fixture.dir.path().join("metadata.db")).unwrap();
        assert_eq!(
            database
                .query_row(
                    "SELECT SUM(value) FROM compression_tick_effect",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
    }
}

#[tokio::test]
async fn admitted_flush_drains_before_runtime_stop_completes() {
    use crate::memory::flush::{flush_agent_memory, FlushRegistry};

    let fixture = Fixture::new(RawMemorySource::Transcript).await;
    let phase = Arc::new(Phase::default());
    let service = Arc::new(fixture.service(Some(phase.clone())));
    let registry = FlushRegistry::new();
    let guard = registry.begin("default");
    let mut ready = pin!(registry.await_ready("default", Duration::from_secs(10)));
    assert!(matches!(futures::poll!(&mut ready), Poll::Pending));

    let (sent, received) = tokio::sync::oneshot::channel();
    let flush_service = service.clone();
    fixture
        .operation
        .spawn(async move {
            flush_agent_memory(guard, "default".into(), flush_service).await;
            sent.send(()).unwrap();
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), phase.entered.notified())
        .await
        .unwrap();
    assert!(phase.admitted.load(Ordering::SeqCst));

    drop(fixture.operation);
    let mut stop = pin!(fixture.runtime.stop());
    assert!(matches!(futures::poll!(&mut stop), Poll::Pending));
    assert_eq!(fixture.runtime.status().state, MemoryState::Draining);
    phase.release.notify_one();
    received.await.unwrap();
    assert!(ready.await);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), &mut stop)
            .await
            .unwrap()
            .unwrap()
            .state,
        MemoryState::Off
    );
    assert_eq!(fixture.raw.count_unprocessed("default").await.unwrap(), 0);
}

#[tokio::test]
async fn profile_child_drains_after_compression_returns() {
    let fixture = Fixture::new(RawMemorySource::SessionEnd {
        reason: SessionEndReason::Disconnect,
    })
    .await;
    let phase = Arc::new(Phase::default());
    let profile_path = fixture.dir.path().join("USER.md");
    let service = Arc::new(
        fixture
            .service(None)
            .with_profile_synthesizer(Arc::new(Profile {
                phase: phase.clone(),
                path: profile_path.clone(),
            })),
    );
    fixture
        .operation
        .run(async move {
            service.compress().await?;
            Ok(())
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), phase.entered.notified())
        .await
        .unwrap();
    assert!(
        phase.admitted.load(Ordering::SeqCst),
        "the profile child must inherit the completed compression's admission"
    );
    assert_eq!(fixture.raw.count_unprocessed("default").await.unwrap(), 0);
    drop(fixture.operation);
    {
        let mut stop = pin!(fixture.runtime.stop());
        assert!(matches!(futures::poll!(&mut stop), Poll::Pending));
    }
    let mut retry = pin!(fixture.runtime.stop());
    assert!(matches!(futures::poll!(&mut retry), Poll::Pending));
    phase.release.notify_one();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), &mut retry)
            .await
            .unwrap()
            .unwrap()
            .state,
        MemoryState::Off
    );
    assert!(tokio::fs::read_to_string(profile_path)
        .await
        .unwrap()
        .contains("substantive session digest"));
    let database = rusqlite::Connection::open(fixture.dir.path().join("metadata.db")).unwrap();
    assert_eq!(
        database
            .query_row(
                "SELECT SUM(value) FROM compression_profile_effect",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}
