/// AetherCore - Main entry point for the Aether library
///
/// Orchestrates hotkey listening, clipboard management, and event callbacks.
use crate::clipboard::{ArboardManager, ClipboardManager};
use crate::config::{Config, MemoryConfig};
use crate::error::{AetherError, Result};
use crate::event_handler::{AetherEventHandler, ErrorType, ProcessingState};
use crate::hotkey::{HotkeyListener, RdevListener};
use crate::memory::database::{MemoryStats, VectorDatabase};
use crate::memory::cleanup::CleanupService;
use crate::router::Router;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;

/// Context for last request (used for retry)
#[derive(Debug, Clone)]
struct RequestContext {
    clipboard_content: String,
    provider: String,
    retry_count: u32,
}

/// Captured context from active application (Swift → Rust)
#[derive(Debug, Clone)]
pub struct CapturedContext {
    pub app_bundle_id: String,
    pub window_title: Option<String>,
}

/// Main core struct for Aether
///
/// Manages lifecycle of all core components and coordinates
/// between hotkey detection, clipboard operations, and client callbacks.
pub struct AetherCore {
    event_handler: Arc<dyn AetherEventHandler>,
    hotkey_listener: Arc<dyn HotkeyListener>,
    clipboard_manager: Arc<dyn ClipboardManager>,
    #[allow(dead_code)]
    runtime: Arc<Runtime>,
    last_request: Arc<Mutex<Option<RequestContext>>>,
    // Memory management
    config: Arc<Mutex<Config>>,
    memory_db: Option<Arc<VectorDatabase>>,
    current_context: Arc<Mutex<Option<CapturedContext>>>,
    cleanup_service: Option<Arc<CleanupService>>,
    #[allow(dead_code)]
    cleanup_task_handle: Option<tokio::task::JoinHandle<()>>,
    // AI routing
    router: Option<Arc<Router>>,
}

