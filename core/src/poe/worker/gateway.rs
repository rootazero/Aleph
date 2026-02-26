//! GatewayAgentLoopWorker - Type alias and factory for Gateway integration.

use std::path::PathBuf;

use super::agent_loop_worker::AgentLoopWorker;

// ============================================================================
// GatewayAgentLoopWorker - Type Alias for Gateway Integration
// ============================================================================

/// Type alias for the concrete AgentLoopWorker used in Gateway.
///
/// This provides a specific instantiation of AgentLoopWorker with:
/// - `Thinker<SingleProviderRegistry>` for LLM decisions
/// - `SingleStepExecutor<BuiltinToolRegistry>` for tool execution
/// - `NoOpCompressor` for context management (compression disabled for POE)
///
/// # Example
///
/// ```rust,ignore
/// use alephcore::poe::{GatewayAgentLoopWorker, create_gateway_worker};
/// use std::sync::Arc;
///
/// let provider = create_claude_provider_from_env()?;
/// let worker = create_gateway_worker(Arc::new(provider), PathBuf::from("/tmp/workspace"));
/// ```
pub type GatewayAgentLoopWorker = AgentLoopWorker<
    crate::thinker::Thinker<crate::thinker::SingleProviderRegistry>,
    crate::executor::SingleStepExecutor<crate::executor::BuiltinToolRegistry>,
    crate::NoOpCompressor,
>;

/// Create a GatewayAgentLoopWorker with the specified provider and workspace.
///
/// This factory function constructs all the necessary components for a POE worker:
/// - Thinker with SingleProviderRegistry for LLM calls
/// - SingleStepExecutor with BuiltinToolRegistry for tool execution
/// - NoOpCompressor (POE manages its own execution cycles)
/// - Builtin tools converted to UnifiedTool format
///
/// # Arguments
///
/// * `provider` - The AI provider for LLM calls (e.g., Claude)
/// * `workspace` - The workspace directory for file operations
///
/// # Example
///
/// ```rust,ignore
/// use alephcore::poe::create_gateway_worker;
/// use alephcore::gateway::create_claude_provider_from_env;
/// use std::sync::Arc;
/// use std::path::PathBuf;
///
/// let provider = create_claude_provider_from_env()?;
/// let worker = create_gateway_worker(
///     Arc::new(provider),
///     PathBuf::from("/tmp/poe-workspace"),
/// );
/// ```
pub fn create_gateway_worker(
    provider: std::sync::Arc<dyn crate::providers::AiProvider>,
    workspace: PathBuf,
) -> GatewayAgentLoopWorker {
    use crate::agent_loop::LoopConfig;
    use crate::dispatcher::{ToolSource, UnifiedTool};
    use crate::executor::{BuiltinToolRegistry, SingleStepExecutor, BUILTIN_TOOL_DEFINITIONS};
    use crate::thinker::{SingleProviderRegistry, Thinker, ThinkerConfig};
    use crate::NoOpCompressor;

    // Create Thinker with single provider registry
    let registry = std::sync::Arc::new(SingleProviderRegistry::new(provider));
    let thinker = std::sync::Arc::new(Thinker::new(registry, ThinkerConfig::default()));

    // Create Executor with builtin tool registry + ExecSecurityGate
    let tool_registry = std::sync::Arc::new(BuiltinToolRegistry::new());

    // Initialize ExecApprovalManager for human-in-the-loop shell approval
    let approval_manager = std::sync::Arc::new(crate::exec::ExecApprovalManager::new());

    // Initialize platform-specific SandboxManager (macOS only)
    #[cfg(target_os = "macos")]
    let sandbox_manager = {
        use crate::exec::sandbox::{FallbackPolicy, SandboxManager};
        use crate::exec::sandbox::platforms::MacOSSandbox;
        Some(std::sync::Arc::new(
            SandboxManager::new(std::sync::Arc::new(MacOSSandbox::new()))
                .with_fallback_policy(FallbackPolicy::WarnAndExecute),
        ))
    };
    #[cfg(not(target_os = "macos"))]
    let sandbox_manager: Option<std::sync::Arc<crate::exec::sandbox::SandboxManager>> = None;

    // Create ExecSecurityGate: risk assessment + approval + sandbox + secret masking
    let exec_gate = std::sync::Arc::new(
        crate::executor::ExecSecurityGate::new(approval_manager, sandbox_manager),
    );

    let executor = std::sync::Arc::new(
        SingleStepExecutor::new(tool_registry)
            .with_exec_security_gate(exec_gate),
    );

    // Build tools list from builtin definitions
    let tools: Vec<UnifiedTool> = BUILTIN_TOOL_DEFINITIONS
        .iter()
        .map(|def| {
            UnifiedTool::new(
                format!("builtin:{}", def.name),
                def.name,
                def.description,
                ToolSource::Builtin,
            )
        })
        .collect();

    // Create the worker
    AgentLoopWorker::new(
        workspace,
        thinker,
        executor,
        std::sync::Arc::new(NoOpCompressor),
        tools,
        LoopConfig::default(),
    )
}
