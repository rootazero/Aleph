//! `ToolSignalSink` — per-invocation tool metric capture.
//!
//! Spec 3 wiring: every tool call completion (success or failure) flows
//! through a `ToolSignalSink` so the Dream cycle's `compute_raw_metrics()`
//! can read cross-session statistics from `raw_memories`.
//!
//! The sink is per-session: `agent_id` and `session_id` are captured at
//! construction time so the harness side just calls `record(...)` without
//! threading identifiers through.
//!
//! Default impl is `NoopToolSignalSink` — zero overhead when the production
//! wiring path is not in place (tests, simple-engine fixtures).

use crate::sync_primitives::Arc;

use async_trait::async_trait;

use crate::error::AlephError;
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

/// Aggregate tool-invocation statistics for an agent over the last
/// `since_seconds` window. Returns `(total, succeeded, failed, avg_duration_ms)`.
///
/// Reads `RawMemorySource::ToolInvocation` rows. Reuses the in-memory
/// filtering path that the default `RawMemoryStore::get_raw_by_source`
/// provides — backends with a smarter SQL query override it.
pub async fn aggregate_tool_stats(
    store: &dyn RawMemoryStore,
    agent_id: &str,
    since_unix_secs: i64,
    limit: usize,
) -> Result<ToolStats, AlephError> {
    let rows = store
        .get_raw_by_source(
            // Probe variant; only the discriminator token matters for filtering.
            RawMemorySource::ToolInvocation {
                tool_name: String::new(),
                success: false,
                duration_ms: 0,
            },
            agent_id,
            limit,
        )
        .await?;

    let mut total: u64 = 0;
    let mut succeeded: u64 = 0;
    let mut failed: u64 = 0;
    let mut total_duration_ms: u64 = 0;

    for r in rows {
        if r.created_at < since_unix_secs {
            continue;
        }
        if let RawMemorySource::ToolInvocation {
            success,
            duration_ms,
            ..
        } = &r.source
        {
            total = total.saturating_add(1);
            if *success {
                succeeded = succeeded.saturating_add(1);
            } else {
                failed = failed.saturating_add(1);
            }
            total_duration_ms = total_duration_ms.saturating_add(*duration_ms);
        }
    }

    let avg_duration_ms = total_duration_ms.checked_div(total).unwrap_or(0);
    Ok(ToolStats {
        total,
        succeeded,
        failed,
        avg_duration_ms,
    })
}

/// Aggregated tool-invocation metrics over a time window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolStats {
    pub total: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub avg_duration_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
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
    async fn aggregate_tool_stats_buckets_success_and_failure() {
        let store = Arc::new(MemStore::new());
        let sink = RawMemoryToolSink::new(store.clone(), "agent-1", "sess");

        sink.record_tool_call("a", true, 10, None).await;
        sink.record_tool_call("b", true, 30, None).await;
        sink.record_tool_call("c", false, 0, Some("oops")).await;

        let stats = aggregate_tool_stats(store.as_ref(), "agent-1", 0, 100)
            .await
            .unwrap();
        assert_eq!(stats.total, 3);
        assert_eq!(stats.succeeded, 2);
        assert_eq!(stats.failed, 1);
        // Two succeeded (10 + 30) plus one zero-duration failure → 40/3 = 13.
        assert_eq!(stats.avg_duration_ms, (10 + 30) / 3);
    }

    #[tokio::test]
    async fn aggregate_tool_stats_respects_since_window() {
        let store = Arc::new(MemStore::new());
        // Write one stale + one fresh row by hand to control timestamps.
        let stale = RawMemory {
            id: "old".into(),
            content: "stale".into(),
            source: RawMemorySource::ToolInvocation {
                tool_name: "old".into(),
                success: true,
                duration_ms: 5,
            },
            agent_id: "agent-1".into(),
            session_id: None,
            path: None,
            layer: None,
            attachment_text: None,
            is_processed: false,
            created_at: 1_000,
        };
        let fresh = RawMemory {
            id: "new".into(),
            content: "fresh".into(),
            source: RawMemorySource::ToolInvocation {
                tool_name: "new".into(),
                success: true,
                duration_ms: 25,
            },
            agent_id: "agent-1".into(),
            session_id: None,
            path: None,
            layer: None,
            attachment_text: None,
            is_processed: false,
            created_at: 9_999_999,
        };
        store.insert_raw_memory(&stale).await.unwrap();
        store.insert_raw_memory(&fresh).await.unwrap();

        let stats = aggregate_tool_stats(store.as_ref(), "agent-1", 5_000, 100)
            .await
            .unwrap();
        assert_eq!(stats.total, 1, "only the fresh row should count");
        assert_eq!(stats.avg_duration_ms, 25);
    }

    #[tokio::test]
    async fn noop_sink_does_nothing() {
        let sink = NoopToolSignalSink;
        // Just ensure it doesn't panic.
        sink.record_tool_call("x", true, 1, None).await;
    }
}
