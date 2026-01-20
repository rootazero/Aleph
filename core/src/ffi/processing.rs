//! Processing methods for AetherCore
//!
//! This module contains AI processing methods: process, cancel, generate_topic_title, extract_text
//!
//! # Architecture (Simplified 2-Layer)
//!
//! - L1: Slash command check (immediate routing for /agent, /search, etc.)
//! - L3: AI unified planner for everything else (conversational, single action, task graph)

use super::{AetherCore, AetherFfiError};
use crate::agents::RigAgentManager;
use crate::command::{CommandContext, CommandParser, ParsedCommand};
use crate::config::RoutingRuleConfig;
use crate::dispatcher::executor::{
    CodeExecutor, ExecutorRegistry, FileOpsExecutor, PathPermissionChecker,
};
use crate::dispatcher::ToolSourceType;
use crate::executor::{ExecutionContext as ExecContext, ExecutorError, UnifiedExecutor};
use crate::intent::{AgentModePrompt, ToolDescription};
use crate::memory::{ContextAnchor, EmbeddingModel, MemoryIngestion, VectorDatabase};
use crate::orchestrator::{OrchestratorMode, OrchestratorRequest, RequestContext, RequestOrchestrator};
use crate::planner::{ExecutionPlan, PlannerConfig, ToolInfo, UnifiedPlanner};
use crate::prompt::PromptBuilder;
use crate::providers::{create_provider, AiProvider};
use crate::skills::SkillsRegistry;
use crate::utils::paths::get_skills_dir;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

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
        let stream = options.stream;

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
        // Clone full config for creating AI provider
        let (generation_config, routing_rules, full_config_clone) = {
            let full_config = self.full_config.lock().unwrap_or_else(|e| e.into_inner());
            (
                full_config.generation.clone(),
                full_config.rules.clone(),
                full_config.clone(),
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
            // EXPERIMENTAL: RequestOrchestrator-based processing
            // ================================================================
            // Check if the new orchestrator-based processing is enabled
            if full_config_clone.policies.experimental.use_request_orchestrator {
                info!("Using experimental RequestOrchestrator processing");
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
                );
                return;
            }

            // ================================================================
            // L1: SLASH COMMAND CHECK (Legacy Path)
            // ================================================================
            // Only parse slash commands (starting with '/').
            // For non-slash input, use the unified planner (L3).
            let parsed_command = if input.starts_with('/') {
                parse_slash_command(&input, &routing_rules)
            } else {
                None
            };

            // Get tool descriptions for agent prompt (used by multiple branches)
            // Includes generate_image if image providers are configured
            info!(
                generation_providers_count = generation_config.providers.len(),
                generation_providers = ?generation_config.providers.keys().collect::<Vec<_>>(),
                "Checking generation config for tool descriptions"
            );
            let tool_descriptions = get_builtin_tool_descriptions(&generation_config);

            // If slash command is detected, handle it with existing logic
            if let Some(ref cmd) = parsed_command {
                // ================================================================
                // SLASH COMMAND HANDLING (Existing Logic)
                // ================================================================
                let processed_input = match cmd.source_type {
                    ToolSourceType::Builtin => {
                        // Handle builtin commands
                        match cmd.command_name.as_str() {
                            "agent" => {
                                // /agent command - inject agent mode prompt
                                let task_input = cmd.arguments.clone().unwrap_or_default();
                                info!(task = %task_input, "Explicit /agent command detected");

                                let agent_prompt = AgentModePrompt::with_tools(tool_descriptions)
                                    .with_generation_config(&generation_config)
                                    .generate();

                                // Notify UI of agent mode
                                let task = crate::intent::ExecutableTask {
                                    category: crate::intent::TaskCategory::General,
                                    action: task_input.clone(),
                                    target: None,
                                    confidence: 1.0,
                                };
                                handler.on_agent_mode_detected((&task).into());

                                format!("{}\n\n---\n\n用户请求: {}", agent_prompt, task_input)
                            }
                            "search" | "youtube" | "webfetch" => {
                                // Other builtin tools - inject tool trigger prompt
                                let tool_name = &cmd.command_name;
                                let args = cmd.arguments.clone().unwrap_or_default();
                                info!(tool = %tool_name, args = %args, "Builtin tool command");

                                // Inject agent prompt with tool hint
                                let agent_prompt =
                                    AgentModePrompt::with_tools(tool_descriptions.clone())
                                        .with_generation_config(&generation_config)
                                        .generate();
                                let tool_hint = match tool_name.as_str() {
                                    "search" => format!("请使用 search 工具搜索以下内容: {}", args),
                                    "youtube" => {
                                        format!("请使用 youtube 工具获取以下视频信息: {}", args)
                                    }
                                    "webfetch" => {
                                        format!("请使用 web_fetch 工具获取以下网页内容: {}", args)
                                    }
                                    _ => args,
                                };

                                format!("{}\n\n---\n\n用户请求: {}", agent_prompt, tool_hint)
                            }
                            _ => {
                                // Unknown builtin - treat as regular input
                                input.clone()
                            }
                        }
                    }
                    ToolSourceType::Skill => {
                        // Handle skill commands - inject skill instructions
                        if let CommandContext::Skill {
                            skill_id,
                            instructions,
                            display_name,
                        } = &cmd.context
                        {
                            let user_input = cmd.arguments.clone().unwrap_or_default();
                            info!(
                                skill_id = %skill_id,
                                skill_name = %display_name,
                                "Skill command detected"
                            );

                            // Inject skill instructions as system context
                            format!(
                                "# Skill: {}\n\n{}\n\n---\n\n用户请求: {}",
                                display_name, instructions, user_input
                            )
                        } else {
                            input.clone()
                        }
                    }
                    ToolSourceType::Custom => {
                        // Handle custom commands - inject system prompt
                        if let CommandContext::Custom {
                            system_prompt,
                            provider: _,
                            pattern: _,
                        } = &cmd.context
                        {
                            let user_input = cmd.arguments.clone().unwrap_or_default();
                            info!(
                                command = %cmd.command_name,
                                "Custom command detected"
                            );

                            if let Some(prompt) = system_prompt {
                                format!("{}\n\n---\n\n用户输入: {}", prompt, user_input)
                            } else {
                                user_input
                            }
                        } else {
                            input.clone()
                        }
                    }
                    ToolSourceType::Mcp => {
                        // Handle MCP commands - inject tool trigger
                        if let CommandContext::Mcp {
                            server_name,
                            tool_name: _,
                        } = &cmd.context
                        {
                            let args = cmd.arguments.clone().unwrap_or_default();
                            info!(
                                server = %server_name,
                                args = %args,
                                "MCP command detected"
                            );

                            // Inject agent prompt with MCP tool hint
                            let agent_prompt =
                                AgentModePrompt::with_tools(tool_descriptions.clone())
                                    .with_generation_config(&generation_config)
                                    .generate();
                            format!(
                                "{}\n\n---\n\n请使用 {} 工具处理: {}",
                                agent_prompt, server_name, args
                            )
                        } else {
                            input.clone()
                        }
                    }
                    ToolSourceType::Native => {
                        // Legacy native tools - treat as regular input
                        input.clone()
                    }
                };

                // Execute with RigAgentManager (existing path for slash commands)
                execute_with_agent_manager(
                    &runtime,
                    &processed_input,
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
                    &app_context,
                    &window_title,
                );
            } else {
                // ================================================================
                // L3: UNIFIED PLANNER + EXECUTOR (New Path)
                // ================================================================
                // No slash command detected - use AI unified planner to decide
                // the execution strategy (conversational, single action, or task graph).
                info!(
                    input_len = input.len(),
                    "Using unified planner for non-slash input"
                );

                // Create AI provider for planner
                let planner_provider = create_planner_provider(&full_config_clone);

                // Try to plan, or fallback to direct agent execution
                let plan_opt = if let Some(provider) = planner_provider {
                    // Create unified planner with available tools
                    let tools = convert_tool_descriptions_to_tool_info(&tool_descriptions);

                    // Log available tools for debugging
                    info!(
                        tools_count = tools.len(),
                        tools = ?tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
                        "Tools available for planner"
                    );

                    let planner_config = PlannerConfig::default();
                    let planner =
                        UnifiedPlanner::with_config(Arc::clone(&provider), planner_config)
                            .with_tools(tools);

                    // Plan the execution
                    let plan_result = runtime.block_on(async {
                        tokio::select! {
                            biased;
                            _ = op_token.cancelled() => {
                                Err(crate::planner::PlannerError::Timeout)
                            }
                            result = planner.plan(&input) => {
                                result
                            }
                        }
                    });

                    match plan_result {
                        Ok(plan) => {
                            info!(plan_type = %plan.plan_type(), "Execution plan generated");
                            Some(plan)
                        }
                        Err(e) => {
                            warn!(error = %e, "Planner failed, falling back to conversational mode");
                            None
                        }
                    }
                } else {
                    warn!("No provider available for planner, using direct agent execution");
                    None
                };

                // Execute plan or fallback to direct agent
                if let Some(plan) = plan_opt {
                    execute_plan(
                        &runtime,
                        plan,
                        &input,
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
                        &app_context,
                        &window_title,
                        stream,
                        &generation_config,
                        &tool_descriptions,
                    );
                } else {
                    // Fallback to direct agent execution
                    execute_with_agent_manager(
                        &runtime,
                        &input,
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
                        &app_context,
                        &window_title,
                    );
                }
            }
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

/// Parse user input as a slash command
///
/// This function creates a CommandParser and attempts to parse slash commands.
/// It only handles explicit slash commands (starting with '/').
/// Natural language command detection has been removed in favor of the unified planner.
///
/// # Arguments
/// * `input` - User input to parse (should start with '/')
/// * `routing_rules` - Routing rules from config (for custom commands)
///
/// # Returns
/// `Some(ParsedCommand)` if input is a recognized slash command, `None` otherwise
fn parse_slash_command(input: &str, routing_rules: &[RoutingRuleConfig]) -> Option<ParsedCommand> {
    // Only process slash commands
    if !input.starts_with('/') {
        return None;
    }

    // Build command parser with all sources
    let mut parser = CommandParser::new();

    // Load skills registry
    let skills_registry = if let Ok(skills_dir) = get_skills_dir() {
        let registry = SkillsRegistry::new(skills_dir);
        if registry.load_all().is_ok() {
            Some(Arc::new(registry))
        } else {
            None
        }
    } else {
        None
    };

    // Add skills registry to parser
    if let Some(ref registry) = skills_registry {
        parser = parser.with_skills_registry(Arc::clone(registry));
    }

    // Pass routing rules from config
    parser = parser.with_routing_rules(routing_rules.to_vec());

    // Parse the input (slash commands only, no NL detection)
    parser.parse(input)
}

/// Create an AI provider for the planner
///
/// Uses the default provider configuration to create an AI provider instance.
fn create_planner_provider(config: &crate::config::Config) -> Option<Arc<dyn AiProvider>> {
    let default_provider_name = config.general.default_provider.as_ref()?;
    let provider_config = config.providers.get(default_provider_name)?;

    match create_provider(default_provider_name, provider_config.clone()) {
        Ok(provider) => Some(provider),
        Err(e) => {
            warn!(error = %e, "Failed to create provider for planner");
            None
        }
    }
}

/// Convert ToolDescription to ToolInfo for the planner
fn convert_tool_descriptions_to_tool_info(descriptions: &[ToolDescription]) -> Vec<ToolInfo> {
    descriptions
        .iter()
        .map(|d| ToolInfo::new(&d.name, &d.description))
        .collect()
}

/// Execute a plan using the unified executor or fallback to agent manager
#[allow(clippy::too_many_arguments)]
fn execute_plan(
    runtime: &tokio::runtime::Handle,
    plan: ExecutionPlan,
    original_input: &str,
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
    _stream: bool,
    generation_config: &crate::config::GenerationConfig,
    tool_descriptions: &[ToolDescription],
) {
    match plan {
        ExecutionPlan::Conversational { enhanced_prompt } => {
            // Check for special ASK_*_MODEL markers for media generation
            let model_selection_prompt = match enhanced_prompt.as_deref() {
                Some("ASK_IMAGE_MODEL") => {
                    debug!("Asking user to select image model");
                    Some(build_media_model_selection_prompt(
                        generation_config,
                        original_input,
                        MediaGenerationType::Image,
                    ))
                }
                Some("ASK_VIDEO_MODEL") => {
                    debug!("Asking user to select video model");
                    Some(build_media_model_selection_prompt(
                        generation_config,
                        original_input,
                        MediaGenerationType::Video,
                    ))
                }
                Some("ASK_AUDIO_MODEL") => {
                    debug!("Asking user to select audio model");
                    Some(build_media_model_selection_prompt(
                        generation_config,
                        original_input,
                        MediaGenerationType::Audio,
                    ))
                }
                Some("ASK_SPEECH_MODEL") => {
                    debug!("Asking user to select speech model");
                    Some(build_media_model_selection_prompt(
                        generation_config,
                        original_input,
                        MediaGenerationType::Speech,
                    ))
                }
                _ => None,
            };

            if let Some(model_list) = model_selection_prompt {
                execute_with_agent_manager(
                    runtime,
                    &model_list,
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
                    window_title,
                );
            } else {
                // Conversational plan - use agent manager for direct response
                let processed_input = enhanced_prompt.unwrap_or_else(|| original_input.to_string());
                debug!("Executing conversational plan");

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
                    window_title,
                );
            }
        }
        ExecutionPlan::SingleAction {
            tool_name,
            parameters,
            requires_confirmation,
        } => {
            // Single action plan - inject tool call instruction and use agent manager
            info!(tool_name = %tool_name, requires_confirmation = requires_confirmation, "Executing single action plan");

            if requires_confirmation {
                // TODO: Add confirmation UI callback
                handler.on_confirmation_required(format!("Execute tool '{}'?", tool_name));
            }

            // Build prompt that instructs the agent to use the specific tool
            let agent_prompt = AgentModePrompt::with_tools(tool_descriptions.to_vec())
                .with_generation_config(generation_config)
                .generate();

            let params_str = serde_json::to_string_pretty(&parameters).unwrap_or_default();
            let processed_input = format!(
                "{}\n\n---\n\n用户请求: {}\n\n请使用 {} 工具，参数如下:\n{}",
                agent_prompt, original_input, tool_name, params_str
            );

            debug!(
                tool_name = %tool_name,
                parameters = %params_str,
                processed_input_len = processed_input.len(),
                "SingleAction: sending request to agent manager"
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
                window_title,
            );
        }
        ExecutionPlan::TaskGraph {
            tasks,
            dependencies,
            requires_confirmation,
        } => {
            // Task graph plan - use unified executor for multi-step execution
            info!(
                task_count = tasks.len(),
                dependency_count = dependencies.len(),
                requires_confirmation = requires_confirmation,
                "Executing task graph plan"
            );

            if requires_confirmation {
                // TODO: Add confirmation UI callback
                let task_names: Vec<_> = tasks.iter().map(|t| t.description.as_str()).collect();
                handler.on_confirmation_required(format!(
                    "Execute {} tasks?\n{}",
                    tasks.len(),
                    task_names.join("\n")
                ));
            }

            // Create agent manager for executor
            let manager = RigAgentManager::with_shared_handle(
                config.clone(),
                tool_server_handle,
                registered_tools,
            );
            let agent_manager = Arc::new(manager);

            // Create executor registry with file ops and code executors
            let mut executor_registry = ExecutorRegistry::new();

            // Register FileOpsExecutor
            let file_ops_executor = FileOpsExecutor::with_defaults();
            executor_registry.register("file_ops", Arc::new(file_ops_executor));

            // Register CodeExecutor with default settings
            let permission_checker = PathPermissionChecker::default();
            let code_executor = CodeExecutor::new(
                true,               // enabled
                "bash".to_string(), // default_runtime
                300,                // timeout_seconds
                false,              // sandbox_enabled (for now)
                vec![],             // allowed_runtimes (all)
                true,               // allow_network
                vec![],             // blocked_commands
                permission_checker,
                None,                                         // working_directory
                vec!["PATH".to_string(), "HOME".to_string()], // pass_env
            );
            executor_registry.register("code_exec", Arc::new(code_executor));

            let executor_registry = Arc::new(executor_registry);

            // Create unified executor
            let executor =
                UnifiedExecutor::new(agent_manager, executor_registry, Arc::clone(handler));

            // Build execution context
            let exec_context = ExecContext::new().with_stream(true);
            let exec_context = if let Some(ref app) = app_context {
                exec_context.with_app_context(app.clone())
            } else {
                exec_context
            };
            let exec_context = if let Some(ref title) = window_title {
                exec_context.with_window_title(title.clone())
            } else {
                exec_context
            };
            let exec_context = if let Some(ref id) = topic_id {
                exec_context.with_topic_id(id.clone())
            } else {
                exec_context
            };

            // Execute the task graph
            let result = runtime.block_on(async {
                tokio::select! {
                    biased;
                    _ = op_token.cancelled() => {
                        Err(ExecutorError::Cancelled)
                    }
                    result = executor.execute_task_graph(tasks, dependencies, exec_context) => {
                        result
                    }
                }
            });

            // Handle result
            match result {
                Ok(exec_result) => {
                    // Store memory if enabled
                    if memory_config.enabled {
                        if let Some(ref db_path) = memory_path {
                            let store_result = runtime.block_on(async {
                                store_memory_after_response(
                                    db_path,
                                    memory_config,
                                    input_for_memory,
                                    &exec_result.content,
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

                    handler.on_complete(exec_result.content);
                }
                Err(e) => {
                    if op_token.is_cancelled() || e.is_cancelled() {
                        handler.on_error("Operation cancelled".to_string());
                    } else {
                        error!(error = %e, "Task graph execution failed");
                        handler.on_error(e.to_string());
                    }
                }
            }
        }
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

/// Media generation type for model selection
#[derive(Debug, Clone, Copy)]
enum MediaGenerationType {
    Image,
    Video,
    Audio,
    Speech,
}

impl MediaGenerationType {
    /// Get the display name for this media type
    fn display_name(&self) -> &'static str {
        match self {
            MediaGenerationType::Image => "image",
            MediaGenerationType::Video => "video",
            MediaGenerationType::Audio => "audio/music",
            MediaGenerationType::Speech => "speech/TTS",
        }
    }

    /// Get the tool name for this media type
    #[allow(dead_code)]
    fn tool_name(&self) -> &'static str {
        match self {
            MediaGenerationType::Image => "generate_image",
            MediaGenerationType::Video => "generate_video",
            MediaGenerationType::Audio => "generate_audio",
            MediaGenerationType::Speech => "generate_speech",
        }
    }

    /// Convert to crate::generation::GenerationType
    fn to_generation_type(self) -> crate::generation::GenerationType {
        match self {
            MediaGenerationType::Image => crate::generation::GenerationType::Image,
            MediaGenerationType::Video => crate::generation::GenerationType::Video,
            MediaGenerationType::Audio => crate::generation::GenerationType::Audio,
            MediaGenerationType::Speech => crate::generation::GenerationType::Speech,
        }
    }

    /// Get common aliases for this media type's providers
    fn get_aliases(&self) -> &'static [(&'static str, &'static [&'static str])] {
        match self {
            MediaGenerationType::Image => &[
                (
                    "t8star-image",
                    &["nanobanana", "nano-banana", "nano banana"],
                ),
                ("midjourney", &["mj", "MJ"]),
                ("dalle", &["dall-e", "DALL-E", "dall·e"]),
                ("stability", &["stable diffusion", "sd", "SD"]),
                ("flux", &["FLUX"]),
                ("ideogram", &["ideo"]),
            ],
            MediaGenerationType::Video => &[
                ("runway", &["runwayml", "gen-3"]),
                ("pika", &["pika labs"]),
                ("sora", &[]),
                ("kling", &[]),
            ],
            MediaGenerationType::Audio => &[("suno", &[]), ("udio", &[]), ("mubert", &[])],
            MediaGenerationType::Speech => {
                &[("elevenlabs", &["11labs"]), ("openai-tts", &["openai tts"])]
            }
        }
    }

    /// Get example usage phrase for this media type
    fn example_phrase(&self) -> &'static str {
        match self {
            MediaGenerationType::Image => "Use nanobanana to draw a cat",
            MediaGenerationType::Video => "Use runway to create a video of flying birds",
            MediaGenerationType::Audio => "Use suno to generate a jazz tune",
            MediaGenerationType::Speech => "Use elevenlabs to read this aloud",
        }
    }
}

/// Build a prompt asking user to select a media generation model
///
/// When user requests media generation without specifying a model,
/// this function generates a response listing available models.
fn build_media_model_selection_prompt(
    generation_config: &crate::config::GenerationConfig,
    original_input: &str,
    media_type: MediaGenerationType,
) -> String {
    // Get available providers for this media type
    let providers: Vec<(&str, &crate::config::GenerationProviderConfig)> =
        generation_config.get_providers_for_type(media_type.to_generation_type());

    let type_name = media_type.display_name();

    if providers.is_empty() {
        return format!(
            "You want to generate {}, but no {} generation models are configured. \
             Please configure a provider with {} capability in your config.toml first.\n\n\
             Original request: {}",
            type_name, type_name, type_name, original_input
        );
    }

    // Build model list with common aliases
    let mut model_list = format!(
        "## {} Generation Model Selection\n\n",
        type_name.to_uppercase()
    );
    model_list.push_str(&format!(
        "I detected you want to generate {}. Please select which model to use:\n\n",
        type_name
    ));
    model_list.push_str("**Available Models:**\n");

    // Get aliases for this media type
    let aliases = media_type.get_aliases();

    for (idx, (name, config)) in providers.iter().enumerate() {
        let model_name = config.model.as_deref().unwrap_or("default");

        // Find aliases for this provider
        let provider_aliases: Vec<&str> = aliases
            .iter()
            .find(|(prov, _)| prov == name)
            .map(|(_, a)| a.to_vec())
            .unwrap_or_default();

        let alias_str = if provider_aliases.is_empty() {
            String::new()
        } else {
            format!(" (aliases: {})", provider_aliases.join(", "))
        };

        model_list.push_str(&format!(
            "{}. **{}**{} - model: {}\n",
            idx + 1,
            name,
            alias_str,
            model_name
        ));
    }

    model_list.push_str("\n**How to use:**\n");
    model_list.push_str(&format!(
        "- Reply with the model name or number (e.g., \"1\" or \"{}\")\n",
        providers.first().map(|(n, _)| *n).unwrap_or("provider")
    ));
    model_list.push_str(&format!(
        "- Or rephrase your request with the model name (e.g., \"{}\")\n",
        media_type.example_phrase()
    ));
    model_list.push_str(&format!("\n**Your original request:** {}", original_input));

    // Wrap as a system instruction for the agent to respond with this
    format!(
        "Please respond to the user with this message, asking them to select a {} generation model:\n\n{}",
        type_name, model_list
    )
}

// ============================================================================
// EXPERIMENTAL: RequestOrchestrator-based processing
// ============================================================================

/// Process input using the new RequestOrchestrator (experimental)
///
/// This function uses the two-phase architecture:
/// - Phase 1: ExecutionIntentDecider decides "execute vs converse"
/// - Phase 2: Dispatcher decides "which tool and model" (only for Execute mode)
///
/// Feature flag: `experimental.use_request_orchestrator`
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
) {
    // Create the orchestrator
    let orchestrator = RequestOrchestrator::new();

    // Build context from FFI options
    let context = RequestContext::from_ffi_options(app_context.clone(), None);

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
