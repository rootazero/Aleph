//! Processing methods for AetherCore
//!
//! This module contains AI processing methods: process, cancel, generate_topic_title, extract_text
//!
//! # Architecture
//!
//! Two processing paths are available:
//!
//! ## Legacy Path (RequestOrchestrator) - DEPRECATED
//!
//! Uses `RequestOrchestrator` with two-phase pipeline:
//! - Phase 1 (ExecutionIntentDecider): Decides execution mode
//! - Phase 2 (Dispatcher): Tool and model routing
//!
//! ## New Path (Agent Loop) - RECOMMENDED
//!
//! Uses `IntentRouter` + `AgentLoop` with observe-think-act cycle:
//! - L0-L2: Fast routing via IntentRouter
//! - Agent Loop: LLM-based thinking for complex tasks

// Allow deprecated orchestrator usage during transition
#![allow(deprecated)]

use super::{AetherCore, AetherFfiError};
use crate::agents::RigAgentManager;
use crate::command::CommandParser;
use crate::config::RoutingRuleConfig;
use crate::intent::{AgentModePrompt, ToolDescription};
use crate::memory::{ContextAnchor, EmbeddingModel, MemoryIngestion, VectorDatabase};
use crate::orchestrator::{OrchestratorMode, OrchestratorRequest, RequestContext as OrchestratorRequestContext, RequestOrchestrator};
use crate::prompt::PromptBuilder;
use crate::skills::SkillsRegistry;
use crate::utils::paths::get_skills_dir;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

// New Agent Loop imports
use crate::agent_loop::{
    AgentLoop, LoopConfig, LoopResult, RequestContext as AgentRequestContext,
};
use crate::compressor::NoOpCompressor;
use crate::ffi::FfiLoopCallback;
use crate::intent::{DirectMode, IntentRouter, RouteResult, ThinkingContext};
use crate::thinker::{SingleProviderRegistry, Thinker, ThinkerConfig};

/// Processing options
#[derive(Debug, Clone)]
pub struct ProcessOptions {
    /// Application context (bundle ID)
    pub app_context: Option<String>,
    /// Window title of the active application
    pub window_title: Option<String>,
    /// Topic ID for multi-turn conversations (None = "single-turn")
    pub topic_id: Option<String>,
    /// Enable streaming mode
    pub stream: bool,
    /// Media attachments for multimodal content (images, etc.)
    pub attachments: Option<Vec<crate::core::MediaAttachment>>,
}

impl Default for ProcessOptions {
    fn default() -> Self {
        Self {
            app_context: None,
            window_title: None,
            topic_id: None, // None means "single-turn"
            stream: true,   // Streaming enabled by default
            attachments: None,
        }
    }
}

impl ProcessOptions {
    /// Create new processing options with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the application context
    pub fn with_app_context(mut self, context: String) -> Self {
        self.app_context = Some(context);
        self
    }

    /// Set the window title
    pub fn with_window_title(mut self, title: String) -> Self {
        self.window_title = Some(title);
        self
    }

    /// Set streaming mode
    pub fn with_stream(mut self, stream: bool) -> Self {
        self.stream = stream;
        self
    }
}

