//! LLM-facing tool: synthesise a coherent answer from the memory store.
//!
//! Thin wrapper around `MemoryReflector::reflect`. The `Arc<MemoryReflector>`
//! is injected at server startup (Task 8); until then the tool returns a clear
//! error rather than panicking.

use crate::error::{AlephError, Result};
use crate::memory::namespace::NamespaceScope;
use crate::memory::notes::query_filer::QueryFiler;
use crate::memory::reflector::{MemoryReflector, ReflectOpts, Synthesis};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// =============================================================================
// Args / Output types
// =============================================================================

/// Arguments for the `memory_reflect` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryReflectArgs {
    /// Natural-language question to synthesise an answer for from memory.
    pub query: String,
}

/// Result returned to the LLM after calling `memory_reflect`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryReflectResult {
    pub synthesis: Synthesis,
}

// =============================================================================
// Tool struct
// =============================================================================

/// LLM-callable tool that synthesises a coherent answer from the memory store.
///
/// Delegates to `MemoryReflector::reflect`, which retrieves relevant notes via
/// the working-memory assembler and condenses them with the LLM.  Unlike
/// `memory_search`, which returns raw hits, this tool returns a distilled
/// natural-language answer plus cited note paths.
#[derive(Clone)]
pub struct MemoryReflectTool {
    /// Injected by Task 8 at server startup. `None` → tool returns an error.
    reflector: Option<Arc<MemoryReflector>>,
    agent_id: String,
    /// Injected at agent-loop startup via `set_session_id`.
    session_id: Option<Arc<tokio::sync::RwLock<String>>>,
    /// Optional fire-and-forget query filer hook (Task 7).
    pub query_filer: Option<Arc<dyn QueryFiler>>,
}

impl MemoryReflectTool {
    /// Create a placeholder tool (reflector not yet wired).
    /// Task 8 calls `with_reflector` after constructing the reflector.
    pub fn new(agent_id: impl Into<String>) -> Self {
        Self {
            reflector: None,
            agent_id: agent_id.into(),
            session_id: None,
            query_filer: None,
        }
    }

    /// Attach the reflector instance (called by Task 8 server builder).
    #[must_use]
    pub fn with_reflector(mut self, reflector: Arc<MemoryReflector>) -> Self {
        self.reflector = Some(reflector);
        self
    }

    /// Attach a query filer for fire-and-forget filing after successful synthesis.
    pub fn with_query_filer(mut self, qf: Arc<dyn QueryFiler>) -> Self {
        self.query_filer = Some(qf);
        self
    }

    /// Attach a shared session-id handle (written by the execution engine).
    pub fn with_session_handle(mut self, handle: Arc<tokio::sync::RwLock<String>>) -> Self {
        self.session_id = Some(handle);
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

    /// Called by the dispatch chokepoint (`BuiltinToolRegistry::execute_tool`,
    /// `"memory_reflect"` arm) with the caller's already-composed memory
    /// PARTITION (`BuiltinToolRegistry::caller_memory_partition`, e.g.
    /// `main__u-alice`), so the fire-and-forget query note this call files
    /// lands where the reader's own memories live, not in the shared org
    /// partition `main`.
    ///
    /// Composed by the CALLER — before this method ever runs, and therefore
    /// before the `tokio::spawn` inside [`Self::reflect_and_file`] — because
    /// `project_scope::session_write_id` reads the ambient scope off a
    /// task-local (`scope::current_scope`) that a spawned task cannot see.
    /// `ReflectOpts.agent_id` still resolves from `acting_agent_id` (the BASE
    /// persona) inside `reflect_and_file`, unaffected by this override:
    /// composing it a second time would make the read side (`gather.rs`)
    /// compose AGAIN into a `main__u-x__u-x` ghost partition.
    pub async fn call_with_filed_partition(
        &self,
        filed_agent_id: String,
        args: MemoryReflectArgs,
    ) -> Result<MemoryReflectResult> {
        self.reflect_and_file(args, Some(filed_agent_id)).await
    }

    /// Shared body for [`AlephTool::call`] and [`Self::call_with_filed_partition`].
    ///
    /// `filed_agent_id_override`, when `Some`, is the partition the query
    /// filer's write goes to; `None` (every caller outside the dispatch
    /// chokepoint — direct construction, tests, headless tooling) falls back
    /// to the base persona `actor` resolves to, byte-identical to this tool's
    /// pre-fix behaviour.
    async fn reflect_and_file(
        &self,
        args: MemoryReflectArgs,
        filed_agent_id_override: Option<String>,
    ) -> Result<MemoryReflectResult> {
        let reflector = self.reflector.as_ref().ok_or_else(|| {
            AlephError::other(
                "memory_reflect tool: MemoryReflector not wired (server builder needs to inject it)",
            )
        })?;

        let session_id = self.current_session_id();

        // PR-3 / BT-D-R4-04: read the actor from the turn context
        // rather than the construction-time self.agent_id. Production
        // hardcodes self.agent_id = "main", so without this every
        // non-main agent's reflection was synthesised from main's
        // owner namespace and filed under main. acting_agent_id()
        // reads the turn context if the dispatcher set TURN_CONTEXT;
        // otherwise it falls back to self.agent_id so older call sites
        // (tests, headless tooling) keep working.
        let actor = crate::builtin_tools::acting_agent::acting_agent_id(&self.agent_id);

        let opts = ReflectOpts {
            agent_id: actor.clone(),
            namespace: NamespaceScope::Owner,
            max_tokens: None,
            session_id: session_id.clone(),
        };

        let synthesis = reflector.reflect(&args.query, opts).await?;

        // Fire-and-forget: file the query without blocking the reflect return path.
        if let Some(qf) = self.query_filer.clone() {
            let filed_agent_id = filed_agent_id_override.unwrap_or_else(|| actor.clone());
            let q = args.query.clone();
            let synth = synthesis.clone();
            let sid = session_id.clone();
            tokio::spawn(async move {
                if let Err(e) = qf.maybe_file(&filed_agent_id, &q, &synth, sid.as_deref()).await {
                    tracing::warn!("query filer failed: {e}");
                }
            });
        }

        Ok(MemoryReflectResult { synthesis })
    }
}

// =============================================================================
// AlephTool impl
// =============================================================================

#[async_trait]
impl AlephTool for MemoryReflectTool {
    const NAME: &'static str = "memory_reflect";
    const DESCRIPTION: &'static str = "Synthesise a coherent answer from your long-term memory. \
         Use this when you want a distilled response (vs memory_search, which returns raw hits). \
         Returns answer text + cited note paths.";

