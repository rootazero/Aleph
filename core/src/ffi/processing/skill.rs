//! Skill execution through AgentLoop

use crate::agent_loop::{
    AgentLoop, LoopConfig, LoopResult, RequestContext as AgentRequestContext,
};
use crate::agents::rig::ChatMessage;
use crate::agents::RigAgentConfig;
use crate::compressor::NoOpCompressor;
use crate::config::GenerationConfig;
use crate::core::MediaAttachment;
use crate::dispatcher::{ToolSource, UnifiedTool};
use crate::executor::{BuiltinToolConfig, BuiltinToolRegistry, SingleStepExecutor};
use crate::ffi::prompt_helpers::{
    build_history_summary_from_conversations, format_generation_models_for_prompt,
};
use crate::ffi::provider_factory::create_provider_from_config;
use crate::ffi::tool_discovery::get_builtin_tool_descriptions;
use crate::ffi::FfiLoopCallback;
use crate::ffi::AetherEventHandler;
use crate::generation::GenerationProviderRegistry;
use crate::intent::SkillInvocation;
use crate::rig_tools::file_ops::{clear_written_files, take_written_files};
use crate::rig_tools::set_tool_progress_handler;
use crate::runtimes::{RuntimeCapability, RuntimeRegistry};
use crate::thinker::{SingleProviderRegistry, Thinker, ThinkerConfig};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::memory::store_memory_after_response;
use super::progress_callback::FfiToolProgressAdapter;

/// Run a Skill through AgentLoop for full streaming support
///
/// This provides Claude Code CLI style progress updates including:
/// - Thinking process streaming
/// - Tool call start/end notifications
/// - Step-by-step progress
#[allow(clippy::too_many_arguments)]
pub fn run_skill_with_agent_loop(
    runtime: &tokio::runtime::Handle,
    skill: &SkillInvocation,
    config: &RigAgentConfig,
    op_token: &CancellationToken,
    handler: &Arc<dyn AetherEventHandler>,
    memory_config: &crate::config::MemoryConfig,
    memory_path: &Option<String>,
    input_for_memory: &str,
    app_context: &Option<String>,
    generation_config: &GenerationConfig,
    generation_registry: &Arc<RwLock<GenerationProviderRegistry>>,
    _attachments: Option<&[MediaAttachment]>,
    attachment_text: Option<&str>,
    conversation_histories: &Arc<RwLock<HashMap<String, Vec<ChatMessage>>>>,
    topic_id: &Option<String>,
    preferred_language: &Option<String>,
) {
    // Check if already cancelled
    if op_token.is_cancelled() {
        handler.on_error("Operation cancelled".to_string());
        return;
    }

    // Clear any previously tracked written files for this session
    clear_written_files();

    // Set up tool progress callback for additional streaming updates
    let progress_adapter = Arc::new(FfiToolProgressAdapter::new(handler.clone()));
    set_tool_progress_handler(Some(progress_adapter));

    // Build the skill execution prompt
    // Include attachment content if present, placing it FIRST with clear instructions
    let full_input = if let Some(att_text) = attachment_text {
        info!(
            attachment_len = att_text.len(),
            skill_id = %skill.skill_id,
            "Including attachment text in skill agent loop context"
        );
        format!(
            "# 用户附件内容 (已内联提供，无需从磁盘读取)\n\n{}\n\n---\n\n# Skill: {}\n\n{}\n\n---\n\n用户请求: {}",
            att_text, skill.display_name, skill.instructions, skill.args
        )
    } else {
        format!(
            "# Skill: {}\n\n{}\n\n---\n\n用户请求: {}",
            skill.display_name, skill.instructions, skill.args
        )
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
            error!(error = %e, "Failed to create provider for skill");
            set_tool_progress_handler(None);
            handler.on_error(format!("Provider error: {}", e));
            return;
        }
    };

    // Build request context with skill metadata
    let mut request_context = AgentRequestContext::empty();
    request_context.current_app = app_context.clone();
    request_context
        .metadata
        .insert("skill_id".to_string(), skill.skill_id.clone());
    request_context
        .metadata
        .insert("skill_name".to_string(), skill.display_name.clone());

    // Create loop config - skills may need more steps for complex operations
    let loop_config = LoopConfig::default()
        .with_max_steps(30)
        .with_max_tokens(150_000);

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
    // Enable skill mode for strict workflow execution
    thinker_config.prompt.skill_mode = true;
    // Set preferred language for AI responses
    thinker_config.prompt.language = preferred_language.clone();

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

    // Create compressor (no-op for now)
    let compressor = Arc::new(NoOpCompressor);

    // Create callback adapter for streaming to UI
    let callback = FfiLoopCallback::new(handler.clone());

    // Create abort signal from cancellation token
    let (abort_tx, abort_rx) = tokio::sync::watch::channel(false);
    let op_token_clone = op_token.clone();
    runtime.spawn(async move {
        op_token_clone.cancelled().await;
        let _ = abort_tx.send(true);
    });

    info!(
        skill_id = %skill.skill_id,
        skill_name = %skill.display_name,
        "Starting skill execution via AgentLoop"
    );

    // Build initial history from conversation histories (for cross-session context)
    // - Single-turn (topic_id = None): 🔒 FROZEN - No history injection, returns empty string
    // - Multi-turn (topic_id = Some(uuid)): ✅ ACTIVE - Full context from previous turns
    let initial_history = build_history_summary_from_conversations(
        conversation_histories,
        topic_id,
        2000, // Max 2000 characters for history summary
    );

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
                    Some(initial_history)
                },
            )
            .await
    });

    // Clear tool progress callback
    set_tool_progress_handler(None);

    // Handle result
    match result {
        LoopResult::Completed { summary, steps, .. } => {
            info!(
                skill_id = %skill.skill_id,
                steps = steps,
                "Skill execution via AgentLoop completed"
            );

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
                    "Appending generated files to skill response"
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
                            None,
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
            warn!(
                skill_id = %skill.skill_id,
                steps = steps,
                reason = %reason,
                "Skill execution via AgentLoop failed"
            );
            handler.on_error(reason);
        }

        LoopResult::GuardTriggered(violation) => {
            warn!(
                skill_id = %skill.skill_id,
                violation = ?violation,
                "Skill execution guard triggered"
            );
            handler.on_error(format!("Limit reached: {}", violation.description()));
        }

        LoopResult::UserAborted => {
            info!(skill_id = %skill.skill_id, "Skill execution aborted by user");
            handler.on_error("Operation cancelled".to_string());
        }
    }
}