impl AetherCore {
    /// Process user input asynchronously
    ///
    /// This method processes the input on a background thread and calls
    /// the appropriate handler callbacks during processing.
    ///
    /// # Architecture (Simplified 2-Layer)
    ///
    /// - **L1**: Slash command check (immediate routing for /agent, /search, /skills, etc.)
    /// - **L3**: AI unified planner for everything else (decides: conversational, single action, or task graph)
    ///
    /// The operation can be cancelled by calling `cancel()`. When cancelled,
    /// the handler's `on_error` callback will be invoked with "Operation cancelled".
    ///
    /// # Hot-Reload Support
    ///
    /// Uses a shared `ToolServerHandle` so that dynamically added/removed tools
    /// are available across all `process()` calls without restarting.
    pub fn process(
        &self,
        input: String,
        options: Option<ProcessOptions>,
    ) -> Result<(), AetherFfiError> {
        let options = options.unwrap_or_default();
        // Extract attachments for multimodal support (images, documents)
        let attachments = options.attachments.clone();
        // Extract context for memory storage
        let app_context = options.app_context.clone();
        let window_title = options.window_title.clone();
        let topic_id = options.topic_id.clone();
        let _stream = options.stream; // TODO: Use streaming mode in orchestrator

        let handler = Arc::clone(&self.handler);
        // Acquire read lock to get current config (supports config reload)
        let config = self.config_holder.read().unwrap().config().clone();
        let runtime = self.runtime.clone();
        // Clone shared tool server handle for use in the new thread
        let tool_server_handle = self.tool_server_handle.clone();
        let registered_tools = Arc::clone(&self.registered_tools);

        // Clone memory config and path for memory storage
        let memory_config = {
            let full_config = self.full_config.lock().unwrap_or_else(|e| e.into_inner());
            full_config.memory.clone()
        };
        let memory_path = self
            .memory_path
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let input_for_memory = input.clone();

        // Clone generation config for model aliases
        // Clone routing rules for slash command parsing
        // Clone orchestrator config for three-layer control switch
        let (generation_config, routing_rules, use_three_layer_control) = {
            let full_config = self.full_config.lock().unwrap_or_else(|e| e.into_inner());
            (
                full_config.generation.clone(),
                full_config.rules.clone(),
                full_config.orchestrator.use_three_layer_control,
            )
        };

        // Clone conversation histories for multi-turn support
        let conversation_histories = Arc::clone(&self.conversation_histories);

        // Create a fresh token for this operation
        // This resets cancellation state, allowing new operations after previous cancellations
        let op_token = self.reset_cancel_token();

        // Spawn a background thread to handle processing
        std::thread::spawn(move || {
            // Check if already cancelled before starting
            if op_token.is_cancelled() {
                handler.on_error("Operation cancelled".to_string());
                return;
            }

            handler.on_thinking();

            // ================================================================
            // Orchestrator Selection (Config-based routing)
            // ================================================================
            // Check if three-layer control is enabled
            if use_three_layer_control {
                // New path: ThreeLayerOrchestrator (Phase 4+ implementation)
                // Currently returns placeholder - full implementation pending
                info!("Three-layer control enabled but not yet fully implemented");
                handler.on_error(
                    "ThreeLayerOrchestrator is not yet fully implemented. \
                     Set orchestrator.use_three_layer_control = false to use legacy orchestrator."
                        .to_string(),
                );
                return;
            }

            // ================================================================
            // RequestOrchestrator-based processing (Legacy Path)
            // ================================================================
            // All processing goes through the orchestrator which handles:
            // - Builtin commands (/screenshot, /search, etc.)
            // - Skills (/skill_name with instructions)
            // - MCP commands (/mcp_server)
            // - Custom commands (from routing rules)
            // - Natural language tasks (Execute or Converse mode)
            info!("Processing via RequestOrchestrator (legacy)");
            #[allow(deprecated)]
            process_with_orchestrator(
                &runtime,
                &input,
                &app_context,
                &window_title,
                &config,
                tool_server_handle,
                registered_tools,
                &conversation_histories,
                &topic_id,
                attachments.as_deref(),
                &op_token,
                &handler,
                &memory_config,
                &memory_path,
                &input_for_memory,
                &generation_config,
                &routing_rules,
            );

            // Processing complete - orchestrator handles all paths
        });

        Ok(())
    }

    /// Cancel current operation
    ///
    /// Triggers cancellation of the current in-progress operation.
    /// The handler's `on_error` callback will be invoked with "Operation cancelled".
    /// After cancellation, subsequent calls to `process()` will work normally
    /// since each operation gets a fresh cancellation token.
    pub fn cancel(&self) {
        info!("Cancel requested, triggering cancellation");
        self.current_op_token.read().unwrap().cancel();
    }

    /// Check if the current operation has been cancelled
    pub fn is_cancelled(&self) -> bool {
        self.current_op_token.read().unwrap().is_cancelled()
    }

    /// Create a fresh cancellation token for a new operation
    ///
    /// This replaces the current token with a new one, effectively resetting
    /// the cancellation state. Returns a clone of the new token for the operation.
    pub(crate) fn reset_cancel_token(&self) -> CancellationToken {
        let new_token = CancellationToken::new();
        let token_clone = new_token.clone();
        *self.current_op_token.write().unwrap() = new_token;
        token_clone
    }

    /// Generate a title for a conversation topic using AI
    ///
    /// Uses the default provider to generate a concise title from the first
    /// user-AI exchange in a conversation.
    pub fn generate_topic_title(
        &self,
        user_input: String,
        ai_response: String,
    ) -> Result<String, AetherFfiError> {
        use crate::title_generator;

        info!(
            user_input_len = user_input.len(),
            ai_response_len = ai_response.len(),
            "Generating topic title"
        );

        // Build the title prompt
        let prompt = title_generator::build_title_prompt(&user_input, &ai_response);

        // Get full config to find default provider and its config
        let full_cfg = self.full_config.lock().unwrap();
        let default_provider_name = full_cfg.general.default_provider.clone();

        // Try to get the provider
        let provider = match &default_provider_name {
            Some(name) => {
                // Find the provider config
                let provider_config = full_cfg.providers.get(name);
                match provider_config {
                    Some(cfg) => match crate::providers::create_provider(name, cfg.clone()) {
                        Ok(p) => Some(p),
                        Err(e) => {
                            info!(error = %e, "Failed to create provider for title generation");
                            None
                        }
                    },
                    None => {
                        info!(provider = %name, "Default provider not found in config");
                        None
                    }
                }
            }
            None => None,
        };

        // Release the lock before making the async call
        drop(full_cfg);

        match provider {
            Some(p) => {
                // Execute AI call using the runtime
                let result: Result<String, crate::error::AetherError> = self
                    .runtime
                    .block_on(async move { p.process(&prompt, None).await });

                match result {
                    Ok(title) => {
                        let cleaned = title_generator::clean_title(&title);
                        info!(title = %cleaned, "Topic title generated");
                        Ok(cleaned)
                    }
                    Err(e) => {
                        let default = title_generator::default_title(&user_input);
                        info!(error = %e, default_title = %default, "AI title failed, using default");
                        Ok(default)
                    }
                }
            }
            None => {
                let default = title_generator::default_title(&user_input);
                info!(default_title = %default, "No provider available, using default title");
                Ok(default)
            }
        }
    }

