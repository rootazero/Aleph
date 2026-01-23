//! Processing methods for AetherCore
//!
//! This module contains AI processing methods: process, cancel, generate_topic_title, extract_text
//!
//! # Architecture
//!
//! Uses `IntentRouter` + `AgentLoop` with observe-think-act cycle:
//! - L0-L2: Fast routing via IntentRouter (slash commands, patterns, context)
//! - Agent Loop: LLM-based thinking for complex tasks

use super::{AetherCore, AetherFfiError};
use crate::agents::RigAgentManager;
use crate::command::CommandParser;
use crate::config::RoutingRuleConfig;
use crate::intent::{AgentModePrompt, ToolDescription};
use crate::memory::{ContextAnchor, EmbeddingModel, MemoryIngestion, VectorDatabase};
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
use crate::executor::{BuiltinToolRegistry, SingleStepExecutor};
use crate::ffi::FfiLoopCallback;
use crate::intent::{DirectMode, IntentRouter, RouteResult, ThinkingContext};
use crate::runtimes::{RuntimeCapability, RuntimeRegistry};
use crate::thinker::{SingleProviderRegistry, Thinker, ThinkerConfig};

// DAG scheduler imports
use crate::dispatcher::{
    AnalysisResult, DagScheduler, ExecutionCallback, TaskAnalyzer, TaskContext,
    DagTaskDisplayStatus, DagTaskPlan, TaskOutput, UserDecision,
};
use crate::dispatcher::agent_types::{Task, TaskGraph};
use crate::dispatcher::scheduler::GraphTaskExecutor;
use crate::dispatcher::MAX_PARALLELISM;

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
        let (generation_config, routing_rules) = {
            let full_config = self.full_config.lock().unwrap_or_else(|e| e.into_inner());
            (full_config.generation.clone(), full_config.rules.clone())
        };

        // Clone generation registry for DAG execution of generation tasks
        let generation_registry = Arc::clone(&self.generation_registry);

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

            // Process via Agent Loop (observe-think-act cycle)
            info!("Processing via Agent Loop");
            process_with_agent_loop(
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
                &generation_registry,
            );
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
// Agent Loop Processing
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
    attachments: Option<&[crate::core::MediaAttachment]>,
    op_token: &CancellationToken,
    handler: &Arc<dyn crate::ffi::AetherEventHandler>,
    memory_config: &crate::config::MemoryConfig,
    memory_path: &Option<String>,
    input_for_memory: &str,
    generation_config: &crate::config::GenerationConfig,
    routing_rules: &[RoutingRuleConfig],
    generation_registry: &Arc<std::sync::RwLock<crate::generation::GenerationProviderRegistry>>,
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
                attachments,
            );
        }

        RouteResult::NeedsThinking(ctx) => {
            // Needs thinking - analyze task complexity first
            info!(
                category_hint = ?ctx.category_hint,
                bias_execute = ctx.bias_execute,
                latency_us = ctx.latency_us,
                "Needs thinking - analyzing task complexity"
            );

            // Create provider for TaskAnalyzer
            let provider = match create_provider_from_config(config) {
                Ok(p) => p,
                Err(e) => {
                    error!(error = %e, "Failed to create provider for analysis");
                    handler.on_error(format!("Provider error: {}", e));
                    return;
                }
            };

            let analyzer = TaskAnalyzer::with_generation_config(
                provider.clone(),
                generation_config.clone(),
            );

            // Analyze input
            let analysis_result = runtime.block_on(async { analyzer.analyze(input).await });

            match analysis_result {
                Ok(AnalysisResult::SingleStep { intent }) => {
                    info!(intent = %intent, "Single-step task - using Agent Loop");
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
                        attachments,
                    );
                }
                Ok(AnalysisResult::MultiStep {
                    task_graph,
                    requires_confirmation,
                }) => {
                    info!(
                        tasks = task_graph.tasks.len(),
                        requires_confirmation,
                        "Multi-step task - using DAG scheduler"
                    );

                    // Build input with attachment content for DAG context
                    let dag_input = if let Some(attachment_text) =
                        extract_attachment_text(attachments)
                    {
                        info!(
                            attachment_len = attachment_text.len(),
                            "Including attachment text in DAG context"
                        );
                        format!("{}\n\n{}", input, attachment_text)
                    } else {
                        input.to_string()
                    };

                    run_dag_execution(
                        runtime,
                        task_graph,
                        requires_confirmation,
                        provider,
                        op_token,
                        handler,
                        &dag_input,
                        generation_registry,
                        None, // Use default max_task_retries from SchedulerConfig
                    );
                }
                Err(e) => {
                    warn!(error = %e, "Task analysis failed, falling back to Agent Loop");
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
                        attachments,
                    );
                }
            }
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
    attachments: Option<&[crate::core::MediaAttachment]>,
) {
    // Extract attachment text if present
    let attachment_text = extract_attachment_text(attachments);
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

            // Include attachment content if present
            let processed_input = if let Some(ref att_text) = attachment_text {
                info!(
                    attachment_len = att_text.len(),
                    "Including attachment text in skill context"
                );
                format!(
                    "# Skill: {}\n\n{}\n\n---\n\n{}\n\n---\n\n用户请求: {}\n\n---\n\n{}",
                    skill.display_name, skill.instructions, agent_prompt, skill.args, att_text
                )
            } else {
                format!(
                    "# Skill: {}\n\n{}\n\n---\n\n{}\n\n---\n\n用户请求: {}",
                    skill.display_name, skill.instructions, agent_prompt, skill.args
                )
            };

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
    attachments: Option<&[crate::core::MediaAttachment]>,
) {
    // Check if already cancelled
    if op_token.is_cancelled() {
        handler.on_error("Operation cancelled".to_string());
        return;
    }

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

    // Create thinker config with runtime capabilities
    let mut thinker_config = ThinkerConfig::default();
    thinker_config.prompt.runtime_capabilities = runtime_capabilities;

    // Create components
    let provider_registry = Arc::new(SingleProviderRegistry::new(provider.clone()));
    let thinker = Arc::new(Thinker::new(provider_registry, thinker_config));

    // Create executor with builtin tool registry
    let tool_registry = Arc::new(BuiltinToolRegistry::new());
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
        timeout_seconds: config.timeout_seconds, // Use config timeout instead of hardcoded value
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

// ============================================================================
// DAG Scheduler Integration
// ============================================================================

use crate::dispatcher::agent_types::TaskType;
use crate::generation::{
    GenerationParams, GenerationProviderRegistry, GenerationRequest, GenerationType,
};

/// Comprehensive task executor for DAG nodes
///
/// This executor handles different task types:
/// - Generation tasks (image/video/audio): calls the actual generation provider
/// - AI inference tasks: uses LLM completion
/// - Other tasks: uses LLM with task-specific prompts
struct DagTaskExecutor {
    provider: Arc<dyn crate::providers::AiProvider>,
    generation_registry: Arc<std::sync::RwLock<GenerationProviderRegistry>>,
}

impl DagTaskExecutor {
    fn new(
        provider: Arc<dyn crate::providers::AiProvider>,
        generation_registry: Arc<std::sync::RwLock<GenerationProviderRegistry>>,
    ) -> Self {
        Self {
            provider,
            generation_registry,
        }
    }

    /// Execute an image generation task
    async fn execute_image_generation(
        &self,
        task: &Task,
        image_gen: &crate::dispatcher::agent_types::ImageGenTask,
    ) -> crate::error::Result<TaskOutput> {
        info!(
            task_id = %task.id,
            provider = %image_gen.provider,
            model = %image_gen.model,
            "Executing image generation task"
        );

        // Get provider from registry (clone Arc to release lock before await)
        let gen_provider = {
            let registry = self.generation_registry.read().map_err(|e| {
                crate::error::AetherError::config(format!(
                    "Failed to read generation registry: {}",
                    e
                ))
            })?;

            registry.get(&image_gen.provider).ok_or_else(|| {
                crate::error::AetherError::config(format!(
                    "Generation provider '{}' not found in registry",
                    image_gen.provider
                ))
            })?
        };

        // Build generation request with model
        let params = GenerationParams::builder().model(&image_gen.model).build();
        let request = GenerationRequest::image(&image_gen.prompt).with_params(params);

        // Execute generation (lock is released before this point)
        let output = gen_provider.generate(request).await.map_err(|e| {
            crate::error::AetherError::provider(format!("Image generation failed: {}", e))
        })?;

        // Format result
        let result = if let Some(url) = output.data.as_url() {
            format!("✓ 图像生成成功: {}", url)
        } else if let Some(path) = output.data.as_local_path() {
            format!("✓ 图像已保存到: {}", path)
        } else {
            "✓ 图像生成成功".to_string()
        };

        info!(task_id = %task.id, "Image generation completed");
        Ok(TaskOutput::text(result))
    }

    /// Execute a video generation task
    async fn execute_video_generation(
        &self,
        task: &Task,
        video_gen: &crate::dispatcher::agent_types::VideoGenTask,
    ) -> crate::error::Result<TaskOutput> {
        info!(
            task_id = %task.id,
            provider = %video_gen.provider,
            model = %video_gen.model,
            "Executing video generation task"
        );

        // Get provider from registry (clone Arc to release lock before await)
        let gen_provider = {
            let registry = self.generation_registry.read().map_err(|e| {
                crate::error::AetherError::config(format!(
                    "Failed to read generation registry: {}",
                    e
                ))
            })?;

            registry.get(&video_gen.provider).ok_or_else(|| {
                crate::error::AetherError::config(format!(
                    "Generation provider '{}' not found in registry",
                    video_gen.provider
                ))
            })?
        };

        // Build generation request with model
        let params = GenerationParams::builder().model(&video_gen.model).build();
        let request = GenerationRequest::video(&video_gen.prompt).with_params(params);

        // Execute generation (lock is released before this point)
        let output = gen_provider.generate(request).await.map_err(|e| {
            crate::error::AetherError::video(format!("Video generation failed: {}", e))
        })?;

        // Format result
        let result = if let Some(url) = output.data.as_url() {
            format!("✓ 视频生成成功: {}", url)
        } else if let Some(path) = output.data.as_local_path() {
            format!("✓ 视频已保存到: {}", path)
        } else {
            "✓ 视频生成成功".to_string()
        };

        info!(task_id = %task.id, "Video generation completed");
        Ok(TaskOutput::text(result))
    }

    /// Execute an audio generation task
    async fn execute_audio_generation(
        &self,
        task: &Task,
        audio_gen: &crate::dispatcher::agent_types::AudioGenTask,
    ) -> crate::error::Result<TaskOutput> {
        info!(
            task_id = %task.id,
            provider = %audio_gen.provider,
            model = %audio_gen.model,
            "Executing audio generation task"
        );

        // Get provider from registry and determine type (release lock before await)
        let (gen_provider, gen_type) = {
            let registry = self.generation_registry.read().map_err(|e| {
                crate::error::AetherError::config(format!(
                    "Failed to read generation registry: {}",
                    e
                ))
            })?;

            let provider = registry.get(&audio_gen.provider).ok_or_else(|| {
                crate::error::AetherError::config(format!(
                    "Generation provider '{}' not found in registry",
                    audio_gen.provider
                ))
            })?;

            // Determine if this is speech or audio based on provider capabilities
            let gen_type = if provider.supports(GenerationType::Speech) {
                GenerationType::Speech
            } else {
                GenerationType::Audio
            };

            (provider, gen_type)
        };

        // Build generation request with model
        let params = GenerationParams::builder().model(&audio_gen.model).build();
        let request = if gen_type == GenerationType::Speech {
            GenerationRequest::speech(&audio_gen.prompt).with_params(params)
        } else {
            GenerationRequest::audio(&audio_gen.prompt).with_params(params)
        };

        // Execute generation (lock is released before this point)
        let output = gen_provider.generate(request).await.map_err(|e| {
            crate::error::AetherError::provider(format!("Audio generation failed: {}", e))
        })?;

        // Format result
        let result = if let Some(url) = output.data.as_url() {
            format!("✓ 音频生成成功: {}", url)
        } else if let Some(path) = output.data.as_local_path() {
            format!("✓ 音频已保存到: {}", path)
        } else {
            "✓ 音频生成成功".to_string()
        };

        info!(task_id = %task.id, "Audio generation completed");
        Ok(TaskOutput::text(result))
    }

    /// Execute a generic LLM task
    async fn execute_llm_task(
        &self,
        task: &Task,
        context: &str,
    ) -> crate::error::Result<TaskOutput> {
        // Truncate context if too large
        const MAX_CONTEXT_CHARS: usize = 50_000;
        let context_to_use = if context.len() > MAX_CONTEXT_CHARS {
            warn!(
                task_id = %task.id,
                actual_len = context.len(),
                "Context too large, truncating to {} chars",
                MAX_CONTEXT_CHARS
            );
            // Safe UTF-8 truncation
            context.chars().take(MAX_CONTEXT_CHARS).collect()
        } else {
            context.to_string()
        };

        let prompt = format!(
            "{}\n\n请执行以下任务:\n任务: {}\n描述: {}\n\n请直接给出结果。",
            context_to_use,
            task.name,
            task.description.as_deref().unwrap_or("无"),
        );

        let response = self.provider.process(&prompt, None).await?;

        // Check if LLM response indicates missing required input
        if response_needs_user_input(&response) {
            warn!(
                task_id = %task.id,
                task_name = %task.name,
                "LLM response indicates missing required input"
            );
            return Err(crate::error::AetherError::MissingInput {
                task_id: task.id.clone(),
                task_name: task.name.clone(),
                message: response,
            });
        }

        Ok(TaskOutput::text(response))
    }
}

#[async_trait::async_trait]
impl GraphTaskExecutor for DagTaskExecutor {
    async fn execute(&self, task: &Task, context: &str) -> crate::error::Result<TaskOutput> {
        match &task.task_type {
            TaskType::ImageGeneration(image_gen) => {
                self.execute_image_generation(task, image_gen).await
            }
            TaskType::VideoGeneration(video_gen) => {
                self.execute_video_generation(task, video_gen).await
            }
            TaskType::AudioGeneration(audio_gen) => {
                self.execute_audio_generation(task, audio_gen).await
            }
            // For all other task types, use LLM completion
            _ => self.execute_llm_task(task, context).await,
        }
    }
}

/// Adapter to convert ExecutionCallback to AetherEventHandler
///
/// This struct bridges the DAG scheduler's callback interface with
/// the FFI event handler, allowing progress updates to flow to the UI.
struct FfiExecutionCallback {
    handler: Arc<dyn crate::ffi::AetherEventHandler>,
}

impl FfiExecutionCallback {
    fn new(handler: Arc<dyn crate::ffi::AetherEventHandler>) -> Self {
        Self { handler }
    }
}

#[async_trait::async_trait]
impl ExecutionCallback for FfiExecutionCallback {
    async fn on_plan_ready(&self, plan: &DagTaskPlan) {
        // Format plan as markdown for display
        let mut output = format!("**任务计划: {}**\n\n", plan.title);
        for (i, task) in plan.tasks.iter().enumerate() {
            let status = match task.status {
                DagTaskDisplayStatus::Pending => "○",
                DagTaskDisplayStatus::Running => "◉",
                DagTaskDisplayStatus::Completed => "✓",
                DagTaskDisplayStatus::Failed => "✗",
                DagTaskDisplayStatus::Cancelled => "⊘",
            };
            output.push_str(&format!("{} {}. {}\n", status, i + 1, task.name));
        }
        output.push_str("\n---\n\n");
        self.handler.on_stream_chunk(output);
    }

    async fn on_confirmation_required(&self, plan: &DagTaskPlan) -> UserDecision {
        use crate::ffi::plan_confirmation::store_pending_confirmation;
        use std::time::Duration;

        // Generate unique plan_id (use the plan's existing ID)
        let plan_id = plan.id.clone();

        info!(
            plan_id = %plan_id,
            task_count = plan.tasks.len(),
            "Requesting user confirmation for DAG plan"
        );

        // Store pending confirmation and get the receiver
        let receiver = store_pending_confirmation(plan_id.clone(), plan.clone());

        // Notify Swift via callback
        // Swift will show a confirmation dialog and call confirm_task_plan(plan_id, decision)
        self.handler.on_plan_confirmation_required(plan_id.clone(), plan.clone());

        // Stream a message to show we're waiting for confirmation
        self.handler.on_stream_chunk(
            "⏳ 等待用户确认任务计划...\n".to_string()
        );

        // Wait for the decision with timeout
        const CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(30);

        match tokio::time::timeout(CONFIRMATION_TIMEOUT, receiver).await {
            Ok(Ok(decision)) => {
                info!(plan_id = %plan_id, decision = ?decision, "Received user confirmation");
                decision
            }
            Ok(Err(_)) => {
                // Channel closed without sending - treat as cancelled
                warn!(plan_id = %plan_id, "Confirmation channel closed");
                self.handler.on_stream_chunk(
                    "⚠️ 确认已取消（内部错误）\n".to_string()
                );
                UserDecision::Cancelled
            }
            Err(_) => {
                // Timeout - treat as cancelled
                warn!(plan_id = %plan_id, "Confirmation timed out after {:?}", CONFIRMATION_TIMEOUT);
                self.handler.on_stream_chunk(
                    format!("⚠️ 确认超时（{}秒），任务已取消\n", CONFIRMATION_TIMEOUT.as_secs())
                );
                UserDecision::Cancelled
            }
        }
    }

    async fn on_task_start(&self, _task_id: &str, task_name: &str) {
        self.handler
            .on_stream_chunk(format!("\n**[开始]** {}\n", task_name));
    }

    async fn on_task_stream(&self, _task_id: &str, chunk: &str) {
        self.handler.on_stream_chunk(chunk.to_string());
    }

    async fn on_task_complete(&self, _task_id: &str, summary: &str) {
        self.handler.on_stream_chunk(format!("\n✓ {}\n", summary));
    }

    async fn on_task_retry(&self, task_id: &str, attempt: u32, error: &str) {
        self.handler.on_stream_chunk(format!(
            "\n重试 {} (第{}次): {}\n",
            task_id, attempt, error
        ));
    }

    async fn on_task_deciding(&self, task_id: &str, error: &str) {
        self.handler.on_stream_chunk(format!(
            "\n任务 {} 失败，正在决策...\n错误: {}\n",
            task_id, error
        ));
    }

    async fn on_task_failed(&self, task_id: &str, error: &str) {
        self.handler
            .on_stream_chunk(format!("\n✗ {} 失败: {}\n", task_id, error));
    }

    async fn on_all_complete(&self, summary: &str) {
        self.handler
            .on_stream_chunk(format!("\n---\n\n**执行完成**: {}\n", summary));
    }

    async fn on_cancelled(&self) {
        self.handler
            .on_stream_chunk("\n---\n\n**已取消**\n".to_string());
    }
}

/// Run DAG-based multi-step execution
///
/// This function handles multi-step task execution using the DAG scheduler.
/// It creates the necessary components (executor, callback, context) and
/// orchestrates the execution of the task graph.
///
/// # Generation Task Support
///
/// When the task graph contains generation tasks (image/video/audio), this
/// function uses the `generation_registry` to call the actual generation
/// providers instead of just asking the LLM to describe the generation.
#[allow(clippy::too_many_arguments)]
fn run_dag_execution(
    runtime: &tokio::runtime::Handle,
    task_graph: TaskGraph,
    _requires_confirmation: bool,
    provider: Arc<dyn crate::providers::AiProvider>,
    op_token: &CancellationToken,
    handler: &Arc<dyn crate::ffi::AetherEventHandler>,
    user_input: &str,
    generation_registry: &Arc<std::sync::RwLock<GenerationProviderRegistry>>,
    max_task_retries: Option<u32>,
) {
    // Check if already cancelled
    if op_token.is_cancelled() {
        handler.on_error("Operation cancelled".to_string());
        return;
    }

    let handler = handler.clone();
    let user_input = user_input.to_string();
    let generation_registry = Arc::clone(generation_registry);

    // Run DAG execution
    let result = runtime.block_on(async {
        // Create callback adapter
        let callback = Arc::new(FfiExecutionCallback::new(handler.clone()));

        // Create executor with generation capabilities
        let executor = Arc::new(DagTaskExecutor::new(provider, generation_registry));

        // Create context
        let context = TaskContext::new(&user_input);

        // Build scheduler config with custom retries if provided
        let scheduler_config = max_task_retries.map(|retries| {
            crate::dispatcher::SchedulerConfig {
                max_parallelism: MAX_PARALLELISM,
                max_task_retries: retries,
            }
        });

        // Execute graph
        DagScheduler::execute_graph(task_graph, executor, callback, context, scheduler_config).await
    });

    match result {
        Ok(exec_result) => {
            if exec_result.cancelled {
                handler.on_error("用户取消了任务执行".to_string());
                return;
            } else if !exec_result.failed_tasks.is_empty() {
                handler.on_error(format!("部分任务失败: {:?}", exec_result.failed_tasks));
                return;
            }
            // Use detailed_summary to include full task results
            let detailed = exec_result.detailed_summary();
            handler.on_complete(detailed);
        }
        Err(e) => {
            handler.on_error(format!("DAG 执行失败: {}", e));
        }
    }
}

/// Extract text content from attachments for DAG task context
///
/// This function extracts readable text content from attachments (e.g., markdown files,
/// text files) so that DAG tasks can access the attachment content.
/// Binary attachments (images, PDFs) are skipped.
fn extract_attachment_text(attachments: Option<&[crate::core::MediaAttachment]>) -> Option<String> {
    let attachments = attachments?;
    if attachments.is_empty() {
        return None;
    }

    let mut text_parts = Vec::new();

    for attachment in attachments {
        // Only process text-based attachments
        if attachment.encoding == "base64" {
            match attachment.mime_type.as_str() {
                "text/plain" | "text/markdown" | "text/x-markdown" | "application/json" => {
                    // Decode base64 to text
                    if let Ok(decoded) = base64::Engine::decode(
                        &base64::engine::general_purpose::STANDARD,
                        &attachment.data,
                    ) {
                        if let Ok(text) = String::from_utf8(decoded) {
                            let filename = attachment.filename.as_deref().unwrap_or("attachment");
                            text_parts.push(format!(
                                "=== 附件内容: {} ===\n{}\n=== 附件结束 ===",
                                filename, text
                            ));
                            debug!(filename = filename, "Extracted text from attachment for DAG context");
                        }
                    }
                }
                _ => {
                    // Skip binary attachments (images, PDFs, etc.)
                    debug!(
                        mime_type = %attachment.mime_type,
                        "Skipping binary attachment for DAG context"
                    );
                }
            }
        }
    }

    if text_parts.is_empty() {
        None
    } else {
        Some(text_parts.join("\n\n"))
    }
}

/// Check if LLM response indicates that it needs more user input to complete the task
///
/// This function detects common patterns in LLM responses that indicate:
/// - Missing required information
/// - Request for user to provide data
/// - Inability to proceed without additional input
///
/// When detected, the task should be marked as failed/needs-input rather than completed.
fn response_needs_user_input(response: &str) -> bool {
    // Chinese indicators for "needs input"
    let chinese_indicators = [
        "缺少",           // missing
        "请提供",         // please provide
        "需要你提供",      // need you to provide
        "需要提供",        // need to provide
        "请把",           // please put/give
        "请给出",         // please give
        "没有提供",        // not provided
        "未提供",         // not provided
        "无法完成",        // cannot complete
        "无法执行",        // cannot execute
        "无法继续",        // cannot continue
        "缺失",           // lacking
        "不完整",         // incomplete
        "请输入",         // please input
        "请粘贴",         // please paste
        "你还没有",        // you haven't yet
        "仍未提供",        // still not provided
        "仍然缺少",        // still missing
    ];

    // English indicators for "needs input"
    let english_indicators = [
        "please provide",
        "need you to provide",
        "missing",
        "required input",
        "cannot complete",
        "cannot proceed",
        "unable to",
        "not provided",
        "incomplete",
        "please paste",
        "please input",
        "you haven't",
        "still missing",
    ];

    let lower_response = response.to_lowercase();

    // Check Chinese indicators
    for indicator in &chinese_indicators {
        if response.contains(indicator) {
            debug!(indicator = indicator, "Detected missing input indicator in response");
            return true;
        }
    }

    // Check English indicators
    for indicator in &english_indicators {
        if lower_response.contains(indicator) {
            debug!(indicator = indicator, "Detected missing input indicator in response");
            return true;
        }
    }

    false
}

