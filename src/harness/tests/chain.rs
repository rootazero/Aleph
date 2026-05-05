//! Stage 4 — Subagent ChainContext Wiring tests (#11).
//!
//! Verifies that `AgentHarness::chain_context()` reflects the chain
//! injected via `HarnessDeps`, that the trait default is `None` for
//! non-`AgentHarness` impls, and that concurrent readers see a stable
//! immutable chain. End-to-end propagation through `subagent_spawner`
//! is covered by the existing spawner tests via `LoopRunResult.depth /
//! chain_id` — those continue to pass and now reflect the same chain
//! the inner harness sees.

use std::sync::Arc;

use crate::harness::agent::AgentHarness;
use crate::harness::chain_context::ChainContext;
use crate::harness::deps::HarnessDeps;
use crate::harness::trait_def::Harness;

mod stubs {
    use super::*;
    use crate::providers::adapter::{ProviderResponse, RequestPayload};
    use crate::providers::AiProvider;
    use crate::session::events::{ToolOutput, ToolOutputMetadata};
    use crate::session::in_process::InProcessActorSessionService;
    use crate::session::service::SessionService;
    use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
    use crate::tools::service::{ToolDefinition, ToolError, ToolService, ToolSource};
    use serde_json::json;
    use std::future::Future;
    use std::pin::Pin;

    pub(super) struct InertProvider;
    impl AiProvider for InertProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>>
        {
            Box::pin(async move { Ok(ProviderResponse::text_only("ok".into())) })
        }
        fn name(&self) -> &str {
            "inert"
        }
        fn color(&self) -> &str {
            "#000"
        }
    }

    pub(super) struct NoopTool;
    #[async_trait::async_trait]
    impl ToolService for NoopTool {
        async fn execute(
            &self,
            _name: &str,
            _input: serde_json::Value,
        ) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                value: json!({}),
                metadata: ToolOutputMetadata::default(),
            })
        }
        async fn list(&self) -> Vec<ToolDefinition> {
            vec![]
        }
        async fn describe(&self, _name: &str) -> Option<ToolDefinition> {
            None
        }
        fn dispatcher_schema(&self) -> Arc<[crate::dispatcher::ToolDefinition]> {
            Arc::from([])
        }
    }

    pub(super) fn fresh_session_service() -> Arc<dyn SessionService> {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        Arc::new(InProcessActorSessionService::new(store))
    }

    pub(super) fn make_deps_with_chain(chain: ChainContext) -> HarnessDeps {
        HarnessDeps {
            session: fresh_session_service(),
            tools: Arc::new(NoopTool),
            sandbox: Arc::new(crate::sandbox::NoopSandbox),
            llm: Arc::new(InertProvider),
            stop_hooks: None,
            context_budget: None,
            context_compactor: None,
            skill_prefetcher: None,
            trace_sink: None,
            system_prompt: None,
            prompt_builder: Arc::new(crate::harness::prompt::DefaultPromptBuilder),
            chain_context: chain,
            guardrails: None,
            fallback_llm: None,
            max_iterations: None,
            power: None,
            stall_config: None,
            consecutive_failure_cap: None,
            turn_timeout: None,
        }
    }
}

#[test]
fn root_harness_has_default_chain_at_depth_zero() {
    let harness = AgentHarness::new(stubs::make_deps_with_chain(ChainContext::default()));
    assert_eq!(harness.chain_context().depth, 0);
    assert!(harness.chain_context().is_root());
    assert!(!harness.chain_context().chain_id.is_empty());
}

#[test]
fn injected_chain_is_visible_via_accessor() {
    let root = ChainContext::new();
    let level1 = root.child().expect("depth 0 → 1");
    let level2 = level1.child().expect("depth 1 → 2");

    let harness = AgentHarness::new(stubs::make_deps_with_chain(level2.clone()));
    assert_eq!(harness.chain_context().depth, 2);
    assert_eq!(harness.chain_context().chain_id, root.chain_id);
}

#[test]
fn three_layer_chain_preserves_id_and_increments_depth() {
    let root = ChainContext::new();
    let l1 = root.child().expect("0→1");
    let l2 = l1.child().expect("1→2");
    let l3 = l2.child().expect("2→3");

    let h_root = AgentHarness::new(stubs::make_deps_with_chain(root.clone()));
    let h_l1 = AgentHarness::new(stubs::make_deps_with_chain(l1.clone()));
    let h_l2 = AgentHarness::new(stubs::make_deps_with_chain(l2.clone()));
    let h_l3 = AgentHarness::new(stubs::make_deps_with_chain(l3.clone()));

    // chain_id is invariant across all four levels.
    assert_eq!(
        h_root.chain_context().chain_id,
        h_l1.chain_context().chain_id
    );
    assert_eq!(h_l1.chain_context().chain_id, h_l2.chain_context().chain_id);
    assert_eq!(h_l2.chain_context().chain_id, h_l3.chain_context().chain_id);
    // Depth increments by exactly 1 per level.
    assert_eq!(h_root.chain_context().depth, 0);
    assert_eq!(h_l1.chain_context().depth, 1);
    assert_eq!(h_l2.chain_context().depth, 2);
    assert_eq!(h_l3.chain_context().depth, 3);
}

#[test]
fn trait_default_returns_none_for_non_overriding_impls() {
    // Synthetic Harness impl that does not override chain_context() must
    // continue to return None (preserves existing mock ergonomics).
    struct Bare;
    #[async_trait::async_trait]
    impl Harness for Bare {
        async fn run_turn(
            &self,
            _sid: &crate::session::service::SessionId,
            _cb: &mut dyn crate::harness::callback::HarnessCallback,
        ) -> Result<crate::harness::trait_def::TurnState, crate::harness::trait_def::HarnessError>
        {
            Ok(crate::harness::trait_def::TurnState::Done)
        }
    }
    let b = Bare;
    let h: &dyn Harness = &b;
    assert!(h.chain_context().is_none());
}

#[test]
fn agent_harness_trait_dispatch_returns_some_chain() {
    let root = ChainContext::new();
    let h = AgentHarness::new(stubs::make_deps_with_chain(root.clone()));
    let h_dyn: &dyn Harness = &h;
    let chain = h_dyn
        .chain_context()
        .expect("AgentHarness must report a chain");
    assert_eq!(chain.chain_id, root.chain_id);
    assert_eq!(chain.depth, 0);
}

/// Concurrent readers of `chain_context()` across `Arc<AgentHarness>` clones
/// must observe a stable, immutable chain. The accessor returns
/// `&self.deps.chain_context`; since `ChainContext` fields are read-only
/// after construction this is a smoke test rather than a UB hunt — but it
/// nails the contract that the seam is `Send + Sync` safe under load.
#[test]
fn concurrent_readers_see_stable_chain() {
    let root = ChainContext::new();
    let chain_id = root.chain_id.clone();
    let harness = Arc::new(AgentHarness::new(stubs::make_deps_with_chain(root)));

    let mut handles = Vec::new();
    for _ in 0..16 {
        let h = harness.clone();
        let expected = chain_id.clone();
        handles.push(std::thread::spawn(move || {
            for _ in 0..1_000 {
                assert_eq!(h.chain_context().chain_id, expected);
                assert_eq!(h.chain_context().depth, 0);
            }
        }));
    }
    for jh in handles {
        jh.join().expect("reader thread joined");
    }
}
