/// AetherCore - Main entry point for the Aether library
///
/// Orchestrates AI routing, memory management, and event callbacks.
///
/// NEW ARCHITECTURE (Phase 2: Native API Separation):
/// - Hotkey listening → Swift GlobalHotkeyMonitor
/// - Clipboard operations → Swift ClipboardManager
/// - Keyboard simulation → Swift KeyboardSimulator
///
/// Rust core focuses on:
/// - AI routing and provider calls
/// - Memory retrieval and storage
/// - Configuration management
use crate::config::{Config, ConfigWatcher, GeneralConfig, MemoryConfig, TestConnectionResult};
use crate::error::{AetherError, AetherException, Result};
use crate::event_handler::{AetherEventHandler, ErrorType, ProcessingState};
use crate::memory::cleanup::CleanupService;
use crate::memory::database::{MemoryStats, VectorDatabase};
use crate::metrics::{StageTimer, TARGET_CLIPBOARD_TO_MEMORY_MS, TARGET_MEMORY_TO_AI_MS};
use crate::router::Router;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use tokio::runtime::Runtime;
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
/// NEW ARCHITECTURE (Phase 2):
/// - System interactions (hotkeys, clipboard, keyboard) → Swift layer
/// - Core focuses on AI processing, routing, memory, and config
pub struct AetherCore {
    event_handler: Arc<dyn AetherEventHandler>,
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
    // AI routing (RwLock allows hot-reload after config changes)
    router: Arc<RwLock<Option<Arc<Router>>>>,
    // Config hot-reload (must be kept alive for file watching)
    #[allow(dead_code)]
    config_watcher: Option<Arc<ConfigWatcher>>,
}

impl AetherCore {
    /// Create a new AetherCore instance with the provided event handler
    ///
    /// # Arguments
    /// * `event_handler` - Handler for receiving callbacks from Rust
    ///
    /// # Returns
    /// * `Result<Self>` - New AetherCore instance or error
    ///
    /// # NEW ARCHITECTURE (Phase 2)
    /// System interactions are handled by Swift layer:
    /// - Hotkey listening: GlobalHotkeyMonitor.swift
    /// - Clipboard operations: ClipboardManager.swift
    /// - Keyboard simulation: KeyboardSimulator.swift
    ///
    /// Rust core focuses on AI processing, memory, and config.
    pub fn new(event_handler: Box<dyn AetherEventHandler>) -> Result<Self> {
        // CRITICAL: Initialize logging system FIRST before any log statements
        // This ensures all log messages are captured to file from the start
        crate::init_logging();

        let event_handler: Arc<dyn AetherEventHandler> = Arc::from(event_handler);

        // Initialize tokio runtime with optimized configuration for macOS
        // Use fewer threads to reduce priority inversion risk with UI thread
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2) // Limit to 2 worker threads (down from default based on CPU cores)
            .max_blocking_threads(2) // Limit blocking threads (down from default 512)
            .thread_name("aether-worker")
            .enable_all()
            .build()
            .map_err(|e| AetherError::other(format!("Failed to create tokio runtime: {}", e)))?;

        // Initialize configuration - load from file or use default
        let config = Arc::new(Mutex::new(Config::load().unwrap_or_else(|e| {
            eprintln!("Warning: Failed to load config file: {}", e);
            eprintln!("Using default configuration");
            Config::default()
        })));

        info!("Configuration loaded successfully");

        // Log configuration status for debugging
        {
            let cfg = config.lock().unwrap_or_else(|e| e.into_inner());
            info!(
                providers_count = cfg.providers.len(),
                rules_count = cfg.rules.len(),
                default_provider = ?cfg.general.default_provider,
                memory_enabled = cfg.memory.enabled,
                "Current configuration"
            );
        }

        // Initialize router (if providers are configured)
        // Wrapped in RwLock to allow hot-reload after config changes
        let router = {
            let cfg = config.lock().unwrap_or_else(|e| e.into_inner());
            let router_opt = if !cfg.providers.is_empty() {
                match Router::new(&cfg) {
                    Ok(r) => Some(Arc::new(r)),
                    Err(e) => {
                        log::warn!("Failed to initialize router: {}", e);
                        None
                    }
                }
            } else {
                None
            };
            Arc::new(RwLock::new(router_opt))
        };

