//! `ToolSignalSink` — per-invocation tool metric capture.
//!
//! Spec 3 wiring: every tool call completion (success or failure) flows
//! through a `ToolSignalSink` and lands as a [`RawMemorySource::ToolInvocation`]
//! row in `raw_memories`.
//!
//! These rows have exactly TWO readers, and both go through
//! [`crate::memory::insights`]:
//!
//! * the read-only `insights.tools` aggregation behind the admin RPC
//!   ([`crate::memory::insights::aggregate_tool_usage`]) — a number for a human;
//! * the nightly
//!   [`tool_failure_distill`](crate::memory::dreaming::stages::tool_failure_distill)
//!   dream stage ([`crate::memory::insights::aggregate_tool_failures`]) — the
//!   failure half turned into `lesson/` notes the model can retrieve.
//!
//! `compute_raw_metrics()` is still NOT one: it derives its signals from note
//! index entries, `recall_hit_counts` and the prior `DreamReport`, never from
//! `raw_memories`. `CompressionService` deliberately bypasses these rows too
//! (they are telemetry, not knowledge). That is the whole consumer list; keep
//! it that way or update these lines.
//!
//! The `content` string built below is not just a log line — it is the verbatim
//! evidence the distiller shows the model. Changing its shape changes what the
//! nightly prompt sees.
//!
//! The sink is per-session: `agent_id` and `session_id` are captured at
//! construction time so the harness side just calls `record(...)` without
//! threading identifiers through.
//!
//! Default impl is `NoopToolSignalSink` — zero overhead when the production
//! wiring path is not in place (tests, simple-engine fixtures).

use crate::sync_primitives::Arc;

use async_trait::async_trait;

use crate::memory::content_scanner::{scan_content, ScanVerdict};
use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};

/// Fire-and-forget sink for per-tool-invocation signals.
///
/// Implementations MUST NOT block: production callers invoke this from
/// `tokio::spawn` so a slow store cannot back-pressure the agent harness.
#[async_trait]
pub trait ToolSignalSink: Send + Sync {
    async fn record_tool_call(
        &self,
        tool_name: &str,
        success: bool,
        duration_ms: u64,
        error: Option<&str>,
    );
}

/// No-op sink for tests / paths without a wired `RawMemoryStore`.
pub struct NoopToolSignalSink;

#[async_trait]
impl ToolSignalSink for NoopToolSignalSink {
    async fn record_tool_call(
        &self,
        _tool_name: &str,
        _success: bool,
        _duration_ms: u64,
        _error: Option<&str>,
    ) {
    }
}

/// Production sink: writes one `RawMemory` row per tool invocation.
/// `agent_id` and `session_id` are baked in at construction so the harness
/// hot path doesn't need to resolve them.
pub struct RawMemoryToolSink {
    store: Arc<dyn RawMemoryStore>,
    agent_id: String,
    session_id: String,
}

impl RawMemoryToolSink {
    pub fn new(
        store: Arc<dyn RawMemoryStore>,
        agent_id: impl Into<String>,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            store,
            agent_id: agent_id.into(),
            session_id: session_id.into(),
        }
    }
}

