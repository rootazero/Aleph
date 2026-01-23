//! Agent loop execution for tasks requiring LLM thinking

use crate::agent_loop::{
    AgentLoop, LoopConfig, LoopResult, RequestContext as AgentRequestContext,
};
use crate::agents::RigAgentConfig;
use crate::compressor::NoOpCompressor;
use crate::config::GenerationConfig;
use crate::core::MediaAttachment;
use crate::dispatcher::{ToolSource, UnifiedTool};
use crate::executor::{BuiltinToolConfig, BuiltinToolRegistry, SingleStepExecutor};
use crate::ffi::prompt_helpers::{
    build_history_summary_from_conversations, extract_attachment_text,
    format_generation_models_for_prompt,
};
use crate::ffi::provider_factory::create_provider_from_config;
use crate::ffi::tool_discovery::get_builtin_tool_descriptions;
use crate::ffi::FfiLoopCallback;
use crate::ffi::AetherEventHandler;
use crate::generation::GenerationProviderRegistry;
use crate::intent::ThinkingContext;
use crate::rig_tools::file_ops::{clear_written_files, take_written_files};
use crate::runtimes::{RuntimeCapability, RuntimeRegistry};
use crate::thinker::{SingleProviderRegistry, Thinker, ThinkerConfig};
use rig::completion::Message;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::memory::store_memory_after_response;