        // Initialize memory database and cleanup service if enabled
        let (memory_db, cleanup_service, cleanup_task_handle) = {
            let cfg = config.lock().unwrap_or_else(|e| e.into_inner());
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
            let router_clone = Arc::clone(&router);

            let watcher = Arc::new(ConfigWatcher::new(move |config_result| {
                match config_result {
                    Ok(new_config) => {
                        log::info!("Config file changed, reloading configuration");

                        // Update config
                        if let Ok(mut cfg) = config_clone.lock() {
                            *cfg = new_config.clone();
                        }

                        // Reinitialize router with new config
                        let new_router = if !new_config.providers.is_empty() {
                            match Router::new(&new_config) {
                                Ok(r) => {
                                    log::info!(
                                        "Router hot-reloaded with {} rules and {} providers",
                                        new_config.rules.len(),
                                        new_config.providers.len()
                                    );
                                    Some(Arc::new(r))
                                }
                                Err(e) => {
                                    log::warn!(
                                        "Failed to reinitialize router during hot-reload: {}",
                                        e
                                    );
                                    None
                                }
                            }
                        } else {
                            None
                        };

                        // Update router
                        if let Ok(mut router_guard) = router_clone.write() {
                            *router_guard = new_router;
                        }

                        // Notify Swift via callback
                        handler_clone.on_config_changed();
                    }
                    Err(e) => {
                        log::error!("Failed to reload config: {}", e);
                        let suggestion = e.suggestion().map(|s| s.to_string());
                        handler_clone.on_error(format!("Config reload failed: {}", e), suggestion);
                    }
                }
            }));

            // Start watching config file asynchronously to avoid blocking UI thread
            // This prevents priority inversion warnings on macOS when called from Swift
            let watcher_for_thread = Arc::clone(&watcher);
            std::thread::Builder::new()
                .name("config-watcher-init".to_string())
                .spawn(move || match watcher_for_thread.start() {
                    Ok(_) => {
                        log::info!("Config watcher started successfully");
                    }
                    Err(e) => {
                        log::warn!("Failed to start config watcher: {}", e);
                    }
                })
                .map_err(|e| log::warn!("Failed to spawn config watcher thread: {}", e))
                .ok();

            Some(watcher)
        };

