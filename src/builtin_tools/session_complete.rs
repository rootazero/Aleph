//! LLM-facing tool: mark a self-contained task as complete.
//! Triggers a retrospective extraction via the memory capture hook pipeline.

use crate::sync_primitives::Arc;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::memory::extensions::types::{CaptureCtx, CaptureDecision};
use crate::memory::extensions::{insert_with_capture_filter, MemoryExtensionRegistry};
use crate::memory::namespace::NamespaceScope;
use crate::memory::store::raw_memory::{
    RawMemory, RawMemorySource, RawMemoryStore, SessionEndReason,
};
use crate::memory::store::MemoryBackend;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output types
// =============================================================================

/// Arguments for the `session_complete` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionCompleteArgs {
    /// Brief summary of what the task accomplished.
    pub outcome: String,
    /// Optional pre-distilled learnings the LLM wants to preserve.
    #[serde(default)]
    pub key_learnings: Option<Vec<String>>,
}

/// Result returned to the LLM after calling `session_complete`.
#[derive(Debug, Clone, Serialize)]
pub struct SessionCompleteResult {
    pub ok: bool,
    pub message: String,
}

// =============================================================================
// Tool struct
// =============================================================================

/// LLM-callable tool that marks a self-contained task as complete.
///
/// Emits a `RawMemory(SessionEnd { reason: TaskDone })` row so that the
/// `CompressionService` can trigger a retrospective extraction.  Does NOT
/// end the conversation — it is a signal, not a teardown.
#[derive(Clone)]
pub struct SessionCompleteTool {
    db: MemoryBackend,
    agent_id: String,
    /// Injected at agent-loop startup via `set_session_id`.
    /// Wrapped in Arc<`RwLock`<…>> so it can be updated without re-building
    /// the registry (same pattern as `memory_workspace_handle`).
    session_id: Option<Arc<tokio::sync::RwLock<String>>>,
    /// Optional capture-filter registry (Spec 4 Task 6).
    /// When set, the `SessionEnd` row goes through `insert_with_capture_filter`.
    /// Task 11 wires the real registry at startup; `None` falls back to direct insert.
    capture_registry: Option<Arc<MemoryExtensionRegistry>>,
}

impl SessionCompleteTool {
    /// Create a new tool backed by the given memory store.
    pub fn new(db: MemoryBackend, agent_id: impl Into<String>) -> Self {
        Self {
            db,
            agent_id: agent_id.into(),
            session_id: None,
            capture_registry: None,
        }
    }

    /// Attach a shared session-id handle (written by the execution engine).
    pub fn with_session_handle(mut self, handle: Arc<tokio::sync::RwLock<String>>) -> Self {
        self.session_id = Some(handle);
        self
    }

    /// Attach a capture-filter registry (Spec 4 Task 6).
    ///
    /// When set, the `SessionEnd` raw-memory row goes through
    /// `insert_with_capture_filter` so extensions can mutate or block it.
    /// Task 11 wires the real registry at startup; `None` preserves current behaviour.
    pub fn with_capture_registry(mut self, registry: Arc<MemoryExtensionRegistry>) -> Self {
        self.capture_registry = Some(registry);
        self
    }

    /// Read the current session id (non-blocking best-effort).
    fn current_session_id(&self) -> Option<String> {
        self.session_id.as_ref().and_then(|h| {
            h.try_read()
                .ok()
                .map(|g| g.clone())
                .filter(|s| !s.is_empty())
        })
    }
}

// =============================================================================
// AlephTool impl
// =============================================================================

#[async_trait]
impl AlephTool for SessionCompleteTool {
    const NAME: &'static str = "session_complete";
    const DESCRIPTION: &'static str =
        "Call when you believe a self-contained task has just completed. \
         Triggers a memory retrospective so future similar tasks can benefit. \
         Does NOT end the conversation — you can keep talking after calling this.";

    type Args = SessionCompleteArgs;
    type Output = SessionCompleteResult;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let content = build_content(&args);
        let session_id = self.current_session_id();

        let mut raw = RawMemory::new(
            content,
            RawMemorySource::SessionEnd {
                reason: SessionEndReason::TaskDone,
            },
        )
        .with_agent(self.agent_id.clone());

        if let Some(sid) = &session_id {
            raw = raw.with_session(sid.clone());
        }

        if let Some(ref registry) = self.capture_registry {
            let store: Arc<dyn RawMemoryStore> = self.db.clone();
            let ctx = CaptureCtx {
                agent_id: self.agent_id.clone(),
                namespace: NamespaceScope::Owner,
                session_id: session_id.clone(),
                source_hint: "session_end".into(),
            };
            // BT-D-R4-03: a capture-filter `Block` is Ok(CaptureDecision::Block)
            // and means the extension refused to record this retrospective.
            // Returning ok: true here would lie to the agent (and the user)
            // that the completion was persisted. Surface the block as an
            // explicit non-OK result so the caller can react.
            match insert_with_capture_filter(&store, registry, &ctx, raw).await? {
                CaptureDecision::Allow => {}
                CaptureDecision::Block { reason } => {
                    return Ok(SessionCompleteResult {
                        ok: false,
                        message: format!(
                            "Task completion NOT recorded: capture-filter blocked this entry ({reason}). \
                             Retrospective extraction skipped for agent '{}'.",
                            self.agent_id
                        ),
                    });
                }
            }
        } else {
            self.db.insert_raw_memory(&raw).await?;
        }