    /// Extract text from image data using OCR
    ///
    /// Uses the configured default AI provider to perform OCR on the image.
    pub fn extract_text(&self, image_data: Vec<u8>) -> Result<String, AetherFfiError> {
        use crate::vision::VisionService;

        info!(data_size = image_data.len(), "Extracting text from image");

        // Get config for vision service
        let config = self
            .full_config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();

        // Create vision service and extract text
        let vision_service = VisionService::with_defaults();

        self.runtime.block_on(async {
            vision_service
                .extract_text(image_data, &config)
                .await
                .map_err(|e| AetherFfiError::Config(format!("OCR failed: {}", e)))
        })
    }
}
/// Execute input using RigAgentManager (existing code path)
#[allow(clippy::too_many_arguments)]
fn execute_with_agent_manager(
    runtime: &tokio::runtime::Handle,
    processed_input: &str,
    config: &crate::agents::RigAgentConfig,
    tool_server_handle: rig::tool::server::ToolServerHandle,
    registered_tools: Arc<std::sync::RwLock<Vec<String>>>,
    conversation_histories: &Arc<
        std::sync::RwLock<std::collections::HashMap<String, Vec<rig::completion::Message>>>,
    >,
    topic_id: &Option<String>,
    attachments: Option<&[crate::core::MediaAttachment]>,
    op_token: &CancellationToken,
    handler: &Arc<dyn crate::ffi::AetherEventHandler>,
    memory_config: &crate::config::MemoryConfig,
    memory_path: &Option<String>,
    input_for_memory: &str,
    app_context: &Option<String>,
    window_title: &Option<String>,
) {
    // Create manager with shared ToolServerHandle (all tools persist across calls)
    let manager =
        RigAgentManager::with_shared_handle(config.clone(), tool_server_handle, registered_tools);

    // Get or create conversation history for this topic
    let topic_key = topic_id
        .clone()
        .unwrap_or_else(|| "single-turn".to_string());
    let mut history = {
        let histories = conversation_histories.read().unwrap();
        histories.get(&topic_key).cloned().unwrap_or_default()
    };
    let history_len_before = history.len();

    let result = runtime.block_on(async {
        tokio::select! {
            biased;

            // Check for cancellation first (biased mode)
            _ = op_token.cancelled() => {
                Err(crate::error::AetherError::cancelled())
            }

            // Process with conversation history and attachments for multi-turn + multimodal support
            result = manager.process_with_history_and_attachments(processed_input, &mut history, attachments) => {
                result
            }
        }
    });

    // Update conversation history after processing
    // rig-core's with_history() mutates the history to add the current exchange
    if history.len() > history_len_before {
        let mut histories = conversation_histories.write().unwrap();
        histories.insert(topic_key.clone(), history);
        debug!(topic_id = %topic_key, "Conversation history updated");
    }

    match result {
        Ok(response) => {
            // Store memory if enabled
            if memory_config.enabled {
                if let Some(ref db_path) = memory_path {
                    let store_result = runtime.block_on(async {
                        store_memory_after_response(
                            db_path,
                            memory_config,
                            input_for_memory,
                            &response.content,
                            app_context.as_deref(),
                            window_title.as_deref(),
                            topic_id.as_deref(),
                        )
                        .await
                    });

                    match store_result {
                        Ok(memory_id) => {
                            info!(memory_id = %memory_id, "Memory stored successfully");
                            handler.on_memory_stored();
                        }
                        Err(e) => {
                            warn!(error = %e, "Failed to store memory (non-blocking)");
                        }
                    }
                }
            }

            // If tokio::select! returned the result branch, the operation completed successfully
            handler.on_complete(response.content);
        }
        Err(e) => {
            // Check if the error is due to cancellation
            if op_token.is_cancelled() {
                handler.on_error("Operation cancelled".to_string());
            } else {
                error!(error = %e, "Processing failed");
                handler.on_error(e.to_string());
            }
        }
    }
}

/// Helper function to store memory after AI response
///
/// This function is called in the background thread after a successful AI response.
/// It creates the necessary memory components on demand and stores the interaction.
///
/// # Arguments
/// * `db_path` - Path to the memory database
/// * `memory_config` - Memory configuration
/// * `user_input` - Original user input
/// * `ai_output` - AI response content
/// * `app_context` - Application bundle ID (optional)
/// * `window_title` - Window title (optional)
/// * `topic_id` - Topic ID for multi-turn conversations (None = "single-turn")
pub(crate) async fn store_memory_after_response(
    db_path: &str,
    memory_config: &crate::config::MemoryConfig,
    user_input: &str,
    ai_output: &str,
    app_context: Option<&str>,
    window_title: Option<&str>,
    topic_id: Option<&str>,
) -> Result<String, crate::error::AetherError> {
    use crate::memory::context::SINGLE_TURN_TOPIC_ID;

    // Create ContextAnchor with topic_id
    let context = ContextAnchor::with_topic(
        app_context.unwrap_or("").to_string(),
        window_title.unwrap_or("").to_string(),
        topic_id.unwrap_or(SINGLE_TURN_TOPIC_ID).to_string(),
    );

    // Create VectorDatabase
    let db = VectorDatabase::new(PathBuf::from(db_path)).map_err(|e| {
        crate::error::AetherError::config(format!("Failed to open memory database: {}", e))
    })?;

    // Create EmbeddingModel
    let model_path = EmbeddingModel::get_default_model_path().map_err(|e| {
        crate::error::AetherError::config(format!("Failed to get model path: {}", e))
    })?;
    let embedding_model = EmbeddingModel::new(model_path).map_err(|e| {
        crate::error::AetherError::config(format!("Failed to create embedding model: {}", e))
    })?;

    // Create MemoryIngestion
    let ingestion = MemoryIngestion::new(
        Arc::new(db),
        Arc::new(embedding_model),
        Arc::new(memory_config.clone()),
    );

    // Store memory
    ingestion.store_memory(context, user_input, ai_output).await
}

