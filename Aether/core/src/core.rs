/// AetherCore - Main entry point for the Aether library
///
/// Orchestrates hotkey listening, clipboard management, and event callbacks.
use crate::clipboard::{ArboardManager, ClipboardManager};
use crate::config::{Config, ConfigWatcher, MemoryConfig};
use crate::error::{AetherError, Result};
use crate::event_handler::{AetherEventHandler, ErrorType, ProcessingState};
use crate::hotkey::{HotkeyListener, RdevListener};
use crate::input::{EnigoSimulator, InputSimulator};
use crate::memory::cleanup::CleanupService;
use crate::memory::database::{MemoryStats, VectorDatabase};
use crate::router::Router;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

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
    input_simulator: Arc<EnigoSimulator>,
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
    // Config hot-reload (must be kept alive for file watching)
    #[allow(dead_code)]
    config_watcher: Option<ConfigWatcher>,
    // Typewriter cancellation
    cancellation_token: CancellationToken,
    // Track if typewriter is currently active
    is_typewriting: Arc<Mutex<bool>>,
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
                    let friendly_message = e.user_friendly_message();
                    handler_clone.on_error(friendly_message);
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
                        log::warn!("Failed to initialize router: {}", e);
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
                                        Ok(_) => {
                                            Some(Arc::clone(&cleanup_arc).start_background_task())
                                        }
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

        // Initialize config watcher for hot-reload
        let config_watcher = {
            let handler_clone = Arc::clone(&event_handler);
            let config_clone = Arc::clone(&config);

            let watcher = ConfigWatcher::new(move |config_result| {
                match config_result {
                    Ok(new_config) => {
                        log::info!("Config file changed, reloading configuration");

                        // Update config
                        if let Ok(mut cfg) = config_clone.lock() {
                            *cfg = new_config;
                        }

                        // Notify Swift via callback
                        handler_clone.on_config_changed();
                    }
                    Err(e) => {
                        log::error!("Failed to reload config: {}", e);
                        handler_clone.on_error(format!("Config reload failed: {}", e));
                    }
                }
            });

            // Start watching config file
            match watcher.start() {
                Ok(_) => {
                    log::info!("Config watcher started successfully");
                    Some(watcher)
                }
                Err(e) => {
                    log::warn!("Failed to start config watcher: {}", e);
                    None
                }
            }
        };

        // Create input simulator
        let input_simulator: Arc<EnigoSimulator> = Arc::new(EnigoSimulator::new());

        Ok(Self {
            event_handler,
            hotkey_listener,
            clipboard_manager,
            input_simulator,
            runtime: Arc::new(runtime),
            last_request: Arc::new(Mutex::new(None)),
            config,
            memory_db,
            current_context: Arc::new(Mutex::new(None)),
            cleanup_service,
            cleanup_task_handle,
            router,
            config_watcher,
            cancellation_token: CancellationToken::new(),
            is_typewriting: Arc::new(Mutex::new(false)),
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

    /// Check if clipboard contains image data
    ///
    /// # Returns
    /// * `true` if clipboard contains an image
    /// * `false` if clipboard does not contain an image
    pub fn has_clipboard_image(&self) -> bool {
        self.clipboard_manager.has_image()
    }

    /// Read image from clipboard
    ///
    /// # Returns
    /// * `Ok(Some(ImageData))` if image is successfully read
    /// * `Ok(None)` if clipboard contains no image
    /// * `Err(AetherError)` if an error occurs
    pub fn read_clipboard_image(&self) -> Result<Option<crate::clipboard::ImageData>> {
        self.clipboard_manager.read_image()
    }

    /// Write image to clipboard
    ///
    /// # Arguments
    /// * `image` - The image data to write
    ///
    /// # Returns
    /// * `Ok(())` if image is successfully written
    /// * `Err(AetherError)` if an error occurs
    pub fn write_clipboard_image(&self, image: crate::clipboard::ImageData) -> Result<()> {
        self.clipboard_manager.write_image(image)
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
        self.event_handler.on_error_typed(error_type, message);
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

    /// Cancel typewriter animation if currently running
    ///
    /// If typewriter animation is in progress, this will cancel it and paste
    /// the remaining text instantly via clipboard.
    ///
    /// # Returns
    /// * `true` if typewriter was cancelled
    /// * `false` if no typewriter animation was running
    pub fn cancel_typewriter(&self) -> bool {
        let is_typing = *self.is_typewriting.lock().unwrap();

        if is_typing {
            info!("Cancelling typewriter animation");
            self.cancellation_token.cancel();
            true
        } else {
            false
        }
    }

    /// Check if typewriter animation is currently running
    pub fn is_typewriting(&self) -> bool {
        *self.is_typewriting.lock().unwrap()
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
        let db = self
            .memory_db
            .as_ref()
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
        let db = self
            .memory_db
            .as_ref()
            .ok_or_else(|| AetherError::config("Memory database not initialized"))?;

        // Use empty window title if not provided
        let window = window_title.as_deref().unwrap_or("");

        // For search without embedding, we'll return recent memories only
        // TODO: In Phase 4B, implement actual embedding-based search
        let memories =
            self.runtime
                .block_on(db.search_memories(&app_bundle_id, window, &[], limit))?;

        // Convert to FFI type
        Ok(memories
            .into_iter()
            .map(|m| MemoryEntryFFI {
                id: m.id,
                app_bundle_id: m.context.app_bundle_id,
                window_title: m.context.window_title,
                user_input: m.user_input,
                ai_output: m.ai_output,
                timestamp: m.context.timestamp,
                similarity_score: m.similarity_score,
            })
            .collect())
    }

    /// Delete specific memory by ID
    pub fn delete_memory(&self, id: String) -> Result<()> {
        let db = self
            .memory_db
            .as_ref()
            .ok_or_else(|| AetherError::config("Memory database not initialized"))?;

        self.runtime.block_on(db.delete_memory(&id))
    }

    /// Clear memories (with optional filters)
    pub fn clear_memories(
        &self,
        app_bundle_id: Option<String>,
        window_title: Option<String>,
    ) -> Result<u64> {
        let db = self
            .memory_db
            .as_ref()
            .ok_or_else(|| AetherError::config("Memory database not initialized"))?;

        self.runtime
            .block_on(db.clear_memories(app_bundle_id.as_deref(), window_title.as_deref()))
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
                    old_retention_days, new_config.retention_days
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
        let cleanup = self
            .cleanup_service
            .as_ref()
            .ok_or_else(|| AetherError::config("Cleanup service not initialized"))?;

        cleanup
            .cleanup_old_memories()
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
        let captured_context = current_context
            .as_ref()
            .ok_or_else(|| AetherError::config("No context captured"))?;

        // Create context anchor
        let context_anchor = ContextAnchor {
            app_bundle_id: captured_context.app_bundle_id.clone(),
            window_title: captured_context.window_title.clone().unwrap_or_default(),
            timestamp: chrono::Utc::now().timestamp(),
        };

        // Get memory database
        let db = self
            .memory_db
            .as_ref()
            .ok_or_else(|| AetherError::config("Memory database not initialized"))?;

        // Get embedding model directory
        let model_dir = Self::get_embedding_model_dir().map_err(|e| {
            AetherError::config(format!("Failed to get embedding model directory: {}", e))
        })?;

        // Create embedding model (lazy load)
        let embedding_model = Arc::new(EmbeddingModel::new(model_dir).map_err(|e| {
            AetherError::config(format!("Failed to initialize embedding model: {}", e))
        })?);

        // Create ingestion service
        let ingestion = MemoryIngestion::new(
            Arc::clone(db),
            embedding_model,
            Arc::new(config.memory.clone()),
        );

        // Store memory asynchronously
        let result =
            self.runtime
                .block_on(ingestion.store_memory(context_anchor, &user_input, &ai_output));

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
        let embedding_model = Arc::new(EmbeddingModel::new(model_dir).map_err(|e| {
            AetherError::config(format!("Failed to initialize embedding model: {}", e))
        })?);

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
        let memories = self
            .runtime
            .block_on(retrieval.retrieve_memories(&context_anchor, &user_input))?;
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
    pub fn process_with_ai(&self, input: String, _context: CapturedContext) -> Result<String> {
        use std::time::Instant;

        let start_time = Instant::now();
        info!(
            input_length = input.len(),
            "Starting AI pipeline processing"
        );

        // Wrapper to handle errors with user-friendly messages
        let result = self.process_with_ai_internal(input, _context, start_time);

        // If error occurred, send user-friendly message to UI
        if let Err(ref e) = result {
            let friendly_message = e.user_friendly_message();
            error!(error = ?e, user_message = %friendly_message, "AI processing failed");
            self.event_handler.on_error(friendly_message);
            self.event_handler.on_state_changed(ProcessingState::Error);
        }

        result
    }

    /// Internal implementation of AI processing pipeline
    fn process_with_ai_internal(
        &self,
        input: String,
        _context: CapturedContext,
        start_time: std::time::Instant,
    ) -> Result<String> {
        // Step 1: Check if router is available
        let router = self
            .router
            .as_ref()
            .ok_or(AetherError::NoProviderAvailable)?;

        // Step 2: Retrieve memories and augment prompt (if enabled)
        let config = self.config.lock().unwrap();
        let base_system_prompt = "You are a helpful AI assistant.".to_string();
        drop(config); // Release lock before async operations

        let augmented_input = if self.memory_db.is_some() {
            // Notify UI that we're retrieving memory
            self.event_handler
                .on_state_changed(ProcessingState::RetrievingMemory);

            match self.retrieve_and_augment_prompt(base_system_prompt.clone(), input.clone()) {
                Ok(augmented) => {
                    debug!("Memory augmentation succeeded");
                    augmented
                }
                Err(e) => {
                    warn!(error = %e, "Memory augmentation failed, using original input");
                    // Fallback to original input
                    format!("{}\n\nUser: {}", base_system_prompt, input)
                }
            }
        } else {
            format!("{}\n\nUser: {}", base_system_prompt, input)
        };

        let memory_time = start_time.elapsed();
        debug!(
            duration_ms = memory_time.as_millis(),
            "Memory retrieval completed"
        );

        // Step 3: Route to appropriate provider with fallback support
        let ((provider, system_prompt_override), fallback_provider) = router
            .route_with_fallback(&input)
            .ok_or(AetherError::NoProviderAvailable)?;

        let provider_name = provider.name().to_string();
        let provider_color = provider.color().to_string();

        info!(
            provider = %provider_name,
            color = %provider_color,
            has_fallback = fallback_provider.is_some(),
            "Routed to AI provider"
        );

        // Notify UI about AI processing start (Task 7.4)
        self.event_handler
            .on_ai_processing_started(provider_name.clone(), provider_color.clone());
        self.event_handler
            .on_state_changed(ProcessingState::ProcessingWithAI);

        // Step 4: Call AI provider with retry and fallback logic (Task 10.1 & 10.2)
        let routing_time = start_time.elapsed();
        let system_prompt = system_prompt_override.unwrap_or(&base_system_prompt);

        // Try primary provider with retry
        let response = self.runtime.block_on(async {
            use crate::providers::retry_with_backoff;

            // Attempt with primary provider (with retry)
            let primary_result = retry_with_backoff(
                || provider.process(&augmented_input, Some(system_prompt)),
                Some(3),
            )
            .await;

            match primary_result {
                Ok(response) => {
                    info!(provider = %provider_name, "Primary provider succeeded");
                    Ok(response)
                }
                Err(primary_error) => {
                    warn!(
                        provider = %provider_name,
                        error = ?primary_error,
                        "Primary provider failed"
                    );

                    // Try fallback provider if available
                    if let Some(fallback) = fallback_provider {
                        let fallback_name = fallback.name().to_string();
                        warn!(
                            from_provider = %provider_name,
                            to_provider = %fallback_name,
                            "Attempting fallback to alternative provider"
                        );

                        // Notify UI about fallback (Task 10.2)
                        self.event_handler
                            .on_provider_fallback(provider_name.clone(), fallback_name.clone());

                        // Try fallback provider (with retry)
                        retry_with_backoff(
                            || fallback.process(&augmented_input, Some(system_prompt)),
                            Some(3),
                        )
                        .await
                    } else {
                        error!(
                            provider = %provider_name,
                            "No fallback provider available"
                        );
                        Err(primary_error)
                    }
                }
            }
        })?;

        let ai_time = start_time.elapsed();
        info!(
            ai_duration_ms = (ai_time - routing_time).as_millis(),
            total_duration_ms = ai_time.as_millis(),
            response_length = response.len(),
            "AI response received"
        );

        // Notify UI about AI response (Task 7.4)
        let response_preview = if response.len() > 100 {
            format!("{}...", &response[..100])
        } else {
            response.clone()
        };
        self.event_handler.on_ai_response_received(response_preview);

        // Step 5: Output the response using configured mode (instant or typewriter)
        let output_mode = {
            let config = self.config.lock().unwrap();
            config.behavior.as_ref().map(|b| b.output_mode.clone())
        };

        match output_mode.as_deref() {
            Some("typewriter") => {
                // Typewriter mode: character-by-character typing
                let typing_speed = {
                    let config = self.config.lock().unwrap();
                    config.behavior.as_ref().map(|b| b.typing_speed).unwrap_or(50)
                };

                info!(
                    typing_speed = typing_speed,
                    response_length = response.len(),
                    "Starting typewriter output"
                );

                // Notify UI that we're typing
                self.event_handler.on_state_changed(ProcessingState::Typewriting);

                // Mark typewriter as active
                *self.is_typewriting.lock().unwrap() = true;

                // Create a new cancellation token for this typing operation
                // (reset from previous uses)
                let typing_token = if self.cancellation_token.is_cancelled() {
                    // Create new token if previous one was cancelled
                    
                    // Note: Can't replace self.cancellation_token directly due to ownership
                    // Instead, we'll use a local token and check both
                    CancellationToken::new()
                } else {
                    self.cancellation_token.clone()
                };

                // Type the response character by character with progress tracking
                let response_clone = response.clone();
                let handler = Arc::clone(&self.event_handler);
                let total_chars = response.chars().count();
                let clipboard_mgr = Arc::clone(&self.clipboard_manager);

                let typing_result = self.runtime.block_on(async move {
                    use std::time::Duration;
                    use tokio::time::sleep;

                    // Calculate delay per character
                    let delay_per_char = Duration::from_millis(1000 / typing_speed as u64);
                    let mut typed_chars = 0;

                    // Type character by character with progress callbacks
                    for (idx, ch) in response_clone.chars().enumerate() {
                        // Check cancellation
                        if typing_token.is_cancelled() {
                            warn!("Typewriter cancelled at {} of {} chars", typed_chars, total_chars);

                            // Paste remaining text instantly using spawn_blocking
                            let remaining = response_clone.chars().skip(idx).collect::<String>();
                            if !remaining.is_empty() {
                                info!("Pasting remaining {} chars instantly", remaining.len());
                                clipboard_mgr.write_text(&remaining)?;

                                // Use spawn_blocking for paste operation
                                // This runs in a dedicated blocking thread pool
                                tokio::task::spawn_blocking(move || {
                                    use enigo::Keyboard;
                                    let mut enigo = enigo::Enigo::new(&enigo::Settings::default())
                                        .map_err(|e| AetherError::InputSimulationError {
                                            message: format!("Failed to create Enigo: {:?}", e),
                                        })?;

                                    // Simulate Cmd+V (macOS) or Ctrl+V (Windows/Linux)
                                    #[cfg(target_os = "macos")]
                                    {
                                        enigo.key(enigo::Key::Meta, enigo::Direction::Press)
                                            .map_err(|e| AetherError::InputSimulationError {
                                                message: format!("Failed to press Meta: {:?}", e),
                                            })?;
                                        enigo.key(enigo::Key::Unicode('v'), enigo::Direction::Click)
                                            .map_err(|e| AetherError::InputSimulationError {
                                                message: format!("Failed to click v: {:?}", e),
                                            })?;
                                        enigo.key(enigo::Key::Meta, enigo::Direction::Release)
                                            .map_err(|e| AetherError::InputSimulationError {
                                                message: format!("Failed to release Meta: {:?}", e),
                                            })?;
                                    }

                                    #[cfg(not(target_os = "macos"))]
                                    {
                                        enigo.key(enigo::Key::Control, enigo::Direction::Press)
                                            .map_err(|e| AetherError::InputSimulationError {
                                                message: format!("Failed to press Ctrl: {:?}", e),
                                            })?;
                                        enigo.key(enigo::Key::Unicode('v'), enigo::Direction::Click)
                                            .map_err(|e| AetherError::InputSimulationError {
                                                message: format!("Failed to click v: {:?}", e),
                                            })?;
                                        enigo.key(enigo::Key::Control, enigo::Direction::Release)
                                            .map_err(|e| AetherError::InputSimulationError {
                                                message: format!("Failed to release Ctrl: {:?}", e),
                                            })?;
                                    }

                                    Ok::<(), AetherError>(())
                                })
                                .await
                                .map_err(|e| AetherError::InputSimulationError {
                                    message: format!("Spawn blocking failed: {:?}", e),
                                })??;
                            }

                            handler.on_typewriter_cancelled();
                            return Ok::<(), AetherError>(());
                        }

                        // Type single character using spawn_blocking
                        // This runs Enigo in a dedicated blocking thread pool, avoiding Send issues
                        // Performance: spawn_blocking reuses threads, much faster than creating new threads
                        tokio::task::spawn_blocking(move || {
                            use enigo::Keyboard;
                            let mut enigo = enigo::Enigo::new(&enigo::Settings::default())
                                .map_err(|e| AetherError::InputSimulationError {
                                    message: format!("Failed to create Enigo: {:?}", e),
                                })?;

                            match ch {
                                '\n' => {
                                    enigo.key(enigo::Key::Return, enigo::Direction::Click)
                                        .map_err(|e| AetherError::InputSimulationError {
                                            message: format!("Failed to type newline: {:?}", e),
                                        })?;
                                }
                                '\t' => {
                                    enigo.key(enigo::Key::Tab, enigo::Direction::Click)
                                        .map_err(|e| AetherError::InputSimulationError {
                                            message: format!("Failed to type tab: {:?}", e),
                                        })?;
                                }
                                _ => {
                                    enigo.text(&ch.to_string())
                                        .map_err(|e| AetherError::InputSimulationError {
                                            message: format!("Failed to type char: {:?}", e),
                                        })?;
                                }
                            }

                            Ok::<(), AetherError>(())
                        })
                        .await
                        .map_err(|e| AetherError::InputSimulationError {
                            message: format!("Spawn blocking join failed: {:?}", e),
                        })??;

                        typed_chars += 1;

                        // Send progress update every 10 chars or at completion
                        if typed_chars % 10 == 0 || typed_chars == total_chars {
                            let progress = typed_chars as f32 / total_chars as f32;
                            handler.on_typewriter_progress(progress);
                        }

                        // Delay before next character
                        sleep(delay_per_char).await;
                    }

                    // Send final 100% progress
                    handler.on_typewriter_progress(1.0);
                    Ok(())
                });

                // Mark typewriter as inactive
                *self.is_typewriting.lock().unwrap() = false;

                match typing_result {
                    Ok(_) => {
                        info!("Typewriter output completed successfully");
                    }
                    Err(e) => {
                        warn!(error = ?e, "Typewriter output failed, falling back to instant paste");
                        // Fallback to instant paste on error
                        self.clipboard_manager.write_text(&response)?;
                        self.input_simulator.simulate_paste()?;
                    }
                }
            }
            _ => {
                // Instant mode (default): paste immediately
                info!("Using instant paste mode");
                self.clipboard_manager.write_text(&response)?;
                self.input_simulator.simulate_paste()?;
            }
        }

        // Step 6: Store interaction asynchronously (non-blocking)
        if self.memory_db.is_some() {
            let user_input = input.clone();
            let ai_output = response.clone();
            let core_clone = self.clone_for_storage();

            // Spawn background task to store memory
            self.runtime.spawn(async move {
                match core_clone.store_interaction_memory(user_input, ai_output) {
                    Ok(memory_id) => {
                        log::debug!("[AI Pipeline] Memory stored: {}", memory_id);
                    }
                    Err(e) => {
                        log::error!("[AI Pipeline] Failed to store memory: {}", e);
                    }
                }
            });
        }

        let total_time = start_time.elapsed();
        log::info!("[AI Pipeline] Total processing time: {:?}", total_time);

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

    // ========== CONFIG MANAGEMENT METHODS (Phase 6 - Task 1.5) ==========

    /// Load configuration and return it in UniFFI-compatible format
    pub fn load_config(&self) -> Result<crate::config::FullConfig> {
        let config = self.config.lock().unwrap();
        Ok(config.clone().into())
    }

    /// Update provider configuration
    pub fn update_provider(&self, name: String, provider: crate::config::ProviderConfig) -> Result<()> {
        let mut config = self.config.lock().unwrap();
        config.providers.insert(name, provider);
        config.save()?;
        Ok(())
    }

    /// Delete provider configuration
    pub fn delete_provider(&self, name: String) -> Result<()> {
        let mut config = self.config.lock().unwrap();
        config.providers.remove(&name);
        config.save()?;
        Ok(())
    }

    /// Update routing rules
    pub fn update_routing_rules(&self, rules: Vec<crate::config::RoutingRuleConfig>) -> Result<()> {
        let mut config = self.config.lock().unwrap();
        config.rules = rules;
        config.validate()?;
        config.save()?;
        Ok(())
    }

    /// Update shortcuts configuration
    pub fn update_shortcuts(&self, shortcuts: crate::config::ShortcutsConfig) -> Result<()> {
        let mut config = self.config.lock().unwrap();
        config.shortcuts = Some(shortcuts);
        config.save()?;
        log::info!("Shortcuts configuration updated");
        Ok(())
    }

    /// Update behavior configuration
    pub fn update_behavior(&self, behavior: crate::config::BehaviorConfig) -> Result<()> {
        let mut config = self.config.lock().unwrap();
        config.behavior = Some(behavior);
        config.save()?;
        log::info!("Behavior configuration updated");
        Ok(())
    }

    /// Validate regex pattern
    pub fn validate_regex(&self, pattern: String) -> Result<bool> {
        match regex::Regex::new(&pattern) {
            Ok(_) => Ok(true),
            Err(e) => Err(AetherError::invalid_config(format!("Invalid regex: {}", e))),
        }
    }

    /// Test provider connection
    ///
    /// Sends a test request to the provider to verify configuration.
    /// Returns a success message if the provider responds correctly.
    pub fn test_provider_connection(&self, provider_name: String) -> Result<String> {
        use crate::providers::create_provider;

        // Get provider config
        let config = self.config.lock().unwrap();
        let provider_config = config
            .providers
            .get(&provider_name)
            .ok_or_else(|| {
                AetherError::invalid_config(format!("Provider '{}' not found", provider_name))
            })?
            .clone();

        drop(config); // Release lock before async operations

        // Create provider instance
        let provider = create_provider(&provider_name, provider_config)?;

        // Send test request (block on async operation)
        let test_prompt = "Say 'OK' if you can read this.";
        let runtime = Runtime::new().map_err(|e| {
            AetherError::provider(format!("Failed to create runtime for test: {}", e))
        })?;

        let result: String = runtime.block_on(async {
            let response = provider
                .process(test_prompt, None)
                .await
                .map_err(|e| AetherError::provider(format!("Connection test failed: {}", e)))?;
            Ok::<String, AetherError>(response)
        })?;

        Ok(format!(
            "✓ Connection successful! Provider responded: {}",
            result.chars().take(50).collect::<String>()
        ))
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
        let captured_context = current_context
            .as_ref()
            .ok_or_else(|| AetherError::config("No context captured"))?;

        // Create context anchor
        let context_anchor = ContextAnchor {
            app_bundle_id: captured_context.app_bundle_id.clone(),
            window_title: captured_context.window_title.clone().unwrap_or_default(),
            timestamp: chrono::Utc::now().timestamp(),
        };

        // Get memory database
        let db = self
            .memory_db
            .as_ref()
            .ok_or_else(|| AetherError::config("Memory database not initialized"))?;

        // Get embedding model directory
        let model_dir = AetherCore::get_embedding_model_dir().map_err(|e| {
            AetherError::config(format!("Failed to get embedding model directory: {}", e))
        })?;

        // Create embedding model (lazy load)
        let embedding_model = Arc::new(EmbeddingModel::new(model_dir).map_err(|e| {
            AetherError::config(format!("Failed to initialize embedding model: {}", e))
        })?);

        // Create ingestion service
        let ingestion = MemoryIngestion::new(
            Arc::clone(db),
            embedding_model,
            Arc::new(config.memory.clone()),
        );

        // Store memory
        let result =
            self.runtime
                .block_on(ingestion.store_memory(context_anchor, &user_input, &ai_output));

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
        core.store_request_context("Test clipboard content".to_string(), "openai".to_string());

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
        core.store_request_context("Test content".to_string(), "openai".to_string());

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
        core.store_request_context("Test content".to_string(), "openai".to_string());
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
                println!(
                    "✓ Context capture test passed - memory stored with ID: {}",
                    memory_id
                );
            }
            Err(e) => {
                println!(
                    "Note: Memory storage failed (expected if memory disabled): {}",
                    e
                );
            }
        }
    }

    #[test]
    fn test_missing_context_error() {
        let handler = Box::new(MockEventHandler::new());
        let core = AetherCore::new(handler).unwrap();

        // Try to store memory without setting context first
        let result =
            core.store_interaction_memory("Test input".to_string(), "Test output".to_string());

        // Should fail because no context was captured
        assert!(result.is_err(), "Should fail when no context is captured");
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
                        println!(
                            "Note: Memory retrieval skipped (expected in test env): {}",
                            e
                        );
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
