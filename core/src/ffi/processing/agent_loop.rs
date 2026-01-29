//! Agent loop execution for tasks requiring LLM thinking

use crate::agent_loop::{
    AgentLoop, LoopConfig, LoopResult, RequestContext as AgentRequestContext,
};
use crate::agents::rig::ChatMessage;
use crate::agents::RigAgentConfig;
use crate::compressor::NoOpCompressor;
use crate::config::GenerationConfig;
use crate::core::MediaAttachment;
use crate::dispatcher::{
    SmartToolFilter, ToolFilterConfig, ToolIndex, ToolIndexCategory, ToolIndexEntry, ToolRegistry as DispatcherToolRegistry, ToolSource, UnifiedTool,
};
use crate::executor::{BuiltinToolConfig, BuiltinToolRegistry, SingleStepExecutor};
use crate::ffi::prompt_helpers::{
    build_history_summary_from_conversations, extract_attachment_text,
    format_generation_models_for_prompt,
};
use crate::ffi::provider_factory::create_provider_from_config;
use crate::ffi::tool_discovery::get_builtin_tool_descriptions;
use crate::ffi::AetherEventHandler;
use crate::ffi::FfiLoopCallback;
use crate::generation::GenerationProviderRegistry;
use crate::intent::{TaskCategory, ThinkingContext};
use crate::builtin_tools::file_ops::{
    clear_written_files, mark_session_start, scan_new_files_in_working_dir, take_written_files,
};
use crate::runtimes::{RuntimeCapability, RuntimeRegistry};
use crate::thinker::{SingleProviderRegistry, Thinker, ThinkerConfig};
use crate::tools::AetherToolServerHandle;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Safely truncate a string at character boundaries (UTF-8 safe)
fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let end_byte = s
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    format!("{}...", &s[..end_byte])
}
use tokio::sync::RwLock as TokioRwLock;
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
    _tool_server_handle: AetherToolServerHandle,
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
    conversation_histories: &Arc<RwLock<HashMap<String, Vec<ChatMessage>>>>,
    topic_id: &Option<String>,
    generation_registry: &Arc<RwLock<GenerationProviderRegistry>>,
    preferred_language: &Option<String>,
) {
    // Check if already cancelled
    if op_token.is_cancelled() {
        handler.on_error("Operation cancelled".to_string());
        return;
    }

    // Clear any previously tracked written files for this session
    clear_written_files();

    // Mark the start of session for detecting newly created files
    mark_session_start();

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
    let all_tools: Vec<UnifiedTool> = tool_descriptions
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

    // === Smart Tool Discovery: Two-Stage Tool Filtering ===
    // 1. Filter tools by intent category (core + relevant get full schema)
    // 2. Enhance with content analysis from user request
    // 3. Generate tool index for remaining tools (name + summary only)
    let task_category = ctx.category_hint.unwrap_or(TaskCategory::General);
    let smart_filter = SmartToolFilter::new(ToolFilterConfig::default());
    let filter_result = smart_filter.filter(&all_tools, task_category, input, None);

    // Log filtering results
    info!(
        task_category = ?task_category,
        core_tools = filter_result.core_tools.len(),
        filtered_tools = filter_result.filtered_tools.len(),
        indexed_tools = filter_result.indexed_tools.len(),
        "Smart tool discovery: filtered tools by intent category"
    );

    // Generate tool index for indexed tools (lightweight prompt injection)
    let tool_index_prompt = if !filter_result.indexed_tools.is_empty() {
        let mut index = ToolIndex::new();
        for tool in &filter_result.indexed_tools {
            let category = ToolIndexCategory::from(&tool.source);
            // Truncate description to ~50 chars for compact index
            let summary = truncate_str(&tool.description, 25);
            let entry = ToolIndexEntry::new(&tool.name, category, summary);
            index.add(entry);
        }
        Some(index.to_prompt())
    } else {
        None
    };

    // Get tools with full schema (core + filtered)
    let tools = filter_result.full_schema_tools();

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
    // - Single-turn (topic_id = None): 🔒 FROZEN - No history injection, returns empty string
    // - Multi-turn (topic_id = Some(uuid)): ✅ ACTIVE - Full context from previous turns
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

    // Create thinker config with runtime capabilities, generation models, and tool index
    let mut thinker_config = ThinkerConfig::default();
    thinker_config.prompt.runtime_capabilities = runtime_capabilities;
    thinker_config.prompt.generation_models =
        format_generation_models_for_prompt(generation_config);
    // Enable two-stage tool discovery with tool index
    thinker_config.prompt.tool_index = tool_index_prompt;
    // Set preferred language for AI responses
    thinker_config.prompt.language = preferred_language.clone();

    // Create dispatcher tool registry for meta tools (smart tool discovery)
    let dispatcher_registry = Arc::new(TokioRwLock::new(DispatcherToolRegistry::new()));

    // Create components
    let provider_registry = Arc::new(SingleProviderRegistry::new(provider.clone()));
    let thinker = Arc::new(Thinker::new(provider_registry, thinker_config));

    // Create executor with builtin tool registry (with generation and dispatcher support)
    let tool_config = BuiltinToolConfig {
        generation_registry: Some(generation_registry.clone()),
        dispatcher_registry: Some(Arc::clone(&dispatcher_registry)),
        ..Default::default()
    };
    let tool_registry = Arc::new(BuiltinToolRegistry::with_config(tool_config));
    let executor = Arc::new(SingleStepExecutor::new(tool_registry));

    // Register builtin tools in dispatcher registry for meta tools
    runtime.block_on(async {
        dispatcher_registry.write().await.register_builtin_tools().await;
    });
    info!("Registered builtin tools in dispatcher registry for smart tool discovery");

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
            let mut written_files = take_written_files();

            // Scan working directory for additional files created during session
            // This captures files created by bash commands, Python scripts, etc.
            let scanned_files = scan_new_files_in_working_dir();
            if !scanned_files.is_empty() {
                info!(
                    scanned_count = scanned_files.len(),
                    "Found additional files in working directory not tracked by tools"
                );
                written_files.extend(scanned_files);
            }

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

            // ✅ MULTI-TURN: Save conversation history for context injection
            // This fixes the missing conversation_histories.write() logic
            if let Some(tid) = topic_id {
                if let Ok(mut histories) = conversation_histories.write() {
                    // Get or create the message list for this topic
                    let messages = histories.entry(tid.clone()).or_insert_with(Vec::new);

                    // Append user message and assistant response
                    messages.push(ChatMessage::user(input));
                    messages.push(ChatMessage::assistant(&summary));

                    // Limit history length to prevent memory bloat (keep last 50 messages = 25 turns)
                    const MAX_HISTORY_MESSAGES: usize = 50;
                    if messages.len() > MAX_HISTORY_MESSAGES {
                        let drain_count = messages.len() - MAX_HISTORY_MESSAGES;
                        messages.drain(0..drain_count);
                        debug!(
                            topic_id = %tid,
                            drained = drain_count,
                            remaining = messages.len(),
                            "Trimmed conversation history"
                        );
                    }

                    debug!(
                        topic_id = %tid,
                        message_count = messages.len(),
                        "Saved conversation history"
                    );
                } else {
                    warn!("Failed to acquire write lock on conversation_histories");
                }
            }
            // Note: Single-turn (topic_id = None) does not save history (🔒 FROZEN behavior)

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