/// Get descriptions for built-in tools
///
/// Returns tool descriptions for the agent prompt so AI knows what tools are available.
/// Includes image generation tool if providers are configured.
fn get_builtin_tool_descriptions(
    generation_config: &crate::config::GenerationConfig,
) -> Vec<ToolDescription> {
    use crate::generation::GenerationType;

    let mut tools = vec![
        ToolDescription::new(
            "file_ops",
            "文件系统操作 - 支持 list(列出目录)、read、write、move、copy、delete、mkdir、search、**organize**(一键按类型整理到 Images/Documents/Videos/Audio/Archives/Code/Others)、**batch_move**(批量移动匹配文件)"
        ),
        ToolDescription::new(
            "search",
            "网络搜索 - 搜索互联网获取最新信息"
        ),
        ToolDescription::new(
            "web_fetch",
            "获取网页内容 - 读取指定URL的网页内容"
        ),
        ToolDescription::new(
            "youtube",
            "YouTube视频信息 - 获取YouTube视频的标题、描述、字幕等信息"
        ),
    ];

    // Add image generation tool if providers are configured
    let all_providers: Vec<_> = generation_config.providers.iter().collect();
    debug!(
        all_providers_count = all_providers.len(),
        "Listing all generation providers for debugging"
    );
    for (name, config) in &all_providers {
        debug!(
            provider = %name,
            enabled = config.enabled,
            capabilities = ?config.capabilities,
            "Generation provider config"
        );
    }

    let image_providers: Vec<String> = generation_config
        .get_providers_for_type(GenerationType::Image)
        .iter()
        .map(|(name, _)| name.to_string())
        .collect();

    debug!(
        image_providers_count = image_providers.len(),
        image_providers = ?image_providers,
        "Filtered image providers"
    );

    if !image_providers.is_empty() {
        tools.push(ToolDescription::new(
            "generate_image",
            format!(
                "Image generation - generate images from text descriptions. Available providers: {}. Use the provider parameter to specify which model to use.",
                image_providers.join(", ")
            )
        ));
        info!(
            providers = ?image_providers,
            "Added generate_image tool to agent capabilities"
        );
    }

    // Add video generation tool if providers are configured
    let video_providers: Vec<String> = generation_config
        .get_providers_for_type(GenerationType::Video)
        .iter()
        .map(|(name, _)| name.to_string())
        .collect();

    if !video_providers.is_empty() {
        tools.push(ToolDescription::new(
            "generate_video",
            format!(
                "Video generation - generate videos from text descriptions. Available providers: {}. Use the provider parameter to specify which model to use.",
                video_providers.join(", ")
            )
        ));
        info!(
            providers = ?video_providers,
            "Added generate_video tool to agent capabilities"
        );
    }

    // Add audio generation tool if providers are configured
    let audio_providers: Vec<String> = generation_config
        .get_providers_for_type(GenerationType::Audio)
        .iter()
        .map(|(name, _)| name.to_string())
        .collect();

    if !audio_providers.is_empty() {
        tools.push(ToolDescription::new(
            "generate_audio",
            format!(
                "Audio/music generation - generate music or audio from text descriptions. Available providers: {}. Use the provider parameter to specify which model to use.",
                audio_providers.join(", ")
            )
        ));
        info!(
            providers = ?audio_providers,
            "Added generate_audio tool to agent capabilities"
        );
    }

    // Add speech generation tool if providers are configured
    let speech_providers: Vec<String> = generation_config
        .get_providers_for_type(GenerationType::Speech)
        .iter()
        .map(|(name, _)| name.to_string())
        .collect();

    if !speech_providers.is_empty() {
        tools.push(ToolDescription::new(
            "generate_speech",
            format!(
                "Speech/TTS generation - convert text to speech. Available providers: {}. Use the provider parameter to specify which model to use.",
                speech_providers.join(", ")
            )
        ));
        info!(
            providers = ?speech_providers,
            "Added generate_speech tool to agent capabilities"
        );
    }

    tools
}

// ============================================================================
// RequestOrchestrator-based processing (Unified Path)
// ============================================================================

