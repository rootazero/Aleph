//! Tests for the execution engine module.

use super::*;
use crate::sync_primitives::{AtomicUsize, Ordering};

use crate::gateway::agent_instance::{AgentInstance, AgentInstanceConfig};
use crate::gateway::event_emitter::{EventEmitError, EventEmitter, StreamEvent};
use crate::gateway::router::SessionKey;

use async_trait::async_trait;
use crate::sync_primitives::Arc;
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

#[tokio::test]
async fn test_simple_execution_engine_basic() {
    let temp = tempfile::tempdir().unwrap();
    let config = AgentInstanceConfig {
        agent_id: "test".to_string(),
        workspace: temp.path().join("workspace"),
        agent_dir: temp.path().join("agents/test"),
        ..Default::default()
    };

    let agent = Arc::new(AgentInstance::new(config).unwrap());
    let emitter = Arc::new(TestEmitter::new());
    let engine = SimpleExecutionEngine::default();

    let request = RunRequest {
        run_id: "test-run-1".to_string(),
        input: "Hello, world!".to_string(),
        session_key: SessionKey::main("test"),
        timeout_secs: None,
        metadata: HashMap::new(),
    };

    let result = engine.execute(request, agent, emitter.clone()).await;
    assert!(result.is_ok());

    let events = emitter.get_events().await;
    assert!(!events.is_empty());

    // Check for expected events
    let has_run_accepted = events.iter().any(|e| matches!(e, StreamEvent::RunAccepted { .. }));
    let has_run_complete = events.iter().any(|e| matches!(e, StreamEvent::RunComplete { .. }));

    assert!(has_run_accepted, "Should have RunAccepted event");
    assert!(has_run_complete, "Should have RunComplete event");
}

#[tokio::test]
async fn test_simple_execution_engine_run() {
    let temp = tempfile::tempdir().unwrap();
    let config = AgentInstanceConfig {
        agent_id: "test-simple".to_string(),
        workspace: temp.path().join("workspace"),
        agent_dir: temp.path().join("agents/test-simple"),
        ..Default::default()
    };

    let agent = Arc::new(AgentInstance::new(config).unwrap());
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
    };

    // This should succeed and complete quickly
    let result = engine.execute(request, agent.clone(), emitter.clone()).await;
    assert!(result.is_ok());

    // Verify events were emitted
    let events = emitter.get_events().await;
    let has_reasoning = events.iter().any(|e| matches!(e, StreamEvent::Reasoning { .. }));
    let has_response = events.iter().any(|e| matches!(e, StreamEvent::ResponseChunk { .. }));

    assert!(has_reasoning, "Should have Reasoning event");
    assert!(has_response, "Should have ResponseChunk event");
}
