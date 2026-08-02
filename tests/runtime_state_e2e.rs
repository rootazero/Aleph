//! End-to-end coverage for the orchestrator-side runtime-state wiring.
//!
//! Exercises the full producer chain that the predecessor cycle left empty:
//!   register tool + probe → `compute_runtime_state_blocks` reads the
//!   dispatcher's `ToolHealthCache` snapshot → `RuntimeStateFragment` for
//!   each cached-unhealthy entry → `ToolRuntimeStateLayer @502` renders
//!   `<tool_runtime_state>` XML through a real `PromptBuilder` pipeline.
//!
//! The gating half of the round trip — a dead probe *strips* the tool from
//! the schema — lives at the single enforcement point, in
//! `src/tools/scoped/tests.rs::{list_strips_unhealthy_tools,
//! metadata_schema_strips_unhealthy_tools_and_invalidates_on_flip}`. Together
//! with this file it locks the pair: a dead probe both vanishes from the
//! schema and *surfaces* a runtime-state hint.

use std::borrow::Cow;
use std::sync::Arc;

use alephcore::orchestrator::harness_bridge::compute_runtime_state_blocks;
use alephcore::thinker::context::{ContextAggregator, ResolvedContext};
use alephcore::thinker::interaction::{InteractionManifest, InteractionParadigm};
use alephcore::thinker::layers::ToolRuntimeStateLayer;
use alephcore::thinker::prompt_builder::PromptConfig;
use alephcore::thinker::prompt_layer::{LayerInput, PromptLayer};
use alephcore::thinker::security_context::SecurityContext;
use alephcore::tool_metadata::{HealthReason, ProbeResult, ToolCatalog, ToolHealthProbe};
use alephcore::tools::runtime_state::ToolStatus;

struct CannedDead(&'static str);

#[async_trait::async_trait]
impl ToolHealthProbe for CannedDead {
    async fn probe(&self) -> ProbeResult {
        ProbeResult::Unhealthy {
            reason: HealthReason::DependencyDown(Cow::Borrowed(self.0)),
            retry_after: None,
        }
    }
}

fn empty_context() -> ResolvedContext {
    ContextAggregator::resolve(
        &InteractionManifest::new(InteractionParadigm::Background),
        &SecurityContext::permissive(),
    )
}

#[tokio::test]
async fn unhealthy_probe_surfaces_in_runtime_state_layer_xml() {
    let registry = Arc::new(ToolCatalog::new());
    let cache = registry.health();
    cache.register_probe("alpha_tool", Arc::new(CannedDead("alpha down")));
    // Snapshots only surface cached entries; force a refresh to populate.
    let _ = cache.refresh("alpha_tool").await;

    // Producer: convert health cache snapshot → RuntimeStateFragments.
    let blocks = compute_runtime_state_blocks(Some(&registry));
    assert_eq!(blocks.len(), 1, "expected one fragment");
    assert_eq!(blocks[0].tool_name, "alpha_tool");
    match &blocks[0].status {
        ToolStatus::Unavailable { reason } => assert_eq!(reason, "alpha down"),
        ToolStatus::Available => panic!("expected Unavailable"),
    }

    // Consumer: hand the fragments to ResolvedContext + the real layer.
    let mut ctx = empty_context();
    ctx.runtime_state_blocks = blocks;

    let config = PromptConfig::default();
    let input = LayerInput::basic(&config, &[]).with_resolved_context_opt(Some(&ctx));
    let mut out = String::new();
    ToolRuntimeStateLayer.inject(&mut out, &input);

    assert!(out.starts_with("<tool_runtime_state>"));
    assert!(out.contains("<tool name=\"alpha_tool\""));
    assert!(out.contains("status=\"unavailable\""));
    assert!(out.contains("<hint>alpha down</hint>"));
    assert!(out.trim_end().ends_with("</tool_runtime_state>"));
}

#[tokio::test]
async fn no_tool_catalog_emits_empty_block() {
    let blocks = compute_runtime_state_blocks(None);
    assert!(blocks.is_empty());

    // Layer with empty `runtime_state_blocks` produces no output.
    let ctx = empty_context();
    let config = PromptConfig::default();
    let input = LayerInput::basic(&config, &[]).with_resolved_context_opt(Some(&ctx));
    let mut out = String::new();
    ToolRuntimeStateLayer.inject(&mut out, &input);
    assert!(out.is_empty());
}

#[tokio::test]
async fn healthy_probe_does_not_produce_fragment() {
    struct AlwaysHealthy;
    #[async_trait::async_trait]
    impl ToolHealthProbe for AlwaysHealthy {
        async fn probe(&self) -> ProbeResult {
            ProbeResult::Healthy
        }
    }

    let registry = Arc::new(ToolCatalog::new());
    let cache = registry.health();
    cache.register_probe("healthy_tool", Arc::new(AlwaysHealthy));
    let _ = cache.refresh("healthy_tool").await;

    let blocks = compute_runtime_state_blocks(Some(&registry));
    assert!(blocks.is_empty(), "healthy probes must not surface");
}