/// Process input using the RequestOrchestrator
///
/// This function uses the two-phase architecture:
/// - Phase 1: ExecutionIntentDecider decides "execute vs converse"
/// - Phase 2: Dispatcher decides "which tool and model" (only for Execute mode)
///
/// Supports all command types:
/// - Builtin commands (/screenshot, /search, etc.)
/// - Skills (/skill_name with instructions)
/// - MCP commands (/mcp_server)
/// - Custom commands (from routing rules)
/// - Natural language tasks
#[allow(clippy::too_many_arguments)]
fn process_with_orchestrator(
    runtime: &tokio::runtime::Handle,
    input: &str,
    app_context: &Option<String>,
    _window_title: &Option<String>,
    config: &crate::agents::RigAgentConfig,
    tool_server_handle: rig::tool::server::ToolServerHandle,
    registered_tools: Arc<std::sync::RwLock<Vec<String>>>,
    conversation_histories: &Arc<
        std::sync::RwLock<std::collections::HashMap<String, Vec<rig::completion::Message>>>,
    >,
    topic_id: &Option<String>,
    attachments: Option<&[crate::core::MediaAttachment]>,
    op_token: &CancellationToken,
    handler: &Arc<dyn crate::ffi::AetherEventHandler>,
    memory_config: &crate::config::MemoryConfig,
    memory_path: &Option<String>,
    input_for_memory: &str,
    generation_config: &crate::config::GenerationConfig,
    routing_rules: &[RoutingRuleConfig],
) {
    // Build CommandParser with all dynamic command sources
    let mut command_parser = CommandParser::new();

    // Load skills registry
    if let Ok(skills_dir) = get_skills_dir() {
        let registry = SkillsRegistry::new(skills_dir);
        if registry.load_all().is_ok() {
            command_parser = command_parser.with_skills_registry(Arc::new(registry));
        }
    }

    // Add routing rules for custom commands
    command_parser = command_parser.with_routing_rules(routing_rules.to_vec());

    // TODO: Add MCP server names when available
    // command_parser = command_parser.with_mcp_servers(mcp_server_names);

    // Create ExecutionIntentDecider with the command parser
    let intent_decider = crate::intent::ExecutionIntentDecider::new()
        .with_command_parser(Arc::new(command_parser));

    // Create the orchestrator with the configured decider
    let orchestrator = RequestOrchestrator::with_intent_decider(intent_decider);

    // Build context from FFI options
    let context = OrchestratorRequestContext::from_ffi_options(app_context.clone(), None);

    // Get available tools for the orchestrator
    let tool_descriptions = get_builtin_tool_descriptions(generation_config);
    let unified_tools: Vec<crate::dispatcher::UnifiedTool> = tool_descriptions
        .iter()
        .map(|td| {
            crate::dispatcher::UnifiedTool::new(
                &format!("builtin:{}", td.name),
                &td.name,
                &td.description,
                crate::dispatcher::ToolSource::Native,
            )
        })
        .collect();

    // Create the request
    let request = OrchestratorRequest::new(input).with_tools(unified_tools);
    let request = if let Some(ctx) = context {
        request.with_context(ctx)
    } else {
        request
    };

    // Process through the orchestrator
    let result = match orchestrator.process(&request) {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, "Orchestrator processing failed");
            // Fallback to conversational mode on error
            handler.on_error(format!("Orchestrator error: {}", e));
            return;
        }
    };

    info!(
        mode = ?result.mode,
        phase = ?result.phase,
        confidence = result.confidence(),
        latency_us = result.latency_us(),
        "Orchestrator decision"
    );

    // Route based on orchestrator result
    match result.mode {
        OrchestratorMode::DirectTool { tool_id, args } => {
            // Direct tool invocation - execute immediately
            info!(tool_id = %tool_id, args = %args, "Direct tool execution via orchestrator");

            // For now, inject a tool trigger prompt similar to existing slash command handling
            let agent_prompt = AgentModePrompt::with_tools(tool_descriptions.clone())
                .with_generation_config(generation_config)
                .generate();

            let processed_input = format!(
                "{}\n\n---\n\n请使用 {} 工具处理: {}",
                agent_prompt, tool_id, args
            );

            execute_with_agent_manager(
                runtime,
                &processed_input,
                config,
                tool_server_handle,
                registered_tools,
                conversation_histories,
                topic_id,
                attachments,
                op_token,
                handler,
                memory_config,
                memory_path,
                input_for_memory,
                app_context,
                &None,
            );
        }

        OrchestratorMode::Converse => {
            // Conversation mode - use the prompt from orchestrator
            info!("Conversational mode via orchestrator");

            let prompt = result.prompt.unwrap_or_else(|| {
                PromptBuilder::conversational_prompt(None)
            });

            // Execute without agent tools
            execute_with_agent_manager(
                runtime,
                &format!("{}\n\n---\n\n{}", prompt, input),
                config,
                tool_server_handle,
                registered_tools,
                conversation_histories,
                topic_id,
                attachments,
                op_token,
                handler,
                memory_config,
                memory_path,
                input_for_memory,
                app_context,
                &None,
            );
        }

        OrchestratorMode::Skill { skill_id, display_name, instructions, args } => {
            // Skill mode - inject skill instructions as context
            info!(
                skill_id = %skill_id,
                skill_name = %display_name,
                "Skill execution via orchestrator"
            );

            // Build prompt with skill instructions
            let agent_prompt = AgentModePrompt::with_tools(tool_descriptions.clone())
                .with_generation_config(generation_config)
                .generate();

            let processed_input = format!(
                "# Skill: {}\n\n{}\n\n---\n\n{}\n\n---\n\n用户请求: {}",
                display_name, instructions, agent_prompt, args
            );

            execute_with_agent_manager(
                runtime,
                &processed_input,
                config,
                tool_server_handle,
                registered_tools,
                conversation_histories,
                topic_id,
                attachments,
                op_token,
                handler,
                memory_config,
                memory_path,
                input_for_memory,
                app_context,
                &None,
            );
        }

        OrchestratorMode::Mcp { server_name, tool_name, args } => {
            // MCP mode - route to MCP server
            info!(
                server_name = %server_name,
                tool_name = ?tool_name,
                "MCP execution via orchestrator"
            );

            // Build prompt with MCP tool hint
            let agent_prompt = AgentModePrompt::with_tools(tool_descriptions.clone())
                .with_generation_config(generation_config)
                .generate();

            let tool_hint = if let Some(ref tool) = tool_name {
                format!("请使用 {} 工具（来自 {} 服务器）处理: {}", tool, server_name, args)
            } else {
                format!("请使用 {} MCP 服务器的工具处理: {}", server_name, args)
            };

            let processed_input = format!("{}\n\n---\n\n{}", agent_prompt, tool_hint);

            execute_with_agent_manager(
                runtime,
                &processed_input,
                config,
                tool_server_handle,
                registered_tools,
                conversation_histories,
                topic_id,
                attachments,
                op_token,
                handler,
                memory_config,
                memory_path,
                input_for_memory,
                app_context,
                &None,
            );
        }

        OrchestratorMode::Custom { command_name, system_prompt, provider: _, args } => {
            // Custom command mode - use custom system prompt
            info!(
                command_name = %command_name,
                has_system_prompt = system_prompt.is_some(),
                "Custom command execution via orchestrator"
            );

            // Use custom system prompt if provided, otherwise use agent prompt
            let prompt = system_prompt.unwrap_or_else(|| {
                AgentModePrompt::with_tools(tool_descriptions.clone())
                    .with_generation_config(generation_config)
                    .generate()
            });

            let processed_input = format!("{}\n\n---\n\n用户输入: {}", prompt, args);

            execute_with_agent_manager(
                runtime,
                &processed_input,
                config,
                tool_server_handle,
                registered_tools,
                conversation_histories,
                topic_id,
                attachments,
                op_token,
                handler,
                memory_config,
                memory_path,
                input_for_memory,
                app_context,
                &None,
            );
        }

        OrchestratorMode::Execute { category, ref task_intent } => {
            // Execute mode - use agent prompt with filtered tools
            info!(
                category = ?category,
                task_intent = %task_intent,
                tools_count = result.tools.len(),
                "Execute mode via orchestrator"
            );

            // Notify UI of agent mode
            let task = crate::intent::ExecutableTask {
                category,
                action: input.to_string(),
                target: None,
                confidence: result.confidence(),
            };
            handler.on_agent_mode_detected((&task).into());

            // Use the prompt generated by orchestrator
            let prompt = result.prompt.unwrap_or_else(|| {
                // Fallback: generate agent prompt with all tools
                AgentModePrompt::with_tools(tool_descriptions.clone())
                    .with_generation_config(generation_config)
                    .generate()
            });

            let processed_input = format!("{}\n\n---\n\n用户请求: {}", prompt, input);

            execute_with_agent_manager(
                runtime,
                &processed_input,
                config,
                tool_server_handle,
                registered_tools,
                conversation_histories,
                topic_id,
                attachments,
                op_token,
                handler,
                memory_config,
                memory_path,
                input_for_memory,
                app_context,
                &None,
            );
        }
    }
}

