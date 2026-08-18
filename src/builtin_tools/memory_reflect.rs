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
            let agent = actor.clone();
            let q = args.query.clone();
            let synth = synthesis.clone();
            let sid = session_id.clone();
            tokio::spawn(async move {
                if let Err(e) = qf.maybe_file(&agent, &q, &synth, sid.as_deref()).await {
                    tracing::warn!("query filer failed: {e}");
                }
            });
        }

        Ok(MemoryReflectResult { synthesis })
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