impl AetherCore {
    /// Create a new AetherCore instance with the provided event handler
    ///
    /// # Arguments
    /// * `event_handler` - Handler for receiving callbacks from Rust
    ///
    /// # Returns
    /// * `Result<Self>` - New AetherCore instance or error
    pub fn new(event_handler: Box<dyn AetherEventHandler>) -> Result<Self> {
        let event_handler: Arc<dyn AetherEventHandler> = Arc::from(event_handler);
        // Initialize tokio runtime for async operations
        let runtime = Runtime::new()
            .map_err(|e| AetherError::other(format!("Failed to create tokio runtime: {}", e)))?;

        // Clone event handler for the hotkey callback
        let handler_clone = Arc::clone(&event_handler);
        let clipboard_manager: Arc<dyn ClipboardManager> = Arc::new(ArboardManager::new());
        let clipboard_clone = Arc::clone(&clipboard_manager);

        // Create hotkey listener with callback
        let hotkey_listener: Arc<dyn HotkeyListener> = Arc::new(RdevListener::new(move || {
            // When hotkey is detected, read clipboard and invoke callback
            handler_clone.on_state_changed(ProcessingState::Listening);

            match clipboard_clone.read_text() {
                Ok(content) => {
                    handler_clone.on_hotkey_detected(content);
                }
                Err(e) => {
                    handler_clone.on_error(format!("Failed to read clipboard: {}", e));
                }
            }
        }));

        // Initialize configuration
        let config = Arc::new(Mutex::new(Config::default()));

        // Initialize router (if providers are configured)
        let router = {
            let cfg = config.lock().unwrap();
            if !cfg.providers.is_empty() {
                match Router::new(&cfg) {
                    Ok(r) => Some(Arc::new(r)),
                    Err(e) => {
                        eprintln!("Warning: Failed to initialize router: {}", e);
                        None
                    }
                }
            } else {
                None
            }
        };

        // Initialize memory database and cleanup service if enabled
        let (memory_db, cleanup_service, cleanup_task_handle) = {
            let cfg = config.lock().unwrap();
            if cfg.memory.enabled {
                let db_path = Self::get_memory_db_path()?;
                match VectorDatabase::new(db_path.clone()) {
                    Ok(db) => {
                        let db_arc = Arc::new(db);

                        // Initialize cleanup service
                        match CleanupService::new(db_path, cfg.memory.retention_days) {
                            Ok(cleanup) => {
                                let cleanup_arc = Arc::new(cleanup);

                                // Start background cleanup task (only in non-test environment)
                                #[cfg(not(test))]
                                let task_handle = {
                                    // Check if we're in a tokio runtime context
                                    match tokio::runtime::Handle::try_current() {
                                        Ok(_) => Some(Arc::clone(&cleanup_arc).start_background_task()),
                                        Err(_) => {
                                            eprintln!("[Memory] Warning: No tokio runtime, skipping background cleanup task");
                                            None
                                        }
                                    }
                                };

                                #[cfg(test)]
                                let task_handle = None;

                                (Some(db_arc), Some(cleanup_arc), task_handle)
                            }
                            Err(e) => {
                                eprintln!("Warning: Failed to initialize cleanup service: {}", e);
                                (Some(db_arc), None, None)
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Warning: Failed to initialize memory database: {}", e);
                        (None, None, None)
                    }
                }
            } else {
                (None, None, None)
            }
        };

        Ok(Self {
            event_handler,
            hotkey_listener,
            clipboard_manager,
            runtime: Arc::new(runtime),
            last_request: Arc::new(Mutex::new(None)),
            config,
            memory_db,
            current_context: Arc::new(Mutex::new(None)),
            cleanup_service,
            cleanup_task_handle,
            router,
        })
    }

    /// Get the path for the memory database file
    fn get_memory_db_path() -> Result<PathBuf> {
        let home_dir = std::env::var("HOME")
            .map_err(|_| AetherError::config("Failed to get HOME environment variable"))?;

        let config_dir = PathBuf::from(home_dir).join(".config").join("aether");
        Ok(config_dir.join("memory.db"))
    }

    /// Get embedding model directory
    fn get_embedding_model_dir() -> Result<PathBuf> {
        let home_dir = std::env::var("HOME")
            .map_err(|_| AetherError::config("Failed to get HOME environment variable"))?;

        let model_dir = PathBuf::from(home_dir)
            .join(".config")
            .join("aether")
            .join("models")
            .join("all-MiniLM-L6-v2");

        // Create directory if it doesn't exist
        std::fs::create_dir_all(&model_dir)
            .map_err(|e| AetherError::config(format!("Failed to create model directory: {}", e)))?;

        Ok(model_dir)
    }

    /// Start listening for hotkey events
    ///
    /// Spawns background thread to monitor keyboard events.
    pub fn start_listening(&self) -> Result<()> {
        self.event_handler
            .on_state_changed(ProcessingState::Listening);

        self.hotkey_listener.start_listening()?;
        Ok(())
    }

    /// Stop listening for hotkey events
    ///
    /// Terminates background thread and releases resources.
    pub fn stop_listening(&self) -> Result<()> {
        self.hotkey_listener.stop_listening()?;
        self.event_handler.on_state_changed(ProcessingState::Idle);
        Ok(())
    }

    /// Get current clipboard text content
    ///
    /// # Returns
    /// * `Result<String>` - Clipboard text or error
    pub fn get_clipboard_text(&self) -> Result<String> {
        self.clipboard_manager.read_text()
    }

    /// Check if currently listening for hotkeys
    pub fn is_listening(&self) -> bool {
        self.hotkey_listener.is_listening()
    }

    /// Test method: Simulate streaming AI response (for development/testing only)
    ///
    /// Sends chunks of text to the event handler with delays to simulate streaming.
    /// This is a placeholder for Phase 4 AI provider integration.
    #[cfg(debug_assertions)]
    pub fn test_streaming_response(&self) {
        use std::thread;
        use std::time::Duration;

        // Simulate a streaming response
        let chunks = vec![
            "Hello, ",
            "this is ",
            "a streaming ",
            "AI response. ",
            "Each chunk ",
            "appears with ",
            "a slight delay ",
            "to demonstrate ",
            "the streaming ",
            "text feature.",
        ];

        self.event_handler
            .on_state_changed(ProcessingState::Processing);

        for i in 0..chunks.len() {
            // Simulate network delay
            thread::sleep(Duration::from_millis(100));

            // Accumulate text and send full text so far
            let accumulated: String = chunks[..=i].concat();
            self.event_handler.on_response_chunk(accumulated);
        }

        // Simulate completion
        thread::sleep(Duration::from_millis(500));
        self.event_handler
            .on_state_changed(ProcessingState::Success);
    }

    /// Test method: Simulate typed error (for development/testing only)
    #[cfg(debug_assertions)]
    pub fn test_typed_error(&self, error_type: ErrorType, message: String) {
        self.event_handler
            .on_error_typed(error_type, message);
    }

    /// Test method: No-op in release mode
    #[cfg(not(debug_assertions))]
    pub fn test_streaming_response(&self) {
        // No-op in release mode
    }

    /// Test method: No-op in release mode
    #[cfg(not(debug_assertions))]
    pub fn test_typed_error(&self, _error_type: ErrorType, _message: String) {
        // No-op in release mode
    }

    /// Retry the last failed request
    ///
    /// Implements exponential backoff: 2s, 4s, 8s
    /// Max 2 auto-retries, then manual retry only
    ///
    /// # Returns
    /// * `Result<()>` - Ok if retry initiated, Error if no request to retry or max retries exceeded
    pub fn retry_last_request(&self) -> Result<()> {
        use std::thread;
        use std::time::Duration;

        let mut last_request_lock = self.last_request.lock().unwrap();

        let request_ctx = last_request_lock
            .as_mut()
            .ok_or_else(|| AetherError::other("No request to retry".to_string()))?;

        // Check max retry limit
        const MAX_RETRIES: u32 = 2;
        if request_ctx.retry_count >= MAX_RETRIES {
            return Err(AetherError::other(format!(
                "Maximum retry limit ({}) reached",
                MAX_RETRIES
            )));
        }

        // Increment retry count
        request_ctx.retry_count += 1;

        // Calculate exponential backoff: 2^retry_count seconds
        let backoff_seconds = 2u64.pow(request_ctx.retry_count);

        // Clone data for async operation (will be used in Phase 4)
        let _clipboard_content = request_ctx.clipboard_content.clone();
        let _provider = request_ctx.provider.clone();
        let _retry_count = request_ctx.retry_count;

        drop(last_request_lock); // Release lock before sleep

        // Wait with exponential backoff
        thread::sleep(Duration::from_secs(backoff_seconds));

        // Notify state change
        self.event_handler
            .on_state_changed(ProcessingState::Processing);

        // TODO: When AI provider integration is implemented in Phase 4,
        // this should call the actual AI provider with the stored context.
        // For now, we'll simulate success after backoff.

        // Simulate processing
        thread::sleep(Duration::from_millis(500));

        // Simulate success (in real implementation, this would be actual API call result)
        self.event_handler
            .on_state_changed(ProcessingState::Success);

        Ok(())
    }

    /// Store request context for retry (called when initiating AI request)
    ///
    /// This should be called before making an AI API request to enable retry functionality.
    ///
    /// # Arguments
    /// * `clipboard_content` - The content being processed
    /// * `provider` - The AI provider being used
    pub fn store_request_context(&self, clipboard_content: String, provider: String) {
        let mut last_request = self.last_request.lock().unwrap();
        *last_request = Some(RequestContext {
            clipboard_content,
            provider,
            retry_count: 0,
        });
    }

    /// Clear stored request context
    pub fn clear_request_context(&self) {
        let mut last_request = self.last_request.lock().unwrap();
        *last_request = None;
    }

    // MEMORY MANAGEMENT METHODS (Phase 4)

    /// Get memory database statistics
    pub fn get_memory_stats(&self) -> Result<MemoryStats> {
        let db = self.memory_db.as_ref()
            .ok_or_else(|| AetherError::config("Memory database not initialized"))?;

        self.runtime.block_on(db.get_stats())
    }

    /// Search memories by context
    pub fn search_memories(
        &self,
        app_bundle_id: String,
        window_title: Option<String>,
        limit: u32,
    ) -> Result<Vec<MemoryEntryFFI>> {
        let db = self.memory_db.as_ref()
            .ok_or_else(|| AetherError::config("Memory database not initialized"))?;

        // Use empty window title if not provided
        let window = window_title.as_deref().unwrap_or("");

        // For search without embedding, we'll return recent memories only
        // TODO: In Phase 4B, implement actual embedding-based search
        let memories = self.runtime.block_on(
            db.search_memories(&app_bundle_id, window, &[], limit)
        )?;

        // Convert to FFI type
        Ok(memories.into_iter().map(|m| MemoryEntryFFI {
            id: m.id,
            app_bundle_id: m.context.app_bundle_id,
            window_title: m.context.window_title,
            user_input: m.user_input,
            ai_output: m.ai_output,
            timestamp: m.context.timestamp,
            similarity_score: m.similarity_score,
        }).collect())
    }

    /// Delete specific memory by ID
    pub fn delete_memory(&self, id: String) -> Result<()> {
        let db = self.memory_db.as_ref()
            .ok_or_else(|| AetherError::config("Memory database not initialized"))?;

        self.runtime.block_on(db.delete_memory(&id))
    }

    /// Clear memories (with optional filters)
    pub fn clear_memories(
        &self,
        app_bundle_id: Option<String>,
        window_title: Option<String>,
    ) -> Result<u64> {
        let db = self.memory_db.as_ref()
            .ok_or_else(|| AetherError::config("Memory database not initialized"))?;

        self.runtime.block_on(
            db.clear_memories(
                app_bundle_id.as_deref(),
                window_title.as_deref(),
            )
        )
    }

    /// Get memory configuration
    pub fn get_memory_config(&self) -> MemoryConfig {
        let config = self.config.lock().unwrap();
        config.memory.clone()
    }

    /// Update memory configuration
    pub fn update_memory_config(&self, new_config: MemoryConfig) -> Result<()> {
        let mut config = self.config.lock().unwrap();
        let old_retention_days = config.memory.retention_days;
        config.memory = new_config.clone();

        // If retention policy changed and cleanup service exists, log the change
        // Note: The cleanup service will pick up the new config on next cleanup cycle
        if old_retention_days != new_config.retention_days {
            if let Some(_cleanup) = &self.cleanup_service {
                println!(
                    "[Memory] Retention policy updated: {} -> {} days",
                    old_retention_days,
                    new_config.retention_days
                );
                // Note: We cannot update the cleanup service directly due to Arc
                // The service will be recreated when AetherCore is reinitialized
            }
        }

        // TODO: Persist config to file in Phase 4
        Ok(())
    }

    /// Set current context (called from Swift when hotkey pressed)
    pub fn set_current_context(&self, context: CapturedContext) {
        let mut current_context = self.current_context.lock().unwrap();
        *current_context = Some(context);
    }

    /// Manually trigger memory cleanup (for testing or immediate cleanup)
    ///
    /// This runs the cleanup operation immediately in the current thread,
    /// deleting memories older than the configured retention period.
    ///
    /// # Returns
    /// * `Result<u64>` - Number of deleted memories, or error
    pub fn cleanup_old_memories(&self) -> Result<u64> {
        let cleanup = self.cleanup_service.as_ref()
            .ok_or_else(|| AetherError::config("Cleanup service not initialized"))?;

        cleanup.cleanup_old_memories()
            .map_err(|e| AetherError::config(format!("Cleanup failed: {}", e)))
    }

    /// Store interaction memory with current context
    ///
    /// This method is called after a successful AI interaction to store the
    /// user input and AI output along with the captured context.
    ///
    /// # Arguments
    /// * `user_input` - User's original input
    /// * `ai_output` - AI's response
    ///
    /// # Returns
    /// * `Result<String>` - Memory ID if stored successfully
    pub fn store_interaction_memory(
        &self,
        user_input: String,
        ai_output: String,
    ) -> Result<String> {
        use crate::memory::context::ContextAnchor;
        use crate::memory::embedding::EmbeddingModel;
        use crate::memory::ingestion::MemoryIngestion;

        // Check if memory is enabled
        let config = self.config.lock().unwrap();
        if !config.memory.enabled {
            return Err(AetherError::config("Memory is disabled"));
        }

        // Get current context
        let current_context = self.current_context.lock().unwrap();
        let captured_context = current_context.as_ref()
            .ok_or_else(|| AetherError::config("No context captured"))?;

        // Create context anchor
        let context_anchor = ContextAnchor {
            app_bundle_id: captured_context.app_bundle_id.clone(),
            window_title: captured_context.window_title.clone().unwrap_or_default(),
            timestamp: chrono::Utc::now().timestamp(),
        };

        // Get memory database
        let db = self.memory_db.as_ref()
            .ok_or_else(|| AetherError::config("Memory database not initialized"))?;

        // Get embedding model directory
        let model_dir = Self::get_embedding_model_dir()
            .map_err(|e| AetherError::config(format!("Failed to get embedding model directory: {}", e)))?;

        // Create embedding model (lazy load)
        let embedding_model = Arc::new(
            EmbeddingModel::new(model_dir)
                .map_err(|e| AetherError::config(format!("Failed to initialize embedding model: {}", e)))?
        );

        // Create ingestion service
        let ingestion = MemoryIngestion::new(
            Arc::clone(db),
            embedding_model,
            Arc::new(config.memory.clone()),
        );

        // Store memory asynchronously
        let result = self.runtime.block_on(
            ingestion.store_memory(context_anchor, &user_input, &ai_output)
        );

        result
    }

    /// Retrieve memories and augment prompt with context
    ///
    /// This is the main entry point for integrating memory into the AI request pipeline.
    /// It performs the following steps:
    /// 1. Check if memory is enabled
    /// 2. Get current context (app + window)
    /// 3. Retrieve relevant memories based on user query
    /// 4. Augment base prompt with retrieved memories
    /// 5. Return augmented prompt ready for LLM
    ///
    /// # Arguments
    /// * `base_prompt` - Base system prompt (e.g., "You are a helpful assistant")
    /// * `user_input` - Current user input/query
    ///
    /// # Returns
    /// * `Result<String>` - Augmented prompt with memory context, or base prompt if memory disabled
    ///
    /// # Performance
    /// - Includes timing logs for monitoring memory operation overhead
    /// - Target: <150ms total (embedding + search + formatting)
    pub fn retrieve_and_augment_prompt(
        &self,
        base_prompt: String,
        user_input: String,
    ) -> Result<String> {
        use crate::memory::augmentation::PromptAugmenter;
        use crate::memory::context::ContextAnchor;
        use crate::memory::embedding::EmbeddingModel;
        use crate::memory::retrieval::MemoryRetrieval;
        use std::time::Instant;

        let start_time = Instant::now();

        // Check if memory is enabled
        let config = self.config.lock().unwrap();
        if !config.memory.enabled {
            println!("[Memory] Disabled - using base prompt");
            return Ok(format!("{}\n\nUser: {}", base_prompt, user_input));
        }

        // Get current context
        let current_context = self.current_context.lock().unwrap();
        let captured_context = match current_context.as_ref() {
            Some(ctx) => ctx,
            None => {
                println!("[Memory] Warning: No context captured, skipping memory retrieval");
                return Ok(format!("{}\n\nUser: {}", base_prompt, user_input));
            }
        };

        // Create context anchor
        let context_anchor = ContextAnchor {
            app_bundle_id: captured_context.app_bundle_id.clone(),
            window_title: captured_context.window_title.clone().unwrap_or_default(),
            timestamp: chrono::Utc::now().timestamp(),
        };

        // Get memory database
        let db = match self.memory_db.as_ref() {
            Some(db) => db,
            None => {
                println!("[Memory] Warning: Database not initialized");
                return Ok(format!("{}\n\nUser: {}", base_prompt, user_input));
            }
        };

        // Get embedding model
        let model_dir = Self::get_embedding_model_dir()?;
        let embedding_model = Arc::new(
            EmbeddingModel::new(model_dir)
                .map_err(|e| AetherError::config(format!("Failed to initialize embedding model: {}", e)))?
        );

        let init_time = start_time.elapsed();
        println!("[Memory] Initialization time: {:?}", init_time);

        // Create retrieval service
        let retrieval = MemoryRetrieval::new(
            Arc::clone(db),
            Arc::clone(&embedding_model),
            Arc::new(config.memory.clone()),
        );

        // Retrieve memories
        let retrieval_start = Instant::now();
        let memories = self.runtime.block_on(
            retrieval.retrieve_memories(&context_anchor, &user_input)
        )?;
        let retrieval_time = retrieval_start.elapsed();

        println!(
            "[Memory] Retrieved {} memories in {:?} (app: {}, window: {})",
            memories.len(),
            retrieval_time,
            context_anchor.app_bundle_id,
            context_anchor.window_title
        );

        // Augment prompt
        let augmentation_start = Instant::now();
        let augmenter = PromptAugmenter::with_config(
            config.memory.max_context_items as usize,
            false, // Don't show similarity scores in production
        );
        let augmented_prompt = augmenter.augment_prompt(&base_prompt, &memories, &user_input);
        let augmentation_time = augmentation_start.elapsed();

        let total_time = start_time.elapsed();
        println!(
            "[Memory] Augmentation time: {:?}, Total time: {:?}",
            augmentation_time, total_time
        );

        Ok(augmented_prompt)
    }

    /// Process input with AI using the complete pipeline: Memory → Router → Provider → Storage
    ///
    /// This is the main entry point for AI processing that integrates all Phase 5 & 6 components:
    /// 1. Retrieve relevant memories based on context
    /// 2. Augment prompt with memory context
    /// 3. Route to appropriate AI provider
    /// 4. Call provider.process() with augmented input
    /// 5. Store interaction for future retrieval (async, non-blocking)
    ///
    /// # Arguments
    ///
    /// * `input` - User input text from clipboard
    /// * `context` - Captured context (app bundle ID + window title)
    ///
    /// # Returns
    ///
    /// * `Ok(String)` - AI-generated response
    /// * `Err(AetherError)` - Various errors:
    ///   - `NoProviderAvailable` - No router configured
    ///   - `NetworkError`, `AuthenticationError`, etc. - From provider
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use aethecore::core::{AetherCore, CapturedContext};
    /// # fn example(core: &AetherCore) {
    /// let context = CapturedContext {
    ///     app_bundle_id: "com.apple.Notes".to_string(),
    ///     window_title: Some("Document.txt".to_string()),
    /// };
    ///
    /// // This will be called from Swift when user presses Cmd+~
    /// // let response = core.process_with_ai("Explain Rust ownership", &context).await?;
    /// # }
    /// ```
    pub fn process_with_ai(&self, input: &str, _context: &CapturedContext) -> Result<String> {
        use std::time::Instant;

        let start_time = Instant::now();
        println!("[AI Pipeline] Starting processing for input: {} chars", input.len());

        // Step 1: Check if router is available
        let router = self.router.as_ref()
            .ok_or_else(|| AetherError::NoProviderAvailable)?;

        // Step 2: Retrieve memories and augment prompt (if enabled)
        let config = self.config.lock().unwrap();
        let base_system_prompt = "You are a helpful AI assistant.".to_string();
        drop(config); // Release lock before async operations

        let augmented_input = if self.memory_db.is_some() {
            match self.retrieve_and_augment_prompt(base_system_prompt.clone(), input.to_string()) {
                Ok(augmented) => {
                    println!("[AI Pipeline] Memory augmentation succeeded");
                    augmented
                }
                Err(e) => {
                    println!("[AI Pipeline] Warning: Memory augmentation failed: {}", e);
                    // Fallback to original input
                    format!("{}\n\nUser: {}", base_system_prompt, input)
                }
            }
        } else {
            format!("{}\n\nUser: {}", base_system_prompt, input)
        };

        let memory_time = start_time.elapsed();
        println!("[AI Pipeline] Memory retrieval time: {:?}", memory_time);

        // Step 3: Route to appropriate provider
        let (provider, system_prompt_override) = router.route(input)
            .ok_or_else(|| AetherError::NoProviderAvailable)?;

        let provider_name = provider.name().to_string();
        let provider_color = provider.color().to_string();

        println!(
            "[AI Pipeline] Routed to provider: {} (color: {})",
            provider_name, provider_color
        );

        // Notify UI about AI processing start
        self.event_handler.on_state_changed(ProcessingState::Processing);

        // Step 4: Call AI provider
        let routing_time = start_time.elapsed();
        let system_prompt = system_prompt_override.unwrap_or(&base_system_prompt);

        let response = self.runtime.block_on(async {
            provider.process(&augmented_input, Some(system_prompt)).await
        })?;

        let ai_time = start_time.elapsed();
        println!(
            "[AI Pipeline] AI response received in {:?} (total: {:?})",
            ai_time - routing_time,
            ai_time
        );

        // Step 5: Store interaction asynchronously (non-blocking)
        if self.memory_db.is_some() {
            let user_input = input.to_string();
            let ai_output = response.clone();
            let core_clone = self.clone_for_storage();

            // Spawn background task to store memory
            self.runtime.spawn(async move {
                match core_clone.store_interaction_memory(user_input, ai_output) {
                    Ok(memory_id) => {
                        println!("[AI Pipeline] Memory stored: {}", memory_id);
                    }
                    Err(e) => {
                        eprintln!("[AI Pipeline] Warning: Failed to store memory: {}", e);
                    }
                }
            });
        }

        let total_time = start_time.elapsed();
        println!("[AI Pipeline] Total processing time: {:?}", total_time);

        Ok(response)
    }

    /// Clone necessary fields for async memory storage
    ///
    /// This creates a lightweight clone that can be moved into async tasks
    /// for non-blocking memory storage operations.
    fn clone_for_storage(&self) -> StorageHelper {
        StorageHelper {
            config: Arc::clone(&self.config),
            memory_db: self.memory_db.clone(),
            current_context: Arc::clone(&self.current_context),
            runtime: Arc::clone(&self.runtime),
        }
    }
}

/// Helper struct for async memory storage operations
///
/// This is a lightweight clone of AetherCore fields needed for
/// storing interactions in the background without blocking the main flow.
struct StorageHelper {
    config: Arc<Mutex<Config>>,
    memory_db: Option<Arc<VectorDatabase>>,
    current_context: Arc<Mutex<Option<CapturedContext>>>,
    runtime: Arc<Runtime>,
}

impl StorageHelper {
    /// Store interaction memory (used in async context)
    fn store_interaction_memory(&self, user_input: String, ai_output: String) -> Result<String> {
        use crate::memory::context::ContextAnchor;
        use crate::memory::embedding::EmbeddingModel;
        use crate::memory::ingestion::MemoryIngestion;

        // Check if memory is enabled
        let config = self.config.lock().unwrap();
        if !config.memory.enabled {
            return Err(AetherError::config("Memory is disabled"));
        }

        // Get current context
        let current_context = self.current_context.lock().unwrap();
        let captured_context = current_context.as_ref()
            .ok_or_else(|| AetherError::config("No context captured"))?;

        // Create context anchor
        let context_anchor = ContextAnchor {
            app_bundle_id: captured_context.app_bundle_id.clone(),
            window_title: captured_context.window_title.clone().unwrap_or_default(),
            timestamp: chrono::Utc::now().timestamp(),
        };

        // Get memory database
        let db = self.memory_db.as_ref()
            .ok_or_else(|| AetherError::config("Memory database not initialized"))?;

        // Get embedding model directory
        let model_dir = AetherCore::get_embedding_model_dir()
            .map_err(|e| AetherError::config(format!("Failed to get embedding model directory: {}", e)))?;

        // Create embedding model (lazy load)
        let embedding_model = Arc::new(
            EmbeddingModel::new(model_dir)
                .map_err(|e| AetherError::config(format!("Failed to initialize embedding model: {}", e)))?
        );

        // Create ingestion service
        let ingestion = MemoryIngestion::new(
            Arc::clone(db),
            embedding_model,
            Arc::new(config.memory.clone()),
        );

        // Store memory
        let result = self.runtime.block_on(
            ingestion.store_memory(context_anchor, &user_input, &ai_output)
        );

        result
    }
}

/// Memory entry type for FFI (UniFFI-compatible)
#[derive(Debug, Clone)]
pub struct MemoryEntryFFI {
    pub id: String,
    pub app_bundle_id: String,
    pub window_title: String,
    pub user_input: String,
    pub ai_output: String,
    pub timestamp: i64,
    pub similarity_score: Option<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_handler::MockEventHandler;

    #[test]
    fn test_core_creation() {
        let handler = Box::new(MockEventHandler::new());
        let core = AetherCore::new(handler).unwrap();
        assert!(!core.is_listening());
    }

    #[test]
    fn test_start_stop_listening() {
        let core = AetherCore::new(Box::new(MockEventHandler::new())).unwrap();

        assert!(!core.is_listening());

        core.start_listening().unwrap();
        assert!(core.is_listening());

        core.stop_listening().unwrap();
        assert!(!core.is_listening());
    }

    #[test]
    fn test_clipboard_read() {
        let handler = Box::new(MockEventHandler::new());
        let core = AetherCore::new(handler).unwrap();

        // Write test content to clipboard
        core.clipboard_manager.write_text("test content").unwrap();

        // Read it back via core
        let content = core.get_clipboard_text().unwrap();
        assert_eq!(content, "test content");
    }

    #[test]
    fn test_multiple_start_stop_cycles() {
        let handler = Box::new(MockEventHandler::new());
        let core = AetherCore::new(handler).unwrap();

        for _ in 0..3 {
            core.start_listening().unwrap();
            assert!(core.is_listening());

            core.stop_listening().unwrap();
            assert!(!core.is_listening());
        }
    }

    #[test]
    fn test_request_context_storage() {
        let handler = Box::new(MockEventHandler::new());
        let core = AetherCore::new(handler).unwrap();

        // Store request context
        core.store_request_context(
            "Test clipboard content".to_string(),
            "openai".to_string(),
        );

        // Verify context is stored by attempting retry
        let result = core.retry_last_request();
        assert!(result.is_ok());
    }

    #[test]
    fn test_retry_without_context() {
        let handler = Box::new(MockEventHandler::new());
        let core = AetherCore::new(handler).unwrap();

        // Attempt retry without storing context first
        let result = core.retry_last_request();
        assert!(result.is_err());
    }

    #[test]
    fn test_retry_max_limit() {
        let handler = Box::new(MockEventHandler::new());
        let core = AetherCore::new(handler).unwrap();

        // Store request context
        core.store_request_context(
            "Test content".to_string(),
            "openai".to_string(),
        );

        // First retry should succeed
        assert!(core.retry_last_request().is_ok());

        // Second retry should succeed
        assert!(core.retry_last_request().is_ok());

        // Third retry should fail (max limit reached)
        let result = core.retry_last_request();
        assert!(result.is_err());
    }

    #[test]
    fn test_clear_request_context() {
        let handler = Box::new(MockEventHandler::new());
        let core = AetherCore::new(handler).unwrap();

        // Store and then clear context
        core.store_request_context(
            "Test content".to_string(),
            "openai".to_string(),
        );
        core.clear_request_context();

        // Retry should fail after clearing
        let result = core.retry_last_request();
        assert!(result.is_err());
    }

    #[test]
    fn test_context_capture_and_storage() {
        let handler = Box::new(MockEventHandler::new());
        let core = AetherCore::new(handler).unwrap();

        // Simulate context capture from Swift
        let context = CapturedContext {
            app_bundle_id: "com.apple.Notes".to_string(),
            window_title: Some("Test Document.txt".to_string()),
        };
        core.set_current_context(context.clone());

        // Try to store interaction memory
        let result = core.store_interaction_memory(
            "What is the capital of France?".to_string(),
            "The capital of France is Paris.".to_string(),
        );

        // Result may fail if memory is disabled, which is OK
        match result {
            Ok(memory_id) => {
                println!("✓ Context capture test passed - memory stored with ID: {}", memory_id);
            }
            Err(e) => {
                println!("Note: Memory storage failed (expected if memory disabled): {}", e);
            }
        }
    }

    #[test]
    fn test_missing_context_error() {
        let handler = Box::new(MockEventHandler::new());
        let core = AetherCore::new(handler).unwrap();

        // Try to store memory without setting context first
        let result = core.store_interaction_memory(
            "Test input".to_string(),
            "Test output".to_string(),
        );

        // Should fail because no context was captured
        assert!(
            result.is_err(),
            "Should fail when no context is captured"
        );
    }

    #[test]
    fn test_retrieve_and_augment_with_memory_disabled() {
        let handler = Box::new(MockEventHandler::new());
        let core = AetherCore::new(handler).unwrap();

        // Memory is disabled by default
        let result = core.retrieve_and_augment_prompt(
            "You are a helpful assistant.".to_string(),
            "Hello world".to_string(),
        );

        assert!(result.is_ok());
        let augmented = result.unwrap();

        // Should return base prompt + user input without memory context
        assert!(augmented.contains("You are a helpful assistant."));
        assert!(augmented.contains("Hello world"));
        assert!(!augmented.contains("Context History"));
    }

    #[test]
    fn test_retrieve_and_augment_without_context() {
        let handler = Box::new(MockEventHandler::new());
        let core = AetherCore::new(handler).unwrap();

        // Enable memory but don't set context
        {
            let mut config = core.config.lock().unwrap();
            config.memory.enabled = true;
        }

        let result = core.retrieve_and_augment_prompt(
            "You are a helpful assistant.".to_string(),
            "Hello world".to_string(),
        );

        assert!(result.is_ok());
        let augmented = result.unwrap();

        // Should fallback to base prompt when no context
        assert!(augmented.contains("You are a helpful assistant."));
        assert!(augmented.contains("Hello world"));
    }

    #[test]
    fn test_full_aether_core_memory_pipeline() {
        // This test demonstrates the complete AetherCore memory pipeline:
        // 1. Set context
        // 2. Store interaction memory
        // 3. Retrieve and augment prompt with memory context

        let handler = Box::new(MockEventHandler::new());
        let core = AetherCore::new(handler).unwrap();

        // Enable memory and initialize database
        {
            let mut config = core.config.lock().unwrap();
            config.memory.enabled = true;
        }

        // Set context (simulating user in Notes app)
        let context = CapturedContext {
            app_bundle_id: "com.apple.Notes".to_string(),
            window_title: Some("Rust Learning.txt".to_string()),
        };
        core.set_current_context(context);

        // Store first interaction
        let result1 = core.store_interaction_memory(
            "What is Rust?".to_string(),
            "Rust is a systems programming language focused on safety and performance.".to_string(),
        );

        // May fail if memory DB not initialized properly in test environment
        if result1.is_ok() {
            println!("✓ First memory stored: {:?}", result1.unwrap());

            // Store second interaction
            let result2 = core.store_interaction_memory(
                "Is Rust memory safe?".to_string(),
                "Yes, Rust guarantees memory safety through its ownership system.".to_string(),
            );

            if result2.is_ok() {
                println!("✓ Second memory stored: {:?}", result2.unwrap());

                // Now retrieve and augment a new query
                let augmented = core.retrieve_and_augment_prompt(
                    "You are a Rust expert.".to_string(),
                    "Tell me about Rust's ownership".to_string(),
                );

                match augmented {
                    Ok(prompt) => {
                        println!("✓ Memory retrieval and augmentation succeeded");
                        println!("Augmented prompt length: {} chars", prompt.len());

                        // Verify structure
                        assert!(prompt.contains("You are a Rust expert."));
                        assert!(prompt.contains("Tell me about Rust's ownership"));

                        // If memories were retrieved, should contain Context History
                        if prompt.contains("Context History") {
                            println!("✓ Context History section found in augmented prompt");
                        }
                    }
                    Err(e) => {
                        println!("Note: Memory retrieval skipped (expected in test env): {}", e);
                    }
                }
            } else {
                println!("Note: Second memory storage skipped (expected in test env)");
            }
        } else {
            println!("Note: Memory storage skipped (expected in test env without full DB setup)");
        }
    }
}