// ============================================================================
// Agent Loop-based processing (New Architecture)
// ============================================================================

/// Process input using the new Agent Loop architecture
///
/// This function implements the new observe-think-act-feedback loop:
/// - L0-L2: Fast routing via IntentRouter (slash commands, patterns, context)
/// - Agent Loop: LLM-based thinking for complex tasks
///
/// # Architecture Flow
///
/// ```text
/// User Input
///     ↓
/// IntentRouter (L0-L2)
///     ├── DirectRoute → Execute immediately (slash commands, etc.)
///     └── NeedsThinking → Agent Loop
///                             ↓
///                         ┌─────────────────────────┐
///                         │ Guards → Compress →     │
///                         │ Think → Decide →        │
///                         │ Execute → Feedback      │
///                         │ (repeat until done)     │
///                         └─────────────────────────┘
/// ```
#[allow(dead_code)]
#[allow(clippy::too_many_arguments)]
fn process_with_agent_loop(
    runtime: &tokio::runtime::Handle,
    input: &str,
    app_context: &Option<String>,
    window_title: &Option<String>,
    config: &crate::agents::RigAgentConfig,
    tool_server_handle: rig::tool::server::ToolServerHandle,
    registered_tools: Arc<std::sync::RwLock<Vec<String>>>,
    _conversation_histories: &Arc<
        std::sync::RwLock<std::collections::HashMap<String, Vec<rig::completion::Message>>>,
    >,
    _topic_id: &Option<String>,
    _attachments: Option<&[crate::core::MediaAttachment]>,
    op_token: &CancellationToken,
    handler: &Arc<dyn crate::ffi::AetherEventHandler>,
    memory_config: &crate::config::MemoryConfig,
    memory_path: &Option<String>,
    input_for_memory: &str,
    generation_config: &crate::config::GenerationConfig,
    routing_rules: &[RoutingRuleConfig],
) {
    // ================================================================
    // Step 1: Build IntentRouter with dynamic command sources
    // ================================================================
    let mut command_parser = CommandParser::new();

    // Load skills registry
    if let Ok(skills_dir) = get_skills_dir() {
        let registry = SkillsRegistry::new(skills_dir);
        if registry.load_all().is_ok() {
            command_parser = command_parser.with_skills_registry(Arc::new(registry));
        }
    }

    // Add routing rules for custom commands
    command_parser = command_parser.with_routing_rules(routing_rules.to_vec());

    let router = IntentRouter::new().with_command_parser(Arc::new(command_parser));

    // ================================================================
    // Step 2: Route the input (L0-L2)
    // ================================================================
    let route_result = router.route(input, None);

    match route_result {
        RouteResult::DirectRoute(info) => {
            // Direct execution - skip Agent Loop
            info!(
                layer = ?info.layer,
                latency_us = info.latency_us,
                "Direct route - skipping Agent Loop"
            );

            handle_direct_route(
                runtime,
                input,
                info.mode,
                config,
                tool_server_handle,
                registered_tools,
                op_token,
                handler,
                memory_config,
                memory_path,
                input_for_memory,
                app_context,
                generation_config,
            );
        }

        RouteResult::NeedsThinking(ctx) => {
            // Needs Agent Loop for LLM-based thinking
            info!(
                category_hint = ?ctx.category_hint,
                bias_execute = ctx.bias_execute,
                latency_us = ctx.latency_us,
                "Needs thinking - entering Agent Loop"
            );

            run_agent_loop(
                runtime,
                input,
                ctx,
                config,
                tool_server_handle,
                registered_tools,
                op_token,
                handler,
                memory_config,
                memory_path,
                input_for_memory,
                app_context,
                window_title,
                generation_config,
            );
        }
    }
}