#[async_trait]
impl ToolSignalSink for RawMemoryToolSink {
    async fn record_tool_call(
        &self,
        tool_name: &str,
        success: bool,
        duration_ms: u64,
        error: Option<&str>,
    ) {
        let content = match (success, error) {
            (true, _) => format!("tool {tool_name} ok in {duration_ms}ms"),
            (false, Some(err)) => {
                let trimmed: String = err.chars().take(200).collect();
                format!("tool {tool_name} failed in {duration_ms}ms: {trimmed}")
            }
            (false, None) => format!("tool {tool_name} failed in {duration_ms}ms"),
        };

        // The constructed `content` is the verbatim evidence the distiller
        // shows the model at nightly dream time. A malicious or corrupted
        // `tool_name` / error string can therefore become a prompt-injection
        // vector into the dream-stage LLM. Run the same content_scanner
        // used everywhere else in the memory pipeline; if it rejects the
        // row, replace the body with a sanitized placeholder so the failure
        // signal still lands but the suspicious payload does not.
        let content = match scan_content(&content) {
            ScanVerdict::Clean => content,
            ScanVerdict::Rejected { reason, pattern } => {
                tracing::warn!(
                    tool = %tool_name,
                    agent = %self.agent_id,
                    session = %self.session_id,
                    pattern,
                    %reason,
                    "tool signal content rejected by content_scanner; \
                     persisting redacted placeholder"
                );
                format!("tool {tool_name} failed in {duration_ms}ms: [content redacted: {reason}]")
            }
        };

        let raw = RawMemory::new(
            content,
            RawMemorySource::ToolInvocation {
                tool_name: tool_name.to_string(),
                success,
                duration_ms,
            },
        )
        .with_agent(self.agent_id.clone())
        .with_session(self.session_id.clone());

        if let Err(e) = self.store.insert_raw_memory(&raw).await {
            // Silent failure: tool-signal capture must never crash the
            // harness. Logged at warn for observability.
            tracing::warn!(
                tool = %tool_name,
                agent = %self.agent_id,
                session = %self.session_id,
                error = %e,
                "tool signal capture failed",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AlephError;
    use crate::memory::store::raw_memory::{RawMemory, RawMemorySource};
    use crate::sync_primitives::Mutex;
    use async_trait::async_trait;

    /// Minimal in-memory store used for sink-write verification.
    struct MemStore {
        rows: Mutex<Vec<RawMemory>>,
    }

    impl MemStore {
        fn new() -> Self {
            Self {
                rows: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl RawMemoryStore for MemStore {
        async fn insert_raw_memory(&self, raw: &RawMemory) -> Result<(), AlephError> {
            self.rows.lock().unwrap().push(raw.clone());
            Ok(())
        }
        async fn get_unprocessed_raw_memories(
            &self,
            _agent_id: &str,
            _limit: usize,
        ) -> Result<Vec<RawMemory>, AlephError> {
            Ok(self.rows.lock().unwrap().clone())
        }
        async fn mark_raw_as_processed(&self, _ids: &[String]) -> Result<usize, AlephError> {
            Ok(0)
        }
        async fn count_unprocessed(&self, _agent_id: &str) -> Result<usize, AlephError> {
            Ok(self.rows.lock().unwrap().len())
        }
        async fn get_raw_by_path_prefix(
            &self,
            _path_prefix: &str,
            _agent_id: &str,
            _limit: usize,
        ) -> Result<Vec<RawMemory>, AlephError> {
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn raw_memory_sink_writes_one_row_per_call() {
        let store = Arc::new(MemStore::new());
        let sink = RawMemoryToolSink::new(store.clone(), "agent-1", "sess-x");

        sink.record_tool_call("read_file", true, 12, None).await;
        sink.record_tool_call("shell", false, 0, Some("perm denied"))
            .await;

        let rows = store.rows.lock().unwrap().clone();
        assert_eq!(rows.len(), 2);

        assert!(matches!(
            rows[0].source,
            RawMemorySource::ToolInvocation { ref tool_name, success: true, duration_ms: 12 }
                if tool_name == "read_file"
        ));
        assert!(matches!(
            rows[1].source,
            RawMemorySource::ToolInvocation { ref tool_name, success: false, duration_ms: 0 }
                if tool_name == "shell"
        ));
        assert!(rows[1].content.contains("perm denied"));
        assert_eq!(rows[0].agent_id, "agent-1");
        assert_eq!(rows[0].session_id.as_deref(), Some("sess-x"));
    }

    #[tokio::test]
    async fn noop_sink_does_nothing() {
        let sink = NoopToolSignalSink;
        // Just ensure it doesn't panic.
        sink.record_tool_call("x", true, 1, None).await;
    }
}
