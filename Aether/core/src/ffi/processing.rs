//! Processing methods for AetherCore
//!
//! This module contains AI processing methods: process, cancel, generate_topic_title, extract_text

use super::{AetherCore, AetherFfiError};
use crate::agent::RigAgentManager;
use crate::intent::{AgentModePrompt, ExecutionIntent, IntentClassifier, ToolDescription};
use crate::memory::{ContextAnchor, EmbeddingModel, MemoryIngestion, VectorDatabase};
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
            topic_id: None,  // None means "single-turn"
            stream: true,  // Streaming enabled by default
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
        // Extract attachments for multimodal support (currently unused with history-based processing)
        let _attachments = options.attachments.clone();
        // Extract context for memory storage
        let app_context = options.app_context.clone();
        let window_title = options.window_title.clone();
        let topic_id = options.topic_id.clone();

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
        let memory_path = self.memory_path.clone();
        let input_for_memory = input.clone();

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

            // Check for explicit /agent command first (hybrid mode)
            let (is_explicit_agent, task_input) = if input.trim().starts_with("/agent ") {
                // Extract task description after "/agent "
                let task = input.trim().strip_prefix("/agent ").unwrap_or(&input).trim();
                info!(task = %task, "Explicit /agent command detected");
                (true, task.to_string())
            } else if input.trim() == "/agent" {
                // Just "/agent" without task - treat as conversational
                (false, input.clone())
            } else {
                (false, input.clone())
            };

            // Get tool descriptions for agent prompt
            let tool_descriptions = get_builtin_tool_descriptions();

            // Determine agent mode: explicit command OR automatic classification
            let processed_input = if is_explicit_agent {
                // Explicit /agent command - always inject agent prompt with tools
                let agent_prompt = AgentModePrompt::with_tools(tool_descriptions).generate();

                // Create a synthetic ExecutableTask for UI notification
                let task = crate::intent::ExecutableTask {
                    category: crate::intent::TaskCategory::General,
                    action: task_input.clone(),
                    target: None,
                    confidence: 1.0, // Explicit command = full confidence
                };
                handler.on_agent_mode_detected((&task).into());

                format!("{}\n\n---\n\n用户请求: {}", agent_prompt, task_input)
            } else {
                // Automatic classification for non-explicit inputs
                let classifier = IntentClassifier::new();
                let intent = runtime.block_on(classifier.classify(&task_input));
                debug!(intent = ?intent, "Intent classification result");

                if let ExecutionIntent::Executable(ref task) = intent {
                    info!(
                        category = ?task.category,
                        action = %task.action,
                        confidence = task.confidence,
                        "Agent execution mode detected (auto)"
                    );
                    handler.on_agent_mode_detected(task.into());

                    // Inject agent mode prompt with tools to guide AI into execution mode
                    let agent_prompt = AgentModePrompt::with_tools(tool_descriptions).generate();
                    format!("{}\n\n---\n\n用户请求: {}", agent_prompt, task_input)
                } else {
                    task_input.clone()
                }
            };

            // Create manager with shared ToolServerHandle (all tools persist across calls)
            let manager = RigAgentManager::with_shared_handle(config, tool_server_handle, registered_tools);

            // Get or create conversation history for this topic
            let topic_key = topic_id.clone().unwrap_or_else(|| "single-turn".to_string());
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

                    // Process with conversation history for multi-turn support
                    result = manager.process_with_history(&processed_input, &mut history) => {
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
                                    &memory_config,
                                    &input_for_memory,
                                    &response.content,
                                    app_context.as_deref(),
                                    window_title.as_deref(),
                                    topic_id.as_deref(),
                                ).await
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
                    Some(cfg) => {
                        match crate::providers::create_provider(name, cfg.clone()) {
                            Ok(p) => Some(p),
                            Err(e) => {
                                info!(error = %e, "Failed to create provider for title generation");
                                None
                            }
                        }
                    }
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
                let result: Result<String, crate::error::AetherError> =
                    self.runtime.block_on(async move { p.process(&prompt, None).await });

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
        let config = self.full_config.lock().unwrap_or_else(|e| e.into_inner()).clone();

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
    let db = VectorDatabase::new(PathBuf::from(db_path))
        .map_err(|e| crate::error::AetherError::config(format!("Failed to open memory database: {}", e)))?;

    // Create EmbeddingModel
    let model_path = EmbeddingModel::get_default_model_path()
        .map_err(|e| crate::error::AetherError::config(format!("Failed to get model path: {}", e)))?;
    let embedding_model = EmbeddingModel::new(model_path)
        .map_err(|e| crate::error::AetherError::config(format!("Failed to create embedding model: {}", e)))?;

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
fn get_builtin_tool_descriptions() -> Vec<ToolDescription> {
    vec![
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
    ]
}
