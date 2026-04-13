//! Tests for G1 pre-compress hook: RawMemory emission before session drain.

use crate::components::session_compactor::SessionCompactor;
use crate::components::types::{ExecutionSession, SessionPart, ToolCallPart, ToolCallStatus, UserInputPart};
use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
use async_trait::async_trait;
use serde_json::json;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Fake in-memory RawMemoryStore that captures inserts
// ---------------------------------------------------------------------------

#[derive(Default)]
struct FakeWriter(Mutex<Vec<RawMemory>>);

#[async_trait]
impl RawMemoryStore for FakeWriter {
    async fn insert_raw_memory(&self, raw: &RawMemory) -> Result<(), crate::error::AlephError> {
        self.0.lock().unwrap().push(raw.clone());
        Ok(())
    }

    async fn get_unprocessed_raw_memories(
        &self,
        _agent_id: &str,
        _limit: usize,
    ) -> Result<Vec<RawMemory>, crate::error::AlephError> {
        Ok(vec![])
    }

    async fn mark_raw_as_processed(
        &self,
        _ids: &[String],
    ) -> Result<usize, crate::error::AlephError> {
        Ok(0)
    }

    async fn count_unprocessed(
        &self,
        _agent_id: &str,
    ) -> Result<usize, crate::error::AlephError> {
        Ok(0)
    }

    async fn get_raw_by_path_prefix(
        &self,
        _path_prefix: &str,
        _agent_id: &str,
        _limit: usize,
    ) -> Result<Vec<RawMemory>, crate::error::AlephError> {
        Ok(vec![])
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn fake_compactor_with_writer(writer: Arc<dyn RawMemoryStore>) -> SessionCompactor {
    SessionCompactor::with_keep_recent(3).with_raw_memory_writer(writer)
}

fn fake_session_with_parts(n: usize) -> ExecutionSession {
    let mut session = ExecutionSession::new().with_model("gpt-4-turbo");
    session.agent_id = "test-agent".to_string();

    // Add a user input first so parts > keep_recent triggers compaction
    session.parts.push(SessionPart::UserInput(UserInputPart {
        text: "test request".to_string(),
        context: None,
        timestamp: 1000,
    }));

    for i in 0..n {
        session.parts.push(SessionPart::ToolCall(ToolCallPart {
            id: format!("tool-{i}"),
            tool_name: format!("tool_{i}"),
            input: json!({"param": i}),
            status: ToolCallStatus::Completed,
            output: Some(format!("output for tool {i}")),
            error: None,
            started_at: 1000 + i as i64 * 100,
            completed_at: Some(1050 + i as i64 * 100),
        }));
    }

    session
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn replace_with_summary_emits_pre_compress_raw_memory() {
    let fake = Arc::new(FakeWriter::default());
    let writer: Arc<dyn RawMemoryStore> = fake.clone();

    let compactor = fake_compactor_with_writer(writer);
    // 5 tool calls + 1 user input = 6 parts; keep_recent=3 → compact_count=3
    let mut session = fake_session_with_parts(5);

    compactor.replace_with_summary(&mut session, "synthetic summary".into());

    // Give the fire-and-forget spawn time to run.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let captured = fake.0.lock().unwrap();
    assert_eq!(
        captured.len(),
        1,
        "expected exactly one pre_compress raw_memory write"
    );
    assert_eq!(
        captured[0].source,
        RawMemorySource::PreCompress,
        "source must be PreCompress"
    );
    assert!(
        !captured[0].content.is_empty(),
        "doomed_text content should not be empty"
    );
    assert_eq!(
        captured[0].agent_id, "test-agent",
        "agent_id must be propagated"
    );
    assert!(
        captured[0].session_id.is_some(),
        "session_id must be propagated"
    );
}

#[tokio::test]
async fn replace_with_summary_no_emit_when_nothing_to_compact() {
    let fake = Arc::new(FakeWriter::default());
    let writer: Arc<dyn RawMemoryStore> = fake.clone();

    // keep_recent=100 so compact_count will be 0
    let compactor = SessionCompactor::with_keep_recent(100).with_raw_memory_writer(writer);
    let mut session = fake_session_with_parts(2);

    compactor.replace_with_summary(&mut session, "summary".into());
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let captured = fake.0.lock().unwrap();
    assert_eq!(
        captured.len(),
        0,
        "no emit expected when compact_count == 0"
    );
}

#[tokio::test]
async fn replace_with_summary_no_emit_without_writer() {
    // No writer attached — must not panic, compaction proceeds normally
    let compactor = SessionCompactor::with_keep_recent(3);
    let mut session = fake_session_with_parts(5);

    compactor.replace_with_summary(&mut session, "summary".into());
    // Just verify it doesn't panic and session is in expected state
    // 1 summary + 3 kept = 4 parts
    assert_eq!(session.parts.len(), 4);
}
