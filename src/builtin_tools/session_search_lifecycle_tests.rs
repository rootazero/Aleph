use async_trait::async_trait;
use serde_json::json;

use super::{SessionSearchArgs, SessionSearchTool};
use crate::error::AlephError;
use crate::gateway::context::GatewayContext;
use crate::gateway::execution_engine::{ExecutionEngineConfig, SimpleExecutionEngine};
use crate::gateway::router::SessionKey;
use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
use crate::gateway::{AgentRegistry, AgentToAgentPolicy};
use crate::memory::assembler::{AssemblyBudget, MemoryEnvelope, WorkingMemoryAssembler};
use crate::memory::session_search_summary::FactSourceFilter;
use crate::sync_primitives::Arc;

struct Summary(MemoryEnvelope);

#[async_trait]
impl WorkingMemoryAssembler for Summary {
    async fn assemble(
        &self,
        _query: &str,
        _agent: &str,
        _session: Option<&str>,
        _budget: AssemblyBudget,
        _filter: FactSourceFilter,
    ) -> Result<MemoryEnvelope, AlephError> {
        Ok(self.0.clone())
    }
}

#[tokio::test]
async fn review_a2a_gate_precedes_actual_evidence_reads() {
    let root = tempfile::tempdir().unwrap();
    let sessions = Arc::new(
        SessionManager::new(SessionManagerConfig {
            db_path: root.path().join("sessions.db"),
            ..Default::default()
        })
        .unwrap(),
    );
    let key = SessionKey::main("private-agent");
    sessions.get_or_create(&key).await.unwrap();
    sessions
        .add_message(&key, "user", "evidenceatlas actual transcript")
        .await
        .unwrap();
    let envelope: MemoryEnvelope = serde_json::from_value(json!({
        "schema_version": "1.0", "generated_at": 0, "query": "evidenceatlas",
        "agent_id": "private-agent", "session_id": null,
        "slots": [{
            "kind": "session_recent", "tokens_used": 1, "tokens_budget": 100,
            "items": [{
                "id": "summary", "title": "An actual session", "content": "A retained summary",
                "source": {"kind": "summary", "layer": "d2", "session_id": key.to_key_string()},
                "relevance": 1.0, "tokens": 1, "updated_at": 0
            }]
        }],
        "meta": {"strategy": "fixture", "candidates_considered": 1, "used_fallback": false,
            "fallback_reason": null, "llm_rerank_latency_ms": null, "total_latency_ms": 0}
    }))
    .unwrap();
    for allowed in [true, false] {
        let context = Arc::new(GatewayContext::new(
            sessions.clone(),
            Arc::new(AgentRegistry::new()),
            Arc::new(SimpleExecutionEngine::new(ExecutionEngineConfig::default())),
            Arc::new(if allowed {
                AgentToAgentPolicy::permissive()
            } else {
                AgentToAgentPolicy::disabled()
            }),
        ));
        let tool = SessionSearchTool::new(
            context,
            "caller-agent",
            Some(Arc::new(Summary(envelope.clone()))),
            None,
        );
        let result = tool
            .call_impl(SessionSearchArgs {
                query: "evidenceatlas".into(),
                max_results: 5,
            })
            .await
            .unwrap();
        assert_eq!(result.total_hits, usize::from(allowed));
        if allowed {
            assert!(result.hits[0]
                .evidence_quotes
                .iter()
                .any(|quote| quote.contains("actual transcript")));
        }
        assert_eq!(
            tool.evidence_fetches
                .load(std::sync::atomic::Ordering::SeqCst),
            usize::from(allowed),
            "a denied agent must never reach raw evidence retrieval",
        );
    }
}