        Ok(SessionCompleteResult {
            ok: true,
            message: format!(
                "Task completion recorded. Retrospective extraction queued for agent '{}'.",
                self.agent_id
            ),
        })
    }
}

// =============================================================================
// Helpers (pub(crate) for tests in other modules)
// =============================================================================

/// Build the content blob that goes into the raw memory row.
pub(crate) fn build_content(args: &SessionCompleteArgs) -> String {
    let mut s = format!("TASK_OUTCOME:\n{}\n", args.outcome);
    if let Some(learnings) = &args.key_learnings {
        if !learnings.is_empty() {
            s.push_str("\nKEY_LEARNINGS:\n");
            for l in learnings {
                s.push_str("- ");
                s.push_str(l);
                s.push('\n');
            }
        }
    }
    s
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_primitives::Mutex;
    use crate::AlephError;

    // Minimal fake that records every insert call.
    #[derive(Default)]
    struct FakeStore(Mutex<Vec<RawMemory>>);

    #[async_trait]
    impl RawMemoryStore for FakeStore {
        async fn insert_raw_memory(&self, raw: &RawMemory) -> std::result::Result<(), AlephError> {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(raw.clone());
            Ok(())
        }
        async fn get_unprocessed_raw_memories(
            &self,
            _agent_id: &str,
            _limit: usize,
        ) -> std::result::Result<Vec<RawMemory>, AlephError> {
            Ok(vec![])
        }
        async fn mark_raw_as_processed(
            &self,
            _ids: &[String],
        ) -> std::result::Result<usize, AlephError> {
            Ok(0)
        }
        async fn count_unprocessed(
            &self,
            _agent_id: &str,
        ) -> std::result::Result<usize, AlephError> {
            Ok(0)
        }
        async fn get_raw_by_path_prefix(
            &self,
            _path_prefix: &str,
            _agent_id: &str,
            _limit: usize,
        ) -> std::result::Result<Vec<RawMemory>, AlephError> {
            Ok(vec![])
        }
    }

    // ---------- pure-logic tests (no async, no store) ----------

    #[test]
    fn args_round_trip_json() {
        let a = SessionCompleteArgs {
            outcome: "built feature X".into(),
            key_learnings: Some(vec!["learning one".into()]),
        };
        let s = serde_json::to_string(&a).unwrap();
        let back: SessionCompleteArgs = serde_json::from_str(&s).unwrap();
        assert_eq!(back.outcome, "built feature X");
        assert_eq!(back.key_learnings.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn build_content_includes_outcome_and_learnings() {
        let args = SessionCompleteArgs {
            outcome: "shipped X".into(),
            key_learnings: Some(vec!["L1".into(), "L2".into()]),
        };
        let content = build_content(&args);
        assert!(content.contains("TASK_OUTCOME:"));
        assert!(content.contains("shipped X"));
        assert!(content.contains("KEY_LEARNINGS:"));
        assert!(content.contains("- L1"));
        assert!(content.contains("- L2"));
    }

    #[test]
    fn build_content_without_learnings_omits_section() {
        let args = SessionCompleteArgs {
            outcome: "ok".into(),
            key_learnings: None,
        };
        let content = build_content(&args);
        assert!(content.contains("TASK_OUTCOME:"));
        assert!(!content.contains("KEY_LEARNINGS:"));
    }

    // ---------- async store-integration test ----------

    /// Verify that calling the tool emits a correctly-shaped RawMemory row.
    /// We can't instantiate `SqliteMemoryBackend` in a unit test, so we bypass
    /// AlephTool and test `build_content` + `RawMemoryStore` directly.
    #[tokio::test]
    async fn emit_writes_session_end_task_done_row() {
        let fake = Arc::new(FakeStore::default());
        let args = SessionCompleteArgs {
            outcome: "built test".into(),
            key_learnings: Some(vec!["L1".into()]),
        };

        let content = build_content(&args);
        let raw = RawMemory::new(
            content,
            RawMemorySource::SessionEnd {
                reason: SessionEndReason::TaskDone,
            },
        )
        .with_agent("agent-1".to_string())
        .with_session("sess-1".to_string());

        fake.insert_raw_memory(&raw).await.unwrap();

        let captured = fake.0.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(captured.len(), 1);
        assert!(matches!(
            captured[0].source,
            RawMemorySource::SessionEnd {
                reason: SessionEndReason::TaskDone
            }
        ));
        assert_eq!(captured[0].agent_id, "agent-1");
        assert_eq!(captured[0].session_id.as_deref(), Some("sess-1"));
        assert!(captured[0].content.contains("built test"));
    }
}
