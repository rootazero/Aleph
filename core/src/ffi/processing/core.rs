//! Core AetherCore processing methods

use crate::ffi::{AetherCore, AetherFfiError};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::info;

use super::orchestration::process_with_agent_loop;
use super::types::ProcessOptions;

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

        // Also cancel any pending user inputs
        crate::ffi::user_input::cancel_all_pending_inputs();
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

    /// Respond to a user input request from the agent loop
    ///
    /// This method is called from Swift after the user provides input in response
    /// to an `on_user_input_request` callback.
    ///
    /// # Arguments
    ///
    /// * `request_id` - The request ID from the `on_user_input_request` callback
    /// * `response` - The user's response text
    ///
    /// # Returns
    ///
    /// `true` if the request was found and completed, `false` if not found.
    pub fn respond_to_user_input(&self, request_id: String, response: String) -> bool {
        info!(
            request_id = %request_id,
            response_len = response.len(),
            "Responding to user input request from FFI"
        );

        crate::ffi::user_input::complete_pending_input(&request_id, response)
    }
}