/// Run the Agent Loop for tasks requiring LLM thinking
#[allow(clippy::too_many_arguments)]
pub fn run_agent_loop(
    runtime: &tokio::runtime::Handle,
    input: &str,
    ctx: ThinkingContext,
    config: &RigAgentConfig,
    _tool_server_handle: rig::tool::server::ToolServerHandle,
    _registered_tools: Arc<RwLock<Vec<String>>>,
    op_token: &CancellationToken,
    handler: &Arc<dyn AetherEventHandler>,
    memory_config: &crate::config::MemoryConfig,
    memory_path: &Option<String>,
    input_for_memory: &str,
    app_context: &Option<String>,
    window_title: &Option<String>,
    generation_config: &GenerationConfig,
    attachments: Option<&[MediaAttachment]>,
    conversation_histories: &Arc<RwLock<HashMap<String, Vec<Message>>>>,
    topic_id: &Option<String>,
    generation_registry: &Arc<RwLock<GenerationProviderRegistry>>,
) {
    // Check if already cancelled
    if op_token.is_cancelled() {
        handler.on_error("Operation cancelled".to_string());
        return;
    }

    // Clear any previously tracked written files for this session
    clear_written_files();

    // Build input with attachment content if present
    let full_input = if let Some(attachment_text) = extract_attachment_text(attachments) {
        info!(
            attachment_len = attachment_text.len(),
            "Including attachment text in agent loop context"
        );
        format!("{}\n\n{}", input, attachment_text)
    } else {
        input.to_string()
    };

    // Get available tools for the loop
    let tool_descriptions = get_builtin_tool_descriptions(generation_config);
    let tools: Vec<UnifiedTool> = tool_descriptions
        .iter()
        .map(|td| {
            let mut tool = UnifiedTool::new(
                &format!("builtin:{}", td.name),
                &td.name,
                &td.description,
                ToolSource::Native,
            );
            if let Some(ref schema) = td.parameters_schema {
                tool = tool.with_parameters_schema(schema.clone());
            }
            tool
        })
        .collect();

    // Create the AI provider
    let provider = match create_provider_from_config(config) {
        Ok(p) => p,
        Err(e) => {
            error!(error = %e, "Failed to create provider");
            handler.on_error(format!("Provider error: {}", e));
            return;
        }
    };

    // Build request context
    let mut request_context = AgentRequestContext::empty();
    request_context.current_app = app_context.clone();
    request_context.window_title = window_title.clone();

    // Store category hint in metadata if present
    if let Some(ref hint) = ctx.category_hint {
        request_context
            .metadata
            .insert("category_hint".to_string(), format!("{:?}", hint));
    }

    // Create loop config
    let loop_config = LoopConfig::default()
        .with_max_steps(20)
        .with_max_tokens(100_000);

    // Build initial history from conversation histories (for cross-session context)
    let initial_history = build_history_summary_from_conversations(
        conversation_histories,
        topic_id,
        2000, // Max 2000 characters for history summary
    );

    // Get runtime capabilities for system prompt injection
    let runtime_capabilities = match RuntimeRegistry::new() {
        Ok(registry) => {
            let capabilities = RuntimeCapability::get_installed_from_registry(&registry);
            if capabilities.is_empty() {
                None
            } else {
                Some(RuntimeCapability::format_for_prompt(&capabilities))
            }
        }
        Err(e) => {
            debug!(error = %e, "Failed to get runtime capabilities (non-blocking)");
            None
        }
    };

    // Create thinker config with runtime capabilities and generation models
    let mut thinker_config = ThinkerConfig::default();
    thinker_config.prompt.runtime_capabilities = runtime_capabilities;
    thinker_config.prompt.generation_models =
        format_generation_models_for_prompt(generation_config);

    // Create components
    let provider_registry = Arc::new(SingleProviderRegistry::new(provider.clone()));
    let thinker = Arc::new(Thinker::new(provider_registry, thinker_config));

    // Create executor with builtin tool registry (with generation support)
    let tool_config = BuiltinToolConfig {
        generation_registry: Some(generation_registry.clone()),
        ..Default::default()
    };
    let tool_registry = Arc::new(BuiltinToolRegistry::with_config(tool_config));
    let executor = Arc::new(SingleStepExecutor::new(tool_registry));

    // Create compressor (rule-based for now)
    let compressor = Arc::new(NoOpCompressor);

    // Create callback adapter
    let callback = FfiLoopCallback::new(handler.clone());

    // Create abort signal from cancellation token
    let (abort_tx, abort_rx) = tokio::sync::watch::channel(false);
    let op_token_clone = op_token.clone();
    runtime.spawn(async move {
        op_token_clone.cancelled().await;
        let _ = abort_tx.send(true);
    });

    // Run the Agent Loop
    let result = runtime.block_on(async {
        let agent_loop = AgentLoop::new(thinker, executor, compressor, loop_config);

        agent_loop
            .run(
                full_input.clone(),
                request_context,
                tools,
                &callback,
                Some(abort_rx),
                if initial_history.is_empty() {
                    None
                } else {
                    Some(initial_history.clone())
                },
            )
            .await
    });

    // Note: We don't save AgentLoop results to conversation_histories because
    // rig's Message type is complex (OneOrMany<UserContent>). The initial_history
    // mechanism provides cross-session context by reading existing history at start.
    // For full multi-turn support, use execute_with_agent_manager which handles
    // rig's Message format properly.

    // Handle result
    match result {
        LoopResult::Completed { summary, steps, .. } => {
            info!(steps = steps, "Agent Loop completed");

            // Collect files written during tool execution
            let written_files = take_written_files();
            let final_summary = if written_files.is_empty() {
                summary.clone()
            } else {
                // Append file markers that Swift can parse
                let file_urls: Vec<String> = written_files
                    .iter()
                    .map(|f| format!("file://{}", f.path.display()))
                    .collect();
                info!(
                    file_count = written_files.len(),
                    files = ?file_urls,
                    "Appending generated files to agent loop response"
                );
                format!(
                    "{}\n\n[GENERATED_FILES]\n{}\n[/GENERATED_FILES]",
                    summary,
                    file_urls.join("\n")
                )
            };

            // Store memory if enabled
            if memory_config.enabled {
                if let Some(ref db_path) = memory_path {
                    let store_result = runtime.block_on(async {
                        store_memory_after_response(
                            db_path,
                            memory_config,
                            input_for_memory,
                            &summary,
                            app_context.as_deref(),
                            window_title.as_deref(),
                            None,
                        )
                        .await
                    });

                    if let Err(e) = store_result {
                        warn!(error = %e, "Failed to store memory (non-blocking)");
                    }
                }
            }

            handler.on_complete(final_summary);
        }

        LoopResult::Failed { reason, steps } => {
            warn!(steps = steps, reason = %reason, "Agent Loop failed");
            handler.on_error(reason);
        }

        LoopResult::GuardTriggered(violation) => {
            warn!(violation = ?violation, "Agent Loop guard triggered");
            handler.on_error(format!("Limit reached: {}", violation.description()));
        }

        LoopResult::UserAborted => {
            info!("Agent Loop aborted by user");
            handler.on_error("Operation cancelled".to_string());
        }
    }
}