/// Handle direct route cases (slash commands, skills, MCP, custom)
#[allow(clippy::too_many_arguments)]
fn handle_direct_route(
    runtime: &tokio::runtime::Handle,
    input: &str,
    mode: DirectMode,
    config: &crate::agents::RigAgentConfig,
    tool_server_handle: rig::tool::server::ToolServerHandle,
    registered_tools: Arc<std::sync::RwLock<Vec<String>>>,
    op_token: &CancellationToken,
    handler: &Arc<dyn crate::ffi::AetherEventHandler>,
    memory_config: &crate::config::MemoryConfig,
    memory_path: &Option<String>,
    input_for_memory: &str,
    app_context: &Option<String>,
    generation_config: &crate::config::GenerationConfig,
) {
    let tool_descriptions = get_builtin_tool_descriptions(generation_config);
    let conversation_histories = Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));
    let topic_id = None;

    match mode {
        DirectMode::Tool(tool) => {
            info!(tool_id = %tool.tool_id, "Direct tool execution");

            let agent_prompt = AgentModePrompt::with_tools(tool_descriptions.clone())
                .with_generation_config(generation_config)
                .generate();

            let processed_input = format!(
                "{}\n\n---\n\n请使用 {} 工具处理: {}",
                agent_prompt, tool.tool_id, tool.args
            );

            execute_with_agent_manager(
                runtime,
                &processed_input,
                config,
                tool_server_handle,
                registered_tools,
                &conversation_histories,
                &topic_id,
                None,
                op_token,
                handler,
                memory_config,
                memory_path,
                input_for_memory,
                app_context,
                &None,
            );
        }

        DirectMode::Skill(skill) => {
            info!(skill_id = %skill.skill_id, "Direct skill execution");

            let agent_prompt = AgentModePrompt::with_tools(tool_descriptions.clone())
                .with_generation_config(generation_config)
                .generate();

            let processed_input = format!(
                "# Skill: {}\n\n{}\n\n---\n\n{}\n\n---\n\n用户请求: {}",
                skill.display_name, skill.instructions, agent_prompt, skill.args
            );

            execute_with_agent_manager(
                runtime,
                &processed_input,
                config,
                tool_server_handle,
                registered_tools,
                &conversation_histories,
                &topic_id,
                None,
                op_token,
                handler,
                memory_config,
                memory_path,
                input_for_memory,
                app_context,
                &None,
            );
        }

        DirectMode::Mcp(mcp) => {
            info!(server_name = %mcp.server_name, "Direct MCP execution");

            let agent_prompt = AgentModePrompt::with_tools(tool_descriptions.clone())
                .with_generation_config(generation_config)
                .generate();

            let tool_hint = if let Some(ref tool) = mcp.tool_name {
                format!(
                    "请使用 {} 工具（来自 {} 服务器）处理: {}",
                    tool, mcp.server_name, mcp.args
                )
            } else {
                format!(
                    "请使用 {} MCP 服务器的工具处理: {}",
                    mcp.server_name, mcp.args
                )
            };

            let processed_input = format!("{}\n\n---\n\n{}", agent_prompt, tool_hint);

            execute_with_agent_manager(
                runtime,
                &processed_input,
                config,
                tool_server_handle,
                registered_tools,
                &conversation_histories,
                &topic_id,
                None,
                op_token,
                handler,
                memory_config,
                memory_path,
                input_for_memory,
                app_context,
                &None,
            );
        }

        DirectMode::Custom(custom) => {
            info!(command_name = %custom.command_name, "Direct custom command execution");

            let prompt = custom.system_prompt.unwrap_or_else(|| {
                AgentModePrompt::with_tools(tool_descriptions.clone())
                    .with_generation_config(generation_config)
                    .generate()
            });

            let processed_input = format!("{}\n\n---\n\n用户输入: {}", prompt, input);

            execute_with_agent_manager(
                runtime,
                &processed_input,
                config,
                tool_server_handle,
                registered_tools,
                &conversation_histories,
                &topic_id,
                None,
                op_token,
                handler,
                memory_config,
                memory_path,
                input_for_memory,
                app_context,
                &None,
            );
        }
    }
}