    type Args = MemoryReflectArgs;
    type Output = MemoryReflectResult;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.reflect_and_file(args, None).await
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_round_trip_json() {
        let a = MemoryReflectArgs {
            query: "What do I know about Rust?".into(),
        };
        let j = serde_json::to_string(&a).unwrap();
        let back: MemoryReflectArgs = serde_json::from_str(&j).unwrap();
        assert_eq!(back.query, a.query);
    }

    #[test]
    fn tool_description_mentions_synthesis() {
        assert!(MemoryReflectTool::DESCRIPTION
            .to_lowercase()
            .contains("synthesi"));
        assert!(MemoryReflectTool::DESCRIPTION.contains("memory_search"));
    }

    #[test]
    fn tool_name_is_memory_reflect() {
        assert_eq!(MemoryReflectTool::NAME, "memory_reflect");
    }
}

/// The query filer must write to the partition the reflection was read from
/// (T09). Uses a `CapturingAssembler` that records every `agent_id` it is
/// asked to read with and always returns an empty envelope, so `reflect()`
/// short-circuits before any LLM call is needed — the assertions here are
/// about WHICH partition each side of the call used, not about synthesis
/// content.
#[cfg(test)]
mod partition_tests {
    use super::*;
    use crate::memory::assembler::envelope::{EnvelopeMeta, MemoryEnvelope};
    use crate::memory::assembler::{AssemblyBudget, WorkingMemoryAssembler};
    use crate::memory::notes::query_filer::{CheapGateReason, FileOutcome};
    use crate::memory::reflector::fs_reflector::RecallWriter;
    use crate::memory::session_search_summary::FactSourceFilter;
    use crate::providers::recording_mock::RecordingMockProvider;
    use async_trait::async_trait;
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::sync::mpsc;

    /// Records every `agent_id` the assembler was asked to read with — the
    /// READ side of the call. Always returns an empty envelope so `reflect`
    /// takes the empty-packet short-circuit and never calls an LLM.
    struct CapturingAssembler {
        seen: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl WorkingMemoryAssembler for CapturingAssembler {
        async fn assemble(
            &self,
            query: &str,
            agent_id: &str,
            _session_id: Option<&str>,
            _budget: AssemblyBudget,
            _filter: FactSourceFilter,
        ) -> Result<MemoryEnvelope> {
            self.seen.lock().unwrap().push(agent_id.to_string());
            Ok(MemoryEnvelope {
                schema_version: "1.0".into(),
                generated_at: 0,
                query: query.to_string(),
                agent_id: agent_id.to_string(),
                session_id: None,
                slots: vec![],
                meta: EnvelopeMeta {
                    strategy: "test_empty".into(),
                    candidates_considered: 0,
                    used_fallback: false,
                    fallback_reason: None,
                    llm_rerank_latency_ms: None,
                    total_latency_ms: 0,
                },
            })
        }
    }

