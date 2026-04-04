//! Sub-agent runner — executes a temporary AgentLoop for delegated tasks.
//!
//! Extracted from `subagent_tool.rs` so the runner can be reused
//! independently of the `SubagentTool` LoopTool wrapper.

use tokio_util::sync::CancellationToken;

use super::loop_core::{AgentLoop, LoopConfig, NoopCallback};
use super::prompt_builder::{PromptBuilder, PromptSection, Stability};
use super::provider_bridge::AiProviderBridge;
use super::safety::SafetyGuard;
use super::tool::LoopToolRegistry;
use crate::agents::AgentDef;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

/// Factory that builds a fresh LoopToolRegistry for the sub-agent.
///
/// The factory is responsible for providing the parent's tools minus
/// the "subagent" tool itself (to prevent infinite recursion).
/// Created at a higher layer where UnifiedTool/ToolRegistry are available.
pub type ToolRegistryFactory = Arc<dyn Fn() -> LoopToolRegistry + Send + Sync>;

/// Factory that builds a SafetyGuard for the sub-agent.
///
/// SafetyGuard is not Clone, so we use a factory to produce a fresh instance
/// each time a sub-agent is spawned.
pub type SafetyGuardFactory = Arc<dyn Fn() -> SafetyGuard + Send + Sync>;

/// Run a sub-agent to completion.
///
/// This is a module-level async function so it can be spawned in a
/// background tokio task (which requires `'static`).
pub async fn run_subagent(
    provider: Arc<dyn AiProvider>,
    agent_def: AgentDef,
    task: String,
    context_summary: Option<String>,
    model: Option<String>,
    tool_registry_factory: ToolRegistryFactory,
    safety_guard_factory: SafetyGuardFactory,
    child_chain: super::chain_context::ChainContext,
    timeout_secs: u64,
) -> Result<super::loop_core::LoopRunResult, String> {
    // Apply model override: explicit arg > agent_def.model_hint > default
    let resolved_model = model.or_else(|| agent_def.model_hint.clone());
    let bridge = if let Some(m) = resolved_model {
        AiProviderBridge::new(provider).with_model(m)
    } else {
        AiProviderBridge::new(provider)
    };

    // Build tool registry, then filter to agent's allowed tools
    let mut registry = (tool_registry_factory)();
    registry.retain(|name| agent_def.is_tool_allowed(name));

    // Build prompt for sub-agent via Section Registry.
    let mut prompt_builder = PromptBuilder::for_agent(&agent_def);

    // Inject parent context if provided
    if let Some(summary) = context_summary {
        prompt_builder.register(PromptSection {
            name: "parent_context".to_string(),
            stability: Stability::Dynamic,
            priority: 500,
            protected: false,
            content: format!("## Context from parent agent\n\n{}", summary),
        });
    }

    // Build loop config from agent definition
    let config = LoopConfig {
        max_iterations: agent_def.max_iterations.unwrap_or(25) as usize,
        token_budget: agent_def.token_budget.unwrap_or(100_000) as usize,
    };

    // Create and run the agent loop
    let mut agent_loop = AgentLoop::new(
        bridge,
        registry,
        prompt_builder,
        (safety_guard_factory)(),
        config,
        CancellationToken::new(),
    )
    .with_chain(child_chain);

    let mut callback = NoopCallback;
    let timeout_duration = std::time::Duration::from_secs(timeout_secs);
    let run_result =
        tokio::time::timeout(timeout_duration, agent_loop.run(&task, &mut callback)).await;

    match run_result {
        Err(_elapsed) => Err(format!("Sub-agent timed out after {}s", timeout_secs)),
        Ok(Ok(result)) => Ok(result),
        Ok(Err(e)) => Err(format!("sub-agent failed: {}", e)),
    }
}
