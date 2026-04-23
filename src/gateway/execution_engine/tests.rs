//! Tests for the execution engine module.

use super::engine::wait_for_deadline;
use super::*;
use crate::sync_primitives::{AtomicUsize, Ordering};

use crate::gateway::agent_instance::{AgentInstance, AgentInstanceConfig};
use crate::gateway::event_emitter::{EventEmitError, EventEmitter, StreamEvent};
use crate::gateway::router::SessionKey;

use crate::sync_primitives::{Arc, Mutex};
use async_trait::async_trait;
use tokio::sync::RwLock;

/// Test event emitter that collects events
struct TestEmitter {
    events: Arc<RwLock<Vec<StreamEvent>>>,
    event_count: AtomicUsize,
    seq_counter: AtomicU64,
}

impl TestEmitter {
    fn new() -> Self {
        Self {
            events: Arc::new(RwLock::new(Vec::new())),
            event_count: AtomicUsize::new(0),
            seq_counter: AtomicU64::new(0),
        }
    }

    async fn get_events(&self) -> Vec<StreamEvent> {
        self.events.read().await.clone()
    }
}

#[async_trait]
impl EventEmitter for TestEmitter {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        self.events.write().await.push(event);
        self.event_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn next_seq(&self) -> u64 {
        self.seq_counter.fetch_add(1, Ordering::SeqCst)
    }
}

fn test_session_manager(
    temp: &tempfile::TempDir,
) -> Arc<crate::gateway::session_manager::SessionManager> {
    let config = crate::gateway::session_manager::SessionManagerConfig {
        db_path: temp.path().join("test_sessions.db"),
        ..Default::default()
    };
    Arc::new(
        crate::gateway::session_manager::SessionManager::new(config).expect("test session manager"),
    )
}

#[tokio::test]
async fn test_simple_execution_engine_basic() {
    let temp = tempfile::tempdir().unwrap();
    let sm = test_session_manager(&temp);
    let config = AgentInstanceConfig {
        agent_id: "test".to_string(),
        workspace: temp.path().join("workspace"),
        agent_dir: temp.path().join("agents/test"),
        ..Default::default()
    };

    let agent = Arc::new(AgentInstance::new(config, sm).unwrap());
    let emitter = Arc::new(TestEmitter::new());
    let engine = SimpleExecutionEngine::default();

    let request = RunRequest {
        run_id: "test-run-1".to_string(),
        input: "Hello, world!".to_string(),
        session_key: SessionKey::main("test"),
        timeout_secs: None,
        metadata: HashMap::new(),
        attachments: Vec::new(),
        pending_media: Arc::new(Mutex::new(Vec::new())),
    };

    let result = engine.execute(request, agent, emitter.clone()).await;
    assert!(result.is_ok());

    let events = emitter.get_events().await;
    assert!(!events.is_empty());

    // Check for expected events
    let has_run_accepted = events
        .iter()
        .any(|e| matches!(e, StreamEvent::RunAccepted { .. }));
    let has_run_complete = events
        .iter()
        .any(|e| matches!(e, StreamEvent::RunComplete { .. }));

    assert!(has_run_accepted, "Should have RunAccepted event");
    assert!(has_run_complete, "Should have RunComplete event");
}

#[tokio::test]
async fn test_simple_execution_engine_run() {
    let temp = tempfile::tempdir().unwrap();
    let sm = test_session_manager(&temp);
    let config = AgentInstanceConfig {
        agent_id: "test-simple".to_string(),
        workspace: temp.path().join("workspace"),
        agent_dir: temp.path().join("agents/test-simple"),
        ..Default::default()
    };

    let agent = Arc::new(AgentInstance::new(config, sm).unwrap());
    let emitter = Arc::new(TestEmitter::new());
    let engine = SimpleExecutionEngine::new(ExecutionEngineConfig {
        default_timeout_secs: 10,
        ..Default::default()
    });

    let request = RunRequest {
        run_id: "run-simple".to_string(),
        input: "Test input".to_string(),
        session_key: SessionKey::main("test-simple"),
        timeout_secs: Some(5),
        metadata: HashMap::new(),
        attachments: Vec::new(),
        pending_media: Arc::new(Mutex::new(Vec::new())),
    };

    // This should succeed and complete quickly
    let result = engine
        .execute(request, agent.clone(), emitter.clone())
        .await;
    assert!(result.is_ok());

    // Verify events were emitted
    let events = emitter.get_events().await;
    let has_reasoning = events
        .iter()
        .any(|e| matches!(e, StreamEvent::Reasoning { .. }));
    let has_response = events
        .iter()
        .any(|e| matches!(e, StreamEvent::ResponseChunk { .. }));

    assert!(has_reasoning, "Should have Reasoning event");
    assert!(has_response, "Should have ResponseChunk event");
}