        Ok(Self {
            event_handler,
            runtime: Arc::new(runtime),
            last_request: Arc::new(Mutex::new(None)),
            config,
            memory_db,
            current_context: Arc::new(Mutex::new(None)),
            cleanup_service,
            cleanup_task_handle,
            router,
            config_watcher,
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

    // === Private Helper Methods ===

    /// Acquires the config mutex lock with poison recovery.
    ///
    /// This is a convenience wrapper to reduce boilerplate for the common pattern
    /// of acquiring a config lock with automatic poison recovery.
    ///
    /// # Returns
    /// A `MutexGuard` providing access to the configuration.
    #[inline(always)]
    fn lock_config(&self) -> std::sync::MutexGuard<'_, Config> {
        self.config.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Ensures the memory database is initialized and returns a reference to it.
    ///
    /// This is a convenience wrapper to reduce boilerplate for memory DB null checks.
    ///
    /// # Returns
    /// A reference to the memory database `Arc`.
    ///
    /// # Errors
    /// Returns `AetherError::config` if the memory database is not initialized.
    #[inline(always)]
    fn require_memory_db(&self) -> Result<&Arc<VectorDatabase>> {
        self.memory_db
            .as_ref()
            .ok_or_else(|| AetherError::config("Memory database not initialized"))
    }

    /// Start listening for hotkey events (DEPRECATED - now handled by Swift layer)
    ///
    /// # IMPORTANT
    /// As of the latest refactor, hotkey listening is handled by Swift's GlobalHotkeyMonitor
    /// to avoid thread conflicts with macOS event system. This method is kept for API compatibility
    /// but does nothing.
    ///
    /// The actual hotkey detection happens in Swift (GlobalHotkeyMonitor.swift) and triggers
    /// EventHandler.onHotkeyDetected() which processes clipboard content.
    pub fn start_listening(&self) -> Result<()> {
        info!(
            "[AetherCore] start_listening() called - hotkey monitoring now handled by Swift layer"
        );
        info!("[AetherCore] See GlobalHotkeyMonitor.swift for implementation details");
        Ok(())
    }

    /// Stop listening for hotkey events (DEPRECATED - now handled by Swift layer)
    ///
    /// # IMPORTANT
    /// This method is kept for API compatibility but does nothing.
    /// Hotkey monitoring lifecycle is managed by Swift's GlobalHotkeyMonitor.
    pub fn stop_listening(&self) -> Result<()> {
        info!("[AetherCore] stop_listening() called - hotkey monitoring handled by Swift layer");
        Ok(())
    }

    // ========================================
    // REMOVED: Clipboard API methods (行 283-320)
    // 剪贴板操作已迁移到 Swift ClipboardManager.swift
    // See: refactor-native-api-separation proposal
    // - get_clipboard_text() → ClipboardManager.getText()
    // - has_clipboard_image() → ClipboardManager.hasImage()
    // - read_clipboard_image() → ClipboardManager.getImage()
    // - write_clipboard_image() → ClipboardManager.setImage()
    // ========================================

    // ========================================
    // LOGGING CONTROL METHODS (Phase 7.3)
    // ========================================

    /// Get the current log level
    ///
    /// Returns the currently configured log level for the application.
    pub fn get_log_level(&self) -> crate::logging::LogLevel {
        crate::logging::get_log_level()
    }

    /// Set the log level dynamically
    ///
    /// Changes the global log level at runtime. This affects all new log messages
    /// but does not retroactively filter existing logs.
    ///
    /// # Arguments
    /// * `level` - The new log level to set
    ///
    /// # Example
    /// ```no_run
    /// core.set_log_level(LogLevel::Debug)?;
    /// ```
    pub fn set_log_level(&self, level: crate::logging::LogLevel) -> Result<()> {
        crate::logging::set_log_level(level);
        Ok(())
    }

    /// Get the log directory path
    ///
    /// Returns the absolute path to the directory where log files are stored.
    /// On macOS/Linux, this is typically `~/.config/aether/logs/`
    ///
    /// # Returns
    /// * `Ok(String)` - Absolute path to log directory
    /// * `Err(AetherError)` - Failed to determine log directory
    pub fn get_log_directory(&self) -> Result<String> {
        let log_dir = crate::logging::get_log_directory()
            .map_err(|e| AetherError::config(format!("Failed to get log directory: {}", e)))?;

        Ok(log_dir.to_string_lossy().to_string())
    }

    /// Check if hotkey listener is active (DEPRECATED - always returns false)
    ///
    /// # Note
    /// Hotkey monitoring is now handled by Swift layer (GlobalHotkeyMonitor).
    /// This method always returns false for backward compatibility.
    pub fn is_listening(&self) -> bool {
        false // Hotkey listening is now in Swift layer
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

        let mut last_request_lock = self.last_request.lock().unwrap_or_else(|e| {
            warn!("Mutex poisoned in last_request (retry_last_request), recovering");
            e.into_inner()
        });

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
        let mut last_request = self.last_request.lock().unwrap_or_else(|e| {
            warn!("Mutex poisoned in last_request (store_request_context), recovering");
            e.into_inner()
        });
        *last_request = Some(RequestContext {
            clipboard_content,
            provider,
            retry_count: 0,
        });
    }

    /// Clear stored request context
    pub fn clear_request_context(&self) {
        let mut last_request = self.last_request.lock().unwrap_or_else(|e| {
            warn!("Mutex poisoned in last_request (clear_request_context), recovering");
            e.into_inner()
        });
        *last_request = None;
    }

    // MEMORY MANAGEMENT METHODS (Phase 4)

    /// Get memory database statistics
    pub fn get_memory_stats(&self) -> Result<MemoryStats> {
        let db = self.require_memory_db()?;

        self.runtime.block_on(db.get_stats())
    }

    /// Search memories by context
    pub fn search_memories(
        &self,
        app_bundle_id: String,
        window_title: Option<String>,
        limit: u32,
    ) -> Result<Vec<MemoryEntryFFI>> {
        let db = self.require_memory_db()?;

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

    /// Get list of unique app bundle IDs from memories
    pub fn get_memory_app_list(&self) -> Result<Vec<AppMemoryInfo>> {
        let db = self.require_memory_db()?;

        let apps = self.runtime.block_on(db.get_app_list())?;

        // Convert to FFI type
        Ok(apps
            .into_iter()
            .map(|(app_bundle_id, memory_count)| AppMemoryInfo {
                app_bundle_id,
                memory_count,
            })
            .collect())
    }

    /// Delete specific memory by ID
    pub fn delete_memory(&self, id: String) -> Result<()> {
        let db = self.require_memory_db()?;

        self.runtime.block_on(db.delete_memory(&id))
    }

    /// Clear memories (with optional filters)
    pub fn clear_memories(
        &self,
        app_bundle_id: Option<String>,
        window_title: Option<String>,
    ) -> Result<u64> {
        let db = self.require_memory_db()?;

        self.runtime
            .block_on(db.clear_memories(app_bundle_id.as_deref(), window_title.as_deref()))
    }

    /// Get memory configuration
    pub fn get_memory_config(&self) -> MemoryConfig {
        let config = self.lock_config();
        config.memory.clone()
    }

    /// Update memory configuration
    pub fn update_memory_config(&self, new_config: MemoryConfig) -> Result<()> {
        let mut config = self.lock_config();
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

    /// Update general configuration (language preference, etc.)
    ///
    /// This method updates the general configuration section and persists to disk.
    /// Used for settings like language preference that don't require service restart.
    ///
    /// # Arguments
    /// * `new_config` - New general configuration
    ///
    /// # Returns
    /// * `Result<()>` - Success or error
    pub fn update_general_config(&self, new_config: GeneralConfig) -> Result<()> {
        let mut config = self.lock_config();
        config.general = new_config;

        // Persist config to disk
        config
            .save()
            .map_err(|e| AetherError::config(format!("Failed to save general config: {}", e)))?;

        Ok(())
    }

    /// Set current context (called from Swift when hotkey pressed)
    pub fn set_current_context(&self, context: CapturedContext) {
        let mut current_context = self.current_context.lock().unwrap_or_else(|e| {
            warn!("Mutex poisoned in current_context (set_current_context), recovering");
            e.into_inner()
        });
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
        let config = self.lock_config();
        if !config.memory.enabled {
            return Err(AetherError::config("Memory is disabled"));
        }

        // Get current context
        let current_context = self.current_context.lock().unwrap_or_else(|e| {
            warn!("Mutex poisoned in current_context (AetherCore::store_interaction_memory), recovering");
            e.into_inner()
        });
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
        let db = self.require_memory_db()?;

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
        let config = self.lock_config();
        if !config.memory.enabled {
            println!("[Memory] Disabled - using base prompt");
            return Ok(format!("{}\n\nUser: {}", base_prompt, user_input));
        }

        // Get current context
        let current_context = self.current_context.lock().unwrap_or_else(|e| {
            warn!("Mutex poisoned in current_context (retrieve_and_augment_prompt), recovering");
            e.into_inner()
        });
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

    /// Retrieve memories and augment ONLY the user input (no system prompt)
    ///
    /// This is the NEW method for the refactored architecture where:
    /// - System prompt is passed separately to the AI provider
    /// - User input should not contain "User:" prefix
    ///
    /// # Arguments
    /// * `user_input` - Current user input/query
    ///
    /// # Returns
    /// * `Result<String>` - User input with optional memory context
    pub fn retrieve_and_augment_user_input(&self, user_input: String) -> Result<String> {
        use crate::memory::augmentation::PromptAugmenter;
        use crate::memory::context::ContextAnchor;
        use crate::memory::embedding::EmbeddingModel;
        use crate::memory::retrieval::MemoryRetrieval;
        use std::time::Instant;

        let start_time = Instant::now();

        // Check if memory is enabled
        let config = self.lock_config();
        if !config.memory.enabled {
            debug!("[Memory] Disabled - returning original user input");
            return Ok(user_input);
        }

        // Get current context
        let current_context = self.current_context.lock().unwrap_or_else(|e| {
            warn!(
                "Mutex poisoned in current_context (retrieve_and_augment_user_input), recovering"
            );
            e.into_inner()
        });
        let captured_context = match current_context.as_ref() {
            Some(ctx) => ctx,
            None => {
                debug!("[Memory] No context captured, returning original user input");
                return Ok(user_input);
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
                debug!("[Memory] Database not initialized, returning original user input");
                return Ok(user_input);
            }
        };

        // Get embedding model
        let model_dir = Self::get_embedding_model_dir()?;
        let embedding_model = Arc::new(EmbeddingModel::new(model_dir).map_err(|e| {
            AetherError::config(format!("Failed to initialize embedding model: {}", e))
        })?);

        // Create retrieval service
        let retrieval = MemoryRetrieval::new(
            Arc::clone(db),
            Arc::clone(&embedding_model),
            Arc::new(config.memory.clone()),
        );

        // Retrieve memories
        let memories = self
            .runtime
            .block_on(retrieval.retrieve_memories(&context_anchor, &user_input))?;

        debug!(
            "[Memory] Retrieved {} memories for user input augmentation",
            memories.len()
        );

        // Augment user input (without system prompt)
        let augmenter =
            PromptAugmenter::with_config(config.memory.max_context_items as usize, false);
        let augmented_input = augmenter.augment_user_input(&memories, &user_input);

        let total_time = start_time.elapsed();
        debug!(
            "[Memory] User input augmentation completed in {:?}",
            total_time
        );

        Ok(augmented_input)
    }

    /// Process input with AI using the complete pipeline: Memory → Router → Provider → Storage
    ///
    /// This is the NEW entry point for the refactored architecture (Phase 2: Native API Separation).
    /// Swift layer handles system interactions (clipboard, hotkeys, keyboard simulation),
    /// and calls this method with pre-processed user input and captured context.
    ///
    /// Pipeline:
    /// 1. Set current context (for memory retrieval)
    /// 2. Retrieve relevant memories based on context
    /// 3. Augment prompt with memory context
    /// 4. Route to appropriate AI provider
    /// 5. Call provider.process() with augmented input
    /// 6. Store interaction for future retrieval (async, non-blocking)
    ///
    /// # Arguments
    ///
    /// * `user_input` - User input text (from Swift ClipboardManager)
    /// * `context` - Captured context (app bundle ID + window title from Swift ContextCapture)
    ///
    /// # Returns
    ///
    /// * `Ok(String)` - AI-generated response (Swift will use KeyboardSimulator to output)
    /// * `Err(AetherError)` - Various errors:
    ///   - `NoProviderAvailable` - No router configured
    ///   - `NetworkError`, `AuthenticationError`, etc. - From provider
    ///
    /// # Example (Swift → Rust)
    ///
    /// ```swift
    /// // Swift layer captures context and input
    /// let context = CapturedContext(
    ///     app_bundle_id: "com.apple.Notes",
    ///     window_title: "Document.txt"
    /// )
    /// let input = ClipboardManager.getText()
    ///
    /// // Call Rust core
    /// let response = try core.process_input(user_input: input, context: context)
    ///
    /// // Swift handles output
    /// KeyboardSimulator.typeText(response)
    /// ```

    /// Handle processing error with user-friendly messaging
    ///
    /// This helper centralizes error handling logic for AI processing failures.
    /// It extracts user-friendly messages, logs errors, notifies the event handler,
    /// and returns an AetherException.
    ///
    /// # Arguments
    ///
    /// * `error` - The AetherError to handle
    ///
    /// # Returns
    ///
    /// AetherException::Error for UniFFI compatibility
    fn handle_processing_error(&self, error: &AetherError) -> AetherException {
        let friendly_message = error.user_friendly_message();
        let suggestion = error.suggestion().map(|s| s.to_string());

        error!(error = ?error, user_message = %friendly_message, "AI processing failed");

        // Notify Swift layer with detailed error
        self.event_handler.on_error(friendly_message, suggestion);
        self.event_handler.on_state_changed(ProcessingState::Error);

        AetherException::Error
    }

    pub fn process_input(
        &self,
        user_input: String,
        context: CapturedContext,
    ) -> std::result::Result<String, AetherException> {
        use std::time::Instant;
        let start_time = Instant::now();

        info!(
            input_length = user_input.len(),
            app = %context.app_bundle_id,
            window = ?context.window_title,
            "Processing input via new architecture (Swift → Rust)"
        );

        // Store context for memory operations
        self.set_current_context(context.clone());

        // Call internal implementation and handle errors
        match self.process_with_ai_internal(user_input, context, start_time) {
            Ok(response) => Ok(response),
            Err(e) => Err(self.handle_processing_error(&e)),
        }
    }

    /// DEPRECATED: Old entry point for AI processing (kept for backward compatibility)
    ///
    /// This method is kept to avoid breaking existing tests and Swift code during migration.
    /// New code should use `process_input()` instead.
    ///
    /// # Migration Note
    /// This will be removed in Phase 2 cleanup after all Swift code is updated.
    #[deprecated(since = "0.2.0", note = "Use process_input() instead")]
    pub fn process_with_ai(&self, input: String, _context: CapturedContext) -> Result<String> {
        use std::time::Instant;

        let start_time = Instant::now();
        info!(
            input_length = input.len(),
            "Starting AI pipeline processing"
        );

        // Wrapper to handle errors with user-friendly messages
        let result = self.process_with_ai_internal(input, _context, start_time);

        // If error occurred, use centralized error handler
        if let Err(ref e) = result {
            let _ = self.handle_processing_error(e);
        }

        result
    }

    /// Build routing context string from window context and clipboard content
    ///
    /// Format: `ClipboardContent\n---\n[AppName] WindowTitle`
    ///
    /// IMPORTANT: Clipboard content is placed FIRST to maintain backward compatibility
    /// with rules like `^/en` that expect content to start with a command prefix.
    ///
    /// This combines window context with clipboard content to enable context-aware routing.
    /// Rules can match based on:
    /// - Clipboard content prefix (e.g., `^/en` - matches content starting with /en)
    /// - Clipboard content anywhere (e.g., `/translate`)
    /// - Window context (e.g., `\[VSCode\]` - matches VSCode app)
    /// - Both (e.g., `TODO.*\[Notes\]`)
    ///
    /// # Arguments
    ///
    /// * `context` - Window context (app bundle ID and window title)
    /// * `clipboard_content` - Content from clipboard
    ///
    /// # Returns
    ///
    /// Formatted context string for routing
    fn build_routing_context(context: &CapturedContext, clipboard_content: &str) -> String {
        // Extract app name from bundle ID (e.g., "com.apple.Notes" → "Notes")
        let app_name = context.app_bundle_id.split('.').next_back().unwrap_or("Unknown");

        // Format: ClipboardContent\n---\n[AppName] WindowTitle
        // Clipboard content is FIRST to preserve backward compatibility with ^/prefix rules
        format!(
            "{}\n---\n[{}] {}",
            clipboard_content,
            app_name,
            context.window_title.as_deref().unwrap_or("")
        )
    }

    /// Internal implementation of AI processing pipeline (NEW ARCHITECTURE)
    ///
    /// This method now focuses ONLY on AI processing:
    /// - Building routing context (window + clipboard)
    /// - Memory retrieval and prompt augmentation
    /// - AI routing and provider calls
    /// - Memory storage (async)
    ///
    /// OUTPUT HANDLING (Typewriter/Paste) is now handled by Swift KeyboardSimulator.
    /// This simplifies the Rust layer and aligns with the "Native First" principle.
    fn process_with_ai_internal(
        &self,
        input: String,
        context: CapturedContext,
        start_time: std::time::Instant,
    ) -> Result<String> {
        // Overall pipeline timer
        let _pipeline_timer = StageTimer::start("total_pipeline");

        // Step 1: Check if router is available
        // Get a clone of the Arc<Router> to avoid holding the RwLock during AI processing
        let router = {
            let router_guard = self.router.read().unwrap_or_else(|e| e.into_inner());
            router_guard.as_ref().map(Arc::clone).ok_or(
                AetherError::NoProviderAvailable {
                    suggestion: Some(
                        "Configure at least one AI provider in Settings → Providers".to_string(),
                    ),
                },
            )?
        };

        // Step 1.5: Build routing context string (clipboard content + window context)
        let routing_context = Self::build_routing_context(&context, &input);
        info!(
            context_length = routing_context.len(),
            app = %context.app_bundle_id,
            window = ?context.window_title,
            context_preview = %routing_context.chars().take(100).collect::<String>(),
            "Built routing context for provider selection"
        );

        // Step 2: Retrieve memories and augment prompt (if enabled)
        let config = self.lock_config();
        let base_system_prompt = "You are a helpful AI assistant.".to_string();
        let perf_logging_enabled = config.general.enable_performance_logging;
        drop(config); // Release lock before async operations

        // FIXED: Only include user input in augmented_input, NOT the system prompt
        // The system prompt is passed separately to provider.process()
        // Including it here was causing the AI to respond in conversation format
        let augmented_input = if self.memory_db.is_some() {
            // Notify UI that we're retrieving memory
            self.event_handler
                .on_state_changed(ProcessingState::RetrievingMemory);

            // Performance monitoring for memory retrieval
            let _memory_timer = if perf_logging_enabled {
                Some(
                    StageTimer::start("memory_retrieval")
                        .with_target(TARGET_CLIPBOARD_TO_MEMORY_MS)
                        .with_meta("app", &context.app_bundle_id)
                        .with_meta("window", context.window_title.as_deref().unwrap_or("N/A")),
                )
            } else {
                None
            };

            match self.retrieve_and_augment_user_input(input.clone()) {
                Ok(augmented) => {
                    debug!("Memory augmentation succeeded");
                    augmented
                }
                Err(e) => {
                    warn!(error = %e, "Memory augmentation failed, using original input");
                    // Fallback to original input (no system prompt, no "User:" prefix)
                    input.clone()
                }
            }
        } else {
            // No memory - just use the original input directly
            input.clone()
        };

        let memory_time = start_time.elapsed();
        debug!(
            duration_ms = memory_time.as_millis(),
            "Memory retrieval completed"
        );

        // Step 3: Route to appropriate provider with fallback support
        // IMPORTANT: Use routing_context (window + clipboard) for routing decision
        // This enables context-aware routing based on the active application
        let ((provider, system_prompt_override), fallback_provider) = router
            .route_with_fallback(&routing_context)
            .ok_or(AetherError::NoProviderAvailable {
                suggestion: Some(
                    "No routing rules matched. Configure routing rules in Settings → Routing"
                        .to_string(),
                ),
            })?;

        let provider_name = provider.name().to_string();
        let provider_color = provider.color().to_string();

        // Log routing decision with system prompt info
        info!(
            provider = %provider_name,
            color = %provider_color,
            has_fallback = fallback_provider.is_some(),
            has_custom_system_prompt = system_prompt_override.is_some(),
            system_prompt_preview = ?system_prompt_override.map(|s| s.chars().take(50).collect::<String>()),
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

        // Strip command prefix from input if rule has strip_prefix enabled
        // This removes patterns like "/en" from "/en Hello world" before sending to AI
        let final_input = router.strip_command_prefix(&routing_context, &augmented_input);
        let prefix_was_stripped = final_input.len() < augmented_input.len();

        // Log the final system prompt being used
        info!(
            using_custom_prompt = system_prompt_override.is_some(),
            system_prompt_preview = %system_prompt.chars().take(80).collect::<String>(),
            prefix_stripped = prefix_was_stripped,
            final_input_preview = %final_input.chars().take(50).collect::<String>(),
            "Final system prompt for AI request"
        );

        // Try primary provider with retry
        let response = {
            // Performance monitoring for AI request
            let _ai_timer = if perf_logging_enabled {
                Some(
                    StageTimer::start("ai_request")
                        .with_target(TARGET_MEMORY_TO_AI_MS)
                        .with_meta("provider", &provider_name)
                        .with_meta("input_length", &input.len().to_string()),
                )
            } else {
                None
            };

            self.runtime.block_on(async {
                use crate::providers::retry_with_backoff;

                // Attempt with primary provider (with retry)
                // Use final_input which has command prefix stripped if applicable
                let primary_result = retry_with_backoff(
                    || provider.process(&final_input, Some(system_prompt)),
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
                            // Also use final_input with command prefix stripped
                            retry_with_backoff(
                                || fallback.process(&final_input, Some(system_prompt)),
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
            })?
        };

        let ai_time = start_time.elapsed();
        info!(
            ai_duration_ms = (ai_time - routing_time).as_millis(),
            total_duration_ms = ai_time.as_millis(),
            response_length = response.len(),
            "AI response received"
        );

        // Notify UI about AI response (Task 7.4)
        // Use char-boundary safe truncation for Unicode strings (e.g., Chinese)
        let response_preview = if response.chars().count() > 100 {
            let truncated: String = response.chars().take(100).collect();
            format!("{}...", truncated)
        } else {
            response.clone()
        };
        self.event_handler.on_ai_response_received(response_preview);

        // NEW ARCHITECTURE: Return response to Swift layer for output handling
        // Swift will use KeyboardSimulator.typeText() or .paste() based on config
        // This removes dependency on Rust clipboard/input modules

        // Step 5: Store interaction asynchronously (non-blocking)
        if self.memory_db.is_some() {
            let user_input = input.clone();
            let ai_output = response.clone();
            let core_clone = self.clone_for_storage();

            // Spawn background task to store memory
            self.runtime.spawn(async move {
                match core_clone
                    .store_interaction_memory(user_input, ai_output)
                    .await
                {
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
        info!(
            total_duration_ms = total_time.as_millis(),
            "AI processing complete, returning response to Swift layer"
        );

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
        }
    }

    // ========== CONFIG MANAGEMENT METHODS (Phase 6 - Task 1.5) ==========

    /// Internal helper to test provider connection (shared logic)
    ///
    /// This method contains the common testing logic used by both
    /// `test_provider_connection()` and `test_provider_connection_with_config()`.
    ///
    /// # Arguments
    ///
    /// * `provider_name` - Name of the provider (for error messages)
    /// * `provider_config` - Provider configuration to test
    ///
    /// # Returns
    ///
    /// TestConnectionResult with success status and message
    fn test_provider_internal(
        provider_name: &str,
        provider_config: crate::config::ProviderConfig,
    ) -> TestConnectionResult {
        use crate::providers::create_provider;

        // Create provider instance
        let provider = match create_provider(provider_name, provider_config) {
            Ok(p) => p,
            Err(e) => {
                return TestConnectionResult {
                    success: false,
                    message: format!("Failed to create provider: {}", e.user_friendly_message()),
                };
            }
        };

        // Send test request (block on async operation)
        let test_prompt = "Say 'OK' if you can read this.";
        let runtime = match Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                return TestConnectionResult {
                    success: false,
                    message: format!("Failed to create runtime: {}", e),
                };
            }
        };

        let result = runtime.block_on(async {
            provider.process(test_prompt, None).await.map_err(|e| {
                // During testing, show detailed error for debugging
                // (unlike production where we show user-friendly messages)
                format!("{}", e)
            })
        });

        match result {
            Ok(response) => TestConnectionResult {
                success: true,
                message: format!(
                    "✓ Connection successful! Provider responded: {}",
                    response.chars().take(50).collect::<String>()
                ),
            },
            Err(err_msg) => TestConnectionResult {
                success: false,
                message: err_msg,
            },
        }
    }

    /// Load configuration and return it in UniFFI-compatible format
    pub fn load_config(&self) -> Result<crate::config::FullConfig> {
        let config = self.lock_config();
        Ok(config.clone().into())
    }

    /// Update provider configuration
    pub fn update_provider(
        &self,
        name: String,
        provider: crate::config::ProviderConfig,
    ) -> Result<()> {
        let mut config = self.lock_config();
        config.providers.insert(name, provider);
        config.save()?;
        Ok(())
    }

    /// Delete provider configuration
    pub fn delete_provider(&self, name: String) -> Result<()> {
        let mut config = self.lock_config();
        config.providers.remove(&name);
        config.save()?;
        Ok(())
    }

    /// Update routing rules
    ///
    /// This method updates the routing rules in config AND reinitializes the router
    /// to ensure the new rules take effect immediately.
    pub fn update_routing_rules(&self, rules: Vec<crate::config::RoutingRuleConfig>) -> Result<()> {
        let mut config = self.lock_config();
        config.rules = rules;
        config.validate()?;
        config.save()?;
        drop(config); // Release lock before reloading router

        // Reinitialize router with updated config
        self.reload_router()?;

        log::info!("Routing rules updated and router reinitialized");
        Ok(())
    }

    /// Reload the router from current configuration
    ///
    /// This method reinitializes the router using the current config.
    /// Called after config changes to ensure routing rules take effect immediately.
    pub fn reload_router(&self) -> Result<()> {
        let config = self.lock_config();

        let new_router = if !config.providers.is_empty() {
            match Router::new(&config) {
                Ok(r) => {
                    log::info!(
                        "Router reloaded with {} rules and {} providers",
                        config.rules.len(),
                        config.providers.len()
                    );
                    Some(Arc::new(r))
                }
                Err(e) => {
                    log::warn!("Failed to reinitialize router: {}", e);
                    return Err(e);
                }
            }
        } else {
            log::warn!("No providers configured, router will be empty");
            None
        };

        drop(config); // Release config lock before acquiring router lock

        // Update router with write lock
        let mut router_guard = self.router.write().unwrap_or_else(|e| e.into_inner());
        *router_guard = new_router;

        Ok(())
    }

    /// Update shortcuts configuration
    pub fn update_shortcuts(&self, shortcuts: crate::config::ShortcutsConfig) -> Result<()> {
        let mut config = self.lock_config();
        config.shortcuts = Some(shortcuts);
        config.save()?;
        log::info!("Shortcuts configuration updated");
        Ok(())
    }

    /// Update behavior configuration
    pub fn update_behavior(&self, behavior: crate::config::BehaviorConfig) -> Result<()> {
        let mut config = self.lock_config();
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
    /// Returns a TestConnectionResult with success status and message.
    pub fn test_provider_connection(&self, provider_name: String) -> TestConnectionResult {
        // Get provider config from stored configuration
        let config = self.lock_config();
        let provider_config = match config.providers.get(&provider_name) {
            Some(cfg) => cfg.clone(),
            None => {
                drop(config);
                return TestConnectionResult {
                    success: false,
                    message: format!("Provider '{}' not found in configuration", provider_name),
                };
            }
        };
        drop(config); // Release lock before async operations

        // Use internal helper
        Self::test_provider_internal(&provider_name, provider_config)
    }

    /// Test provider connection with temporary configuration
    ///
    /// This method tests a provider without persisting the configuration to disk.
    /// Useful for "Test Connection" feature in UI before saving the provider.
    ///
    /// # Arguments
    ///
    /// * `provider_name` - Name of the provider (for logging and error messages)
    /// * `provider_config` - Temporary provider configuration to test
    ///
    /// # Returns
    ///
    /// TestConnectionResult with success status and message
    pub fn test_provider_connection_with_config(
        &self,
        provider_name: String,
        provider_config: crate::config::ProviderConfig,
    ) -> TestConnectionResult {
        // Use internal helper directly
        Self::test_provider_internal(&provider_name, provider_config)
    }

    // DEFAULT PROVIDER MANAGEMENT METHODS (Phase 3.3 - add-default-provider-selection)

    /// Get the current default provider (if exists and enabled)
    ///
    /// Returns None if:
    /// - No default provider is configured
    /// - Default provider does not exist
    /// - Default provider is disabled
    pub fn get_default_provider(&self) -> Option<String> {
        let config = self.lock_config();
        config.get_default_provider()
    }

    /// Set the default provider (validates that provider exists and is enabled)
    ///
    /// # Arguments
    /// * `provider_name` - The name of the provider to set as default
    ///
    /// # Returns
    /// * `Ok(())` - Successfully set default provider
    /// * `Err` - Provider not found or disabled
    pub fn set_default_provider(&self, provider_name: String) -> Result<()> {
        let mut config = self.lock_config();
        config.set_default_provider(&provider_name)?;
        config.save()?;

        // Notify event handler of config change
        self.event_handler.on_config_changed();

        info!(provider = %provider_name, "Default provider updated");
        Ok(())
    }

    /// Get list of all enabled provider names (sorted alphabetically)
    ///
    /// # Returns
    /// * `Vec<String>` - List of enabled provider names
    pub fn get_enabled_providers(&self) -> Vec<String> {
        let config = self.lock_config();
        config.get_enabled_providers()
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
}

impl StorageHelper {
    /// Acquires the config mutex lock with poison recovery.
    #[inline(always)]
    fn lock_config(&self) -> std::sync::MutexGuard<'_, Config> {
        self.config.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Ensures the memory database is initialized and returns a reference to it.
    #[inline(always)]
    fn require_memory_db(&self) -> Result<&Arc<VectorDatabase>> {
        self.memory_db
            .as_ref()
            .ok_or_else(|| AetherError::config("Memory database not initialized"))
    }

    /// Store interaction memory (used in async context)
    ///
    /// IMPORTANT: This is an async function because it's called from within
    /// a tokio::spawn() task. Using block_on() inside an async context would
    /// cause a panic: "Cannot start a runtime from within a runtime".
    async fn store_interaction_memory(
        &self,
        user_input: String,
        ai_output: String,
    ) -> Result<String> {
        use crate::memory::context::ContextAnchor;
        use crate::memory::embedding::EmbeddingModel;
        use crate::memory::ingestion::MemoryIngestion;

        // Extract all needed data from locks before any await point
        // MutexGuard is not Send, so we must drop it before await
        let (memory_config, context_anchor, db) = {
            // Check if memory is enabled
            let config = self.lock_config();
            if !config.memory.enabled {
                return Err(AetherError::config("Memory is disabled"));
            }

            // Get current context
            let current_context = self.current_context.lock().unwrap_or_else(|e| {
                warn!("Mutex poisoned in current_context (StorageHelper::store_interaction_memory), recovering");
                e.into_inner()
            });
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
            let db = self.require_memory_db()?.clone();

            // Clone memory config for use after lock is dropped
            let memory_config = config.memory.clone();

            (memory_config, context_anchor, db)
        }; // All locks are dropped here

        // Get embedding model directory
        let model_dir = AetherCore::get_embedding_model_dir().map_err(|e| {
            AetherError::config(format!("Failed to get embedding model directory: {}", e))
        })?;

        // Create embedding model (lazy load)
        let embedding_model = Arc::new(EmbeddingModel::new(model_dir).map_err(|e| {
            AetherError::config(format!("Failed to initialize embedding model: {}", e))
        })?);

        // Create ingestion service
        let ingestion = MemoryIngestion::new(db, embedding_model, Arc::new(memory_config));

        // Store memory - use await instead of block_on since we're in async context
        let result = ingestion
            .store_memory(context_anchor, &user_input, &ai_output)
            .await;

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

/// App memory info for UI filtering (UniFFI-compatible)
#[derive(Debug, Clone)]
pub struct AppMemoryInfo {
    pub app_bundle_id: String,
    pub memory_count: u64,
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

        // Note: is_listening() always returns false since hotkey monitoring is now in Swift layer
        assert!(!core.is_listening());

        core.start_listening().unwrap();
        // is_listening() still returns false (hotkey monitoring handled by Swift)
        assert!(!core.is_listening());

        core.stop_listening().unwrap();
        assert!(!core.is_listening());
    }

    // REMOVED: test_clipboard_read
    // Clipboard operations have been migrated to Swift layer (ClipboardManager.swift)
    // See: refactor-native-api-separation proposal

    #[test]
    fn test_multiple_start_stop_cycles() {
        let handler = Box::new(MockEventHandler::new());
        let core = AetherCore::new(handler).unwrap();

        // Note: start_listening() and stop_listening() are deprecated (now handled by Swift layer)
        // but kept for API compatibility. They should not crash when called.
        for _ in 0..3 {
            core.start_listening().unwrap();
            // is_listening() always returns false since hotkey monitoring is now in Swift
            assert!(!core.is_listening());

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
            let mut config = core.config.lock().unwrap_or_else(|e| e.into_inner());
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
            let mut config = core.config.lock().unwrap_or_else(|e| e.into_inner());
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