    /// Records every `agent_id` handed to `maybe_file` — the WRITE side of
    /// the call — and signals over an mpsc channel so the test can wait
    /// deterministically for the fire-and-forget `tokio::spawn` to run,
    /// rather than sleeping.
    struct RecordingFiler {
        tx: mpsc::UnboundedSender<String>,
    }

    #[async_trait]
    impl crate::memory::notes::query_filer::QueryFiler for RecordingFiler {
        async fn maybe_file(
            &self,
            agent_id: &str,
            _query: &str,
            _synthesis: &Synthesis,
            _session_id: Option<&str>,
        ) -> Result<FileOutcome> {
            let _ = self.tx.send(agent_id.to_string());
            Ok(FileOutcome::SkippedCheapGate {
                reason: CheapGateReason::TooFewSources { count: 0 },
            })
        }
    }

    fn make_tool(
        seen_reads: Arc<Mutex<Vec<String>>>,
    ) -> (MemoryReflectTool, mpsc::UnboundedReceiver<String>) {
        let assembler: Arc<dyn WorkingMemoryAssembler> =
            Arc::new(CapturingAssembler { seen: seen_reads });
        let provider: Arc<dyn crate::providers::AiProvider> =
            Arc::new(RecordingMockProvider::new("unused".into()));
        let recall_writer: RecallWriter = Arc::new(|_row| Box::pin(async { Ok(()) }));
        let reflector = Arc::new(MemoryReflector::new(assembler, provider, recall_writer));

        let (tx, rx) = mpsc::unbounded_channel();
        let filer: Arc<dyn crate::memory::notes::query_filer::QueryFiler> =
            Arc::new(RecordingFiler { tx });

        let tool = MemoryReflectTool::new("main")
            .with_reflector(reflector)
            .with_query_filer(filer);
        (tool, rx)
    }

    async fn recv_filed_id(rx: &mut mpsc::UnboundedReceiver<String>) -> String {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("query filer was never invoked (timed out waiting on the spawned task)")
            .expect("query filer channel closed without a value")
    }

    #[tokio::test]
    async fn dispatch_chokepoint_override_files_to_the_composed_partition() {
        let seen_reads = Arc::new(Mutex::new(Vec::new()));
        let (tool, mut rx) = make_tool(seen_reads);

        let args = MemoryReflectArgs {
            query: "what do I know about rust?".into(),
        };
        tool.call_with_filed_partition("main__u-alice".to_string(), args)
            .await
            .expect("call should succeed");

        let filed_id = recv_filed_id(&mut rx).await;
        assert_eq!(
            filed_id, "main__u-alice",
            "the query note must land in the reader's own partition, not the org partition `main`"
        );
    }

    #[tokio::test]
    async fn the_override_is_not_composed_a_second_time_into_the_read_side() {
        let seen_reads = Arc::new(Mutex::new(Vec::new()));
        let (tool, mut rx) = make_tool(seen_reads.clone());

        let args = MemoryReflectArgs { query: "q".into() };
        tool.call_with_filed_partition("main__u-alice".to_string(), args)
            .await
            .expect("call should succeed");

        // Drain the write side too so the spawned task has definitely run
        // before this test asserts and returns (not part of the assertion).
        let _ = recv_filed_id(&mut rx).await;

        let reads = seen_reads.lock().unwrap();
        assert_eq!(
            reads.as_slice(),
            ["main"],
            "ReflectOpts.agent_id must stay the BASE persona id the read side composes \
             itself; feeding it the already-composed override would make gather.rs compose \
             a SECOND time into a main__u-alice__u-alice ghost partition"
        );
    }

    #[tokio::test]
    async fn no_override_files_to_the_bare_base_persona_unchanged() {
        let seen_reads = Arc::new(Mutex::new(Vec::new()));
        let (tool, mut rx) = make_tool(seen_reads);

        // AlephTool::call (no dispatch-chokepoint override) — every caller
        // outside the gateway/channel-run dispatch arm: direct construction,
        // tests, headless tooling.
        let args = MemoryReflectArgs { query: "q".into() };
        crate::tools::AlephTool::call(&tool, args)
            .await
            .expect("call should succeed");

        let filed_id = recv_filed_id(&mut rx).await;
        assert_eq!(
            filed_id, "main",
            "with no composed override the write must stay byte-identical to \
             pre-fix behaviour"
        );
    }
}