// =============================================================================
// Cascade timeout resolution
// =============================================================================

/// Helper: resolve timeout using the same cascade as engine.rs:349-352
fn resolve_timeout(
    request_timeout: Option<u64>,
    agent_timeout: Option<u64>,
    engine_default: u64,
) -> u64 {
    request_timeout.or(agent_timeout).unwrap_or(engine_default)
}

#[test]
fn test_cascade_request_wins_over_agent_and_global() {
    assert_eq!(resolve_timeout(Some(30), Some(600), 172_800), 30);
}

#[test]
fn test_cascade_agent_wins_over_global() {
    assert_eq!(resolve_timeout(None, Some(3600), 172_800), 3600);
}

#[test]
fn test_cascade_falls_through_to_global() {
    assert_eq!(resolve_timeout(None, None, 172_800), 172_800);
}

#[test]
fn test_cascade_request_overrides_even_when_agent_is_none() {
    assert_eq!(resolve_timeout(Some(60), None, 172_800), 60);
}

#[test]
fn test_cascade_matches_engine_default() {
    let engine_config = ExecutionEngineConfig::default();
    assert_eq!(
        resolve_timeout(None, None, engine_config.default_timeout_secs),
        172_800
    );
}

// =============================================================================
// wait_for_deadline — resettable deadline behavior
// =============================================================================

#[tokio::test]
async fn test_wait_for_deadline_fires_at_deadline() {
    let deadline = Arc::new(tokio::sync::Mutex::new(
        tokio::time::Instant::now() + tokio::time::Duration::from_millis(100),
    ));

    let start = tokio::time::Instant::now();
    wait_for_deadline(deadline).await;
    let elapsed = start.elapsed();

    // Should fire after ~100ms (with some tolerance)
    assert!(
        elapsed >= tokio::time::Duration::from_millis(80),
        "fired too early: {:?}",
        elapsed
    );
    assert!(
        elapsed < tokio::time::Duration::from_millis(500),
        "fired too late: {:?}",
        elapsed
    );
}

#[tokio::test]
async fn test_wait_for_deadline_extension_delays_firing() {
    let deadline = Arc::new(tokio::sync::Mutex::new(
        tokio::time::Instant::now() + tokio::time::Duration::from_millis(100),
    ));

    let deadline_clone = deadline.clone();

    // Spawn a task that extends the deadline after 50ms
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        // Extend deadline by 200ms from now
        *deadline_clone.lock().await =
            tokio::time::Instant::now() + tokio::time::Duration::from_millis(200);
    });

    let start = tokio::time::Instant::now();
    wait_for_deadline(deadline).await;
    let elapsed = start.elapsed();

    // Should fire after ~250ms (50ms wait + 200ms extended), not at 100ms
    assert!(
        elapsed >= tokio::time::Duration::from_millis(200),
        "deadline extension was ignored, fired too early: {:?}",
        elapsed
    );
    assert!(
        elapsed < tokio::time::Duration::from_secs(1),
        "fired too late: {:?}",
        elapsed
    );
}

#[tokio::test]
async fn test_wait_for_deadline_multiple_extensions() {
    let deadline = Arc::new(tokio::sync::Mutex::new(
        tokio::time::Instant::now() + tokio::time::Duration::from_millis(50),
    ));

    let dl = deadline.clone();

    // Extend twice: at 30ms and at 100ms
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(30)).await;
        *dl.lock().await = tokio::time::Instant::now() + tokio::time::Duration::from_millis(100);

        tokio::time::sleep(tokio::time::Duration::from_millis(80)).await;
        *dl.lock().await = tokio::time::Instant::now() + tokio::time::Duration::from_millis(100);
    });

    let start = tokio::time::Instant::now();
    wait_for_deadline(deadline).await;
    let elapsed = start.elapsed();

    // Should fire after ~210ms (30 + 80 + 100), not at 50ms
    assert!(
        elapsed >= tokio::time::Duration::from_millis(180),
        "multiple extensions were ignored: {:?}",
        elapsed
    );
    assert!(
        elapsed < tokio::time::Duration::from_secs(2),
        "fired too late: {:?}",
        elapsed
    );
}
