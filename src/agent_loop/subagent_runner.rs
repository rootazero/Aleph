//! Subagent runner — creates and runs an AgentLoop for sub-agent execution.
//!
//! This module is the only place outside `loop_core.rs` that calls
//! `AgentLoop::new`, keeping the invariant enforced by check-phase6b-flip-exit.sh
//! that `AgentLoop::new` is confined to `src/agent_loop/`.
//!
//! Called by `agents::runtime::AgentRuntime::execute_fresh_path`.

use tokio_util::sync::CancellationToken;

use super::chain_context::ChainContext;
use super::loop_core::{AgentLoop, LoopConfig, LoopRunResult, NoopCallback};
use super::provider_bridge::AiProviderBridge;
use crate::agents::AgentDef;
use crate::providers::AiProvider;
use crate::session::ingress_safety::SafetyGuard;
use crate::sync_primitives::Arc;
use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};
use crate::tools::runtime::LoopToolRegistry;

/// Configuration for a fresh-path sub-agent run.
pub struct SubagentRunConfig<'a> {
    pub provider: &'a Arc<dyn AiProvider>,
    pub registry: LoopToolRegistry,
    pub safety_guard: SafetyGuard,
    pub agent_def: &'a AgentDef,
    pub model: Option<String>,
    pub task: &'a str,
    pub context_summary: Option<&'a str>,
    pub timeout_secs: u64,
    pub child_chain: ChainContext,
    pub cancel_token: CancellationToken,
}

/// Build and run an AgentLoop for a sub-agent task.
///
/// This is the only call site of `AgentLoop::new` outside `loop_core.rs`,
/// kept inside `src/agent_loop/` to satisfy the phase6b exit-gate constraint.
pub async fn run_subagent(config: SubagentRunConfig<'_>) -> Result<LoopRunResult, String> {
    let resolved_model = config
        .model
        .clone()
        .or_else(|| config.agent_def.model_hint.clone());
    let bridge = if let Some(m) = resolved_model {
        AiProviderBridge::new(config.provider.clone()).with_model(m)
    } else {
        AiProviderBridge::new(config.provider.clone())
    };

    let loop_config = LoopConfig {
        max_iterations: config.agent_def.max_iterations.unwrap_or(25) as usize,
        token_budget: config.agent_def.token_budget.unwrap_or(100_000) as usize,
    };

    let prompt_builder =
        PromptBuilder::new(PromptConfig::default()).with_agent(config.agent_def.clone());

    let mut agent_loop = AgentLoop::new(
        bridge,
        config.registry,
        prompt_builder,
        config.safety_guard,
        loop_config,
        config.cancel_token,
    )
    .with_chain(config.child_chain);

    let effective_task = match config.context_summary {
        Some(summary) => format!(
            "## Context from parent agent\n\n{}\n\n---\n\n{}",
            summary, config.task
        ),
        None => config.task.to_string(),
    };

    let mut callback = NoopCallback;
    let timeout_duration = std::time::Duration::from_secs(config.timeout_secs);
    let run_result =
        tokio::time::timeout(timeout_duration, agent_loop.run(&effective_task, &mut callback))
            .await;

    match run_result {
        Err(_elapsed) => Err(format!(
            "Sub-agent timed out after {}s",
            config.timeout_secs
        )),
        Ok(Ok(result)) => Ok(result),
        Ok(Err(e)) => Err(format!("sub-agent failed: {}", e)),
    }
}