/// Run the Agent Loop for tasks requiring LLM thinking
#[allow(clippy::too_many_arguments)]
fn run_agent_loop(
    runtime: &tokio::runtime::Handle,
    input: &str,
    ctx: ThinkingContext,
    config: &crate::agents::RigAgentConfig,
    _tool_server_handle: rig::tool::server::ToolServerHandle,
    _registered_tools: Arc<std::sync::RwLock<Vec<String>>>,
    op_token: &CancellationToken,
    handler: &Arc<dyn crate::ffi::AetherEventHandler>,
    memory_config: &crate::config::MemoryConfig,
    memory_path: &Option<String>,
    input_for_memory: &str,
    app_context: &Option<String>,
    window_title: &Option<String>,
    generation_config: &crate::config::GenerationConfig,
) {
    // Check if already cancelled
    if op_token.is_cancelled() {
        handler.on_error("Operation cancelled".to_string());
        return;
    }

    // Get available tools for the loop
    let tool_descriptions = get_builtin_tool_descriptions(generation_config);
    let tools: Vec<crate::dispatcher::UnifiedTool> = tool_descriptions
        .iter()
        .map(|td| {
            crate::dispatcher::UnifiedTool::new(
                &format!("builtin:{}", td.name),
                &td.name,
                &td.description,
                crate::dispatcher::ToolSource::Native,
            )
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
        request_context.metadata.insert("category_hint".to_string(), format!("{:?}", hint));
    }

    // Create loop config
    let loop_config = LoopConfig::default()
        .with_max_steps(20)
        .with_max_tokens(100_000);

    // Create components
    let provider_registry = Arc::new(SingleProviderRegistry::new(provider.clone()));
    let thinker = Arc::new(Thinker::new(provider_registry, ThinkerConfig::default()));

    // Create a simple executor that delegates to the existing RigAgentManager
    // For now, use a placeholder that will be replaced with proper SingleStepExecutor
    let executor = Arc::new(PlaceholderExecutor);

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
                input.to_string(),
                request_context,
                tools,
                &callback,
                Some(abort_rx),
            )
            .await
    });

    // Handle result
    match result {
        LoopResult::Completed { summary, steps, .. } => {
            info!(steps = steps, "Agent Loop completed");

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

            handler.on_complete(summary);
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

/// Create an AI provider from config
fn create_provider_from_config(
    config: &crate::agents::RigAgentConfig,
) -> Result<Arc<dyn crate::providers::AiProvider>, String> {
    use crate::config::ProviderConfig;

    let provider_config = ProviderConfig {
        provider_type: Some(config.provider.clone()),
        api_key: config.api_key.clone(),
        model: config.model.clone(),
        base_url: config.base_url.clone(),
        color: "#808080".to_string(), // Default gray
        timeout_seconds: 30,
        enabled: true,
        max_tokens: Some(config.max_tokens),
        temperature: Some(config.temperature),
        top_p: None,
        top_k: None,
        frequency_penalty: None,
        presence_penalty: None,
        stop_sequences: None,
        thinking_level: None,
        media_resolution: None,
        repeat_penalty: None,
        system_prompt_mode: None,
    };

    crate::providers::create_provider(&config.provider, provider_config)
        .map_err(|e| e.to_string())
}

/// Placeholder executor for Agent Loop
/// TODO: Replace with proper SingleStepExecutor once ToolRegistry is implemented
struct PlaceholderExecutor;

#[async_trait::async_trait]
impl crate::agent_loop::ExecutorTrait for PlaceholderExecutor {
    async fn execute(&self, action: &crate::agent_loop::Action) -> crate::agent_loop::ActionResult {
        match action {
            crate::agent_loop::Action::ToolCall { tool_name, .. } => {
                // For now, return a placeholder result
                // TODO: Integrate with actual tool execution via ToolServerHandle
                warn!(tool_name = %tool_name, "PlaceholderExecutor: tool execution not implemented");
                crate::agent_loop::ActionResult::ToolError {
                    error: "Tool execution not yet implemented in Agent Loop".to_string(),
                    retryable: false,
                }
            }
            crate::agent_loop::Action::UserInteraction { question, .. } => {
                crate::agent_loop::ActionResult::UserResponse {
                    response: format!("Awaiting response for: {}", question),
                }
            }
            crate::agent_loop::Action::Completion { .. } => {
                crate::agent_loop::ActionResult::Completed
            }
            crate::agent_loop::Action::Failure { .. } => crate::agent_loop::ActionResult::Failed,
        }
    }
}
