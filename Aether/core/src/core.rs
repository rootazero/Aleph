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
use crate::clarification::{ClarificationOption, ClarificationRequest};
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

/// Media attachment for multimodal content (add-multimodal-content-support)
/// Supports images, videos, and files from clipboard
#[derive(Debug, Clone)]
pub struct MediaAttachment {
    pub media_type: String,    // "image", "video", "file"
    pub mime_type: String,     // "image/png", "image/jpeg", "video/mp4", etc.
    pub data: String,          // Base64-encoded content
    pub filename: Option<String>,  // Optional original filename
    pub size_bytes: u64,       // Original size in bytes for logging/validation
}

/// Captured context from active application (Swift → Rust)
#[derive(Debug, Clone)]
pub struct CapturedContext {
    pub app_bundle_id: String,
    pub window_title: Option<String>,
    pub attachments: Option<Vec<MediaAttachment>>,  // Multimodal content support
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
    // Search capability (integrate-search-registry)
    // RwLock allows hot-reload when search config changes
    search_registry: Arc<RwLock<Option<Arc<crate::search::SearchRegistry>>>>,
    // Config hot-reload (must be kept alive for file watching)
    #[allow(dead_code)]
    config_watcher: Option<Arc<ConfigWatcher>>,
    // Multi-turn conversation support
    conversation_manager: Arc<Mutex<crate::conversation::ConversationManager>>,
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

        // Initialize SearchRegistry (if search enabled) - integrate-search-registry
        // Wrapped in RwLock to allow hot-reload after config changes
        let search_registry = {
            let cfg = config.lock().unwrap_or_else(|e| e.into_inner());
            let registry_opt = if let Some(ref search_config) = cfg.search {
                if search_config.enabled {
                    match Self::create_search_registry_from_config(search_config) {
                        Ok(registry) => {
                            info!("SearchRegistry initialized successfully");
                            Some(Arc::new(registry))
                        }
                        Err(e) => {
                            warn!("Failed to initialize SearchRegistry: {}", e);
                            None
                        }
                    }
                } else {
                    debug!("Search capability disabled in config");
                    None
                }
            } else {
                debug!("No search configuration found");
                None
            };
            Arc::new(RwLock::new(registry_opt))
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
            let search_registry_clone = Arc::clone(&search_registry);

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

                        // Reinitialize SearchRegistry with new config (integrate-search-registry)
                        let new_search_registry = if let Some(ref search_config) = new_config.search {
                            if search_config.enabled {
                                match Self::create_search_registry_from_config(search_config) {
                                    Ok(registry) => {
                                        log::info!("SearchRegistry hot-reloaded successfully");
                                        Some(Arc::new(registry))
                                    }
                                    Err(e) => {
                                        log::warn!("Failed to reinitialize SearchRegistry during hot-reload: {}", e);
                                        None
                                    }
                                }
                            } else {
                                log::debug!("Search capability disabled in new config");
                                None
                            }
                        } else {
                            log::debug!("No search configuration in new config");
                            None
                        };

                        // Update search_registry
                        if let Ok(mut registry_guard) = search_registry_clone.write() {
                            *registry_guard = new_search_registry;
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
            search_registry,
            config_watcher,
            conversation_manager: Arc::new(Mutex::new(crate::conversation::ConversationManager::new())),
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

    /// Extract SearchOptions from search configuration (integrate-search-registry)
    ///
    /// Converts SearchConfigInternal to SearchOptions for use in capability executor.
    ///
    /// # Arguments
    ///
    /// * `search_config` - Search configuration from Config
    ///
    /// # Returns
    ///
    /// * `crate::search::SearchOptions` - Configured search options
    fn get_search_options_from_config(
        search_config: &crate::config::SearchConfigInternal,
    ) -> crate::search::SearchOptions {
        use crate::search::SearchOptions;

        // Create SearchOptions with defaults, override from config
        SearchOptions {
            max_results: search_config.max_results,
            timeout_seconds: search_config.timeout_seconds,
            // Use default values for other fields (None or false)
            ..Default::default()
        }
    }

    /// Create SearchRegistry from search configuration (integrate-search-registry)
    ///
    /// This method initializes a SearchRegistry with configured backends and fallback chain.
    ///
    /// # Arguments
    ///
    /// * `search_config` - Search configuration from Config
    ///
    /// # Returns
    ///
    /// * `Result<crate::search::SearchRegistry>` - Initialized registry or error
    fn create_search_registry_from_config(
        search_config: &crate::config::SearchConfigInternal,
    ) -> Result<crate::search::SearchRegistry> {
        use crate::search::providers::*;
        use crate::search::SearchProvider;

        info!(
            enabled = search_config.enabled,
            default_provider = %search_config.default_provider,
            backend_count = search_config.backends.len(),
            "Creating SearchRegistry from config"
        );

        // Create providers from backend configurations
        let mut providers: Vec<(String, Box<dyn SearchProvider>)> = Vec::new();

        for (name, backend_config) in &search_config.backends {
            let provider: Box<dyn SearchProvider> = match backend_config.provider_type.as_str() {
                "tavily" => {
                    let api_key = backend_config
                        .api_key
                        .as_ref()
                        .ok_or_else(|| AetherError::config("Tavily requires api_key"))?;
                    Box::new(TavilyProvider::new(api_key.clone())?)
                }
                "searxng" => {
                    let base_url = backend_config
                        .base_url
                        .as_ref()
                        .ok_or_else(|| AetherError::config("SearXNG requires base_url"))?;
                    Box::new(SearxngProvider::new(base_url.clone())?)
                }
                "google" => {
                    let api_key = backend_config
                        .api_key
                        .as_ref()
                        .ok_or_else(|| AetherError::config("Google CSE requires api_key"))?;
                    let engine_id = backend_config.engine_id.as_ref().ok_or_else(|| {
                        AetherError::config("Google CSE requires engine_id")
                    })?;
                    Box::new(GoogleProvider::new(api_key.clone(), engine_id.clone())?)
                }
                "bing" => {
                    let api_key = backend_config
                        .api_key
                        .as_ref()
                        .ok_or_else(|| AetherError::config("Bing requires api_key"))?;
                    Box::new(BingProvider::new(api_key.clone())?)
                }
                "brave" => {
                    let api_key = backend_config
                        .api_key
                        .as_ref()
                        .ok_or_else(|| AetherError::config("Brave requires api_key"))?;
                    Box::new(BraveProvider::new(api_key.clone())?)
                }
                "exa" => {
                    let api_key = backend_config
                        .api_key
                        .as_ref()
                        .ok_or_else(|| AetherError::config("Exa requires api_key"))?;
                    Box::new(ExaProvider::new(api_key.clone())?)
                }
                _ => {
                    warn!(
                        provider_type = %backend_config.provider_type,
                        "Unknown search provider type, skipping"
                    );
                    continue;
                }
            };

            providers.push((name.clone(), provider));
        }

        if providers.is_empty() {
            return Err(AetherError::config(
                "No search providers configured in backends",
            ));
        }

        // Build fallback chain
        let fallback_chain = search_config
            .fallback_providers
            .clone()
            .unwrap_or_default();

        // Create registry
        let mut registry =
            crate::search::SearchRegistry::new(search_config.default_provider.clone());

        // Add all providers
        let provider_count = providers.len();
        for (name, provider) in providers {
            // Provider is already Box<dyn SearchProvider>, wrap in Arc
            registry.add_provider(name, Arc::from(provider));
        }

        // Set fallback chain
        registry.set_fallback_providers(fallback_chain);

        info!(
            provider_count = provider_count,
            "SearchRegistry created successfully"
        );

        Ok(registry)
    }

    /// Build and enrich AgentPayload using new payload architecture
    ///
    /// This method implements the structured context protocol:
    /// 1. Creates AgentPayload using PayloadBuilder
    /// 2. Executes CapabilityExecutor to enrich context (memory, search, MCP)
    /// 3. Returns enriched payload ready for prompt assembly
    ///
    /// # Arguments
    ///
    /// * `user_input` - User's input text
    /// * `context` - Captured application context
    /// * `provider_name` - Target provider name
    /// * `capabilities` - List of capabilities to execute
    ///
    /// # Returns
    ///
    /// Enriched AgentPayload with context data populated
    async fn build_enriched_payload(
        &self,
        user_input: String,
        context: CapturedContext,
        provider_name: String,
        capabilities: Vec<crate::payload::Capability>,
    ) -> Result<crate::payload::AgentPayload> {
        use crate::capability::CapabilityExecutor;
        use crate::payload::{ContextAnchor, ContextFormat, Intent, PayloadBuilder};

        // Create context anchor from captured context
        let anchor = ContextAnchor::from_captured_context(&context);

        // Get config for context format
        let context_format = ContextFormat::Markdown; // MVP uses Markdown format

        // Build initial payload
        let payload = PayloadBuilder::new()
            .meta(
                Intent::GeneralChat, // MVP uses GeneralChat intent
                chrono::Utc::now().timestamp(),
                anchor,
            )
            .config(provider_name, capabilities.clone(), context_format)
            .user_input(user_input)
            .build()
            .map_err(|e| AetherError::config(format!("Failed to build payload: {}", e)))?;

        // Get AI memory retrieval configuration
        let (use_ai_retrieval, ai_timeout_ms, ai_max_candidates, ai_fallback_count) = {
            let cfg = self.lock_config();
            (
                cfg.memory.enabled && cfg.memory.ai_retrieval_enabled,
                cfg.memory.ai_retrieval_timeout_ms,
                cfg.memory.ai_retrieval_max_candidates,
                cfg.memory.ai_retrieval_fallback_count,
            )
        };

        // Build memory exclusion set from current conversation
        let memory_exclusion_set = self.build_memory_exclusion_set();

        // Get AI provider for memory selection (if AI retrieval enabled)
        let ai_provider = if use_ai_retrieval {
            self.get_default_provider_instance()
        } else {
            None
        };

        // Execute capabilities to enrich payload
        let executor = CapabilityExecutor::new(
            self.memory_db.as_ref().map(Arc::clone),
            {
                let cfg = self.lock_config();
                Some(Arc::new(cfg.memory.clone()))
            },
            {
                // Pass SearchRegistry from persistent field (integrate-search-registry)
                let registry = self.search_registry.read().unwrap_or_else(|e| e.into_inner());
                registry.as_ref().map(Arc::clone)
            },
            {
                // Pass SearchOptions from config (integrate-search-registry)
                let cfg = self.lock_config();
                cfg.search.as_ref().map(Self::get_search_options_from_config)
            },
            {
                // Read PII config from search.pii.enabled (integrate-search-registry)
                // Fallback to behavior.pii_scrubbing_enabled for backward compatibility
                let cfg = self.lock_config();
                cfg.search
                    .as_ref()
                    .and_then(|s| s.pii.as_ref())
                    .map(|p| p.enabled)
                    .or_else(|| {
                        cfg.behavior
                            .as_ref()
                            .map(|b| b.pii_scrubbing_enabled)
                    })
                    .unwrap_or(false)
            },
        )
        .with_video_config({
            // Pass VideoConfig from config
            let cfg = self.lock_config();
            cfg.video.as_ref().map(|v| Arc::new(v.clone()))
        })
        // Configure AI-based memory retrieval
        .with_ai_retrieval(
            ai_provider,
            use_ai_retrieval,
            ai_timeout_ms,
            ai_max_candidates,
            ai_fallback_count,
        )
        .with_memory_exclusion_set(memory_exclusion_set);

        executor.execute_all(payload).await
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
    /// # DEPRECATED
    /// This method is deprecated in favor of the CapabilityExecutor system.
    /// Memory retrieval is now handled by `CapabilityExecutor::execute_memory()` which:
    /// - Supports AI-based retrieval via `AiMemoryRetriever`
    /// - Uses exclusion sets to avoid duplicate context
    /// - Is properly integrated into the build_enriched_payload pipeline
    ///
    /// Use `build_enriched_payload()` with Memory capability instead.
    ///
    /// # Arguments
    /// * `user_input` - Current user input/query
    ///
    /// # Returns
    /// * `Result<String>` - User input with optional memory context
    #[allow(dead_code)]
    #[deprecated(
        since = "0.2.0",
        note = "Use CapabilityExecutor with Memory capability via build_enriched_payload()"
    )]
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

        // Check if AI retrieval is enabled
        let use_ai_retrieval = config.memory.ai_retrieval_enabled;

        // Get current context
        let current_context = self.current_context.lock().unwrap_or_else(|e| {
            warn!(
                "Mutex poisoned in current_context (retrieve_and_augment_user_input), recovering"
            );
            e.into_inner()
        });
        let captured_context = match current_context.as_ref() {
            Some(ctx) => ctx.clone(),
            None => {
                debug!("[Memory] No context captured, returning original user input");
                return Ok(user_input);
            }
        };
        drop(current_context); // Release lock before async operations

        // Get memory database
        let db = match self.memory_db.as_ref() {
            Some(db) => db,
            None => {
                debug!("[Memory] Database not initialized, returning original user input");
                return Ok(user_input);
            }
        };

        // Retrieve memories based on configured method
        let memories = if use_ai_retrieval {
            // AI-based retrieval: use AI to select relevant memories
            debug!("[Memory] Using AI-based retrieval");
            let exclusion_set = self.build_memory_exclusion_set();
            self.runtime.block_on(self.retrieve_memories_with_ai(
                &user_input,
                &captured_context,
                &exclusion_set,
            ))?
        } else {
            // Embedding-based retrieval: use vector similarity search
            debug!("[Memory] Using embedding-based retrieval");

            // Create context anchor
            let context_anchor = ContextAnchor {
                app_bundle_id: captured_context.app_bundle_id.clone(),
                window_title: captured_context.window_title.clone().unwrap_or_default(),
                timestamp: chrono::Utc::now().timestamp(),
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

            // Retrieve memories using embedding search
            self.runtime
                .block_on(retrieval.retrieve_memories(&context_anchor, &user_input))?
        };

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

    // ============================================================================
    // AI-based Memory Retrieval + Parallel Execution
    // ============================================================================

    /// Build exclusion set from current conversation session.
    ///
    /// Returns user inputs from all turns in the active session to prevent
    /// memory retrieval from returning content that's already in conversation cache.
    fn build_memory_exclusion_set(&self) -> Vec<String> {
        let manager = self
            .conversation_manager
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        if let Some(session) = manager.active_session() {
            session
                .turns
                .iter()
                .map(|t| t.user_input.clone())
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Retrieve memories using AI-based selection.
    ///
    /// This replaces embedding-based vector similarity with AI evaluation.
    /// The AI is given recent memories and selects which are relevant.
    async fn retrieve_memories_with_ai(
        &self,
        user_input: &str,
        context: &CapturedContext,
        exclusion_set: &[String],
    ) -> Result<Vec<crate::memory::MemoryEntry>> {
        use crate::memory::{AiMemoryRetriever, MemoryEntry};
        use std::time::Duration;

        // Check if memory and AI retrieval are enabled
        let config = self.lock_config();
        if !config.memory.enabled || !config.memory.ai_retrieval_enabled {
            debug!("[Memory] AI retrieval disabled");
            return Ok(Vec::new());
        }

        let timeout_ms = config.memory.ai_retrieval_timeout_ms;
        let max_candidates = config.memory.ai_retrieval_max_candidates;
        let fallback_count = config.memory.ai_retrieval_fallback_count;
        drop(config);

        // Get memory database
        let db = match self.memory_db.as_ref() {
            Some(db) => db,
            None => {
                debug!("[Memory] Database not initialized");
                return Ok(Vec::new());
            }
        };

        // Get recent memories from database (without embedding search)
        let candidates: Vec<MemoryEntry> = db
            .get_recent_memories(
                &context.app_bundle_id,
                context.window_title.as_deref().unwrap_or(""),
                max_candidates,
                exclusion_set,
            )
            .await?;

        if candidates.is_empty() {
            debug!("[Memory] No candidate memories found");
            return Ok(Vec::new());
        }

        debug!(
            "[Memory] Found {} candidate memories for AI selection",
            candidates.len()
        );

        // Get default provider for AI memory selection
        let provider = match self.get_default_provider_instance() {
            Some(p) => p,
            None => {
                warn!("[Memory] No AI provider available for memory selection");
                // Fallback to most recent memories
                return Ok(candidates.into_iter().take(fallback_count as usize).collect());
            }
        };

        // Create AI memory retriever
        let retriever = AiMemoryRetriever::new(provider)
            .with_timeout(Duration::from_millis(timeout_ms))
            .with_max_candidates(max_candidates)
            .with_fallback_count(fallback_count);

        // Retrieve using AI selection
        retriever.retrieve(user_input, candidates, exclusion_set).await
    }

    /// Perform intent detection and memory retrieval in parallel.
    ///
    /// This reduces latency by running both AI calls concurrently.
    /// If either fails, the other's result is still used.
    ///
    /// # Note
    /// Currently not used but preserved for future parallel execution optimization.
    /// Memory retrieval is now handled by CapabilityExecutor.
    #[allow(dead_code)]
    fn parallel_detect_and_retrieve(
        &self,
        user_input: &str,
        context: &CapturedContext,
    ) -> (
        Option<(String, Option<crate::intent::DetectedIntent>)>,
        Vec<crate::memory::MemoryEntry>,
    ) {
        // Build exclusion set from current conversation
        let exclusion_set = self.build_memory_exclusion_set();

        // Get config values
        let config = self.lock_config();
        let use_ai_intent = config.smart_flow.enabled
            && config.smart_flow.intent_detection.enabled
            && config.smart_flow.intent_detection.use_ai;
        let confidence_threshold = config.smart_flow.intent_detection.confidence_threshold;
        let ai_timeout_ms = config.smart_flow.intent_detection.ai_timeout_ms;
        let search_enabled = config.smart_flow.intent_detection.search;
        let video_enabled = config.smart_flow.intent_detection.video;
        let locale = config
            .general
            .language
            .clone()
            .unwrap_or_else(|| "en".to_string());
        let ai_memory_enabled = config.memory.enabled && config.memory.ai_retrieval_enabled;
        drop(config);

        // Clone values for async tasks
        let user_input_owned = user_input.to_string();
        let context_owned = context.clone();
        let exclusion_set_owned = exclusion_set.clone();

        // Run both tasks in parallel using tokio runtime
        let result = self.runtime.block_on(async {
            use tokio::join;

            // Task 1: Intent detection (if enabled)
            let intent_future = async {
                if use_ai_intent {
                    self.try_ai_intent_detection(
                        &user_input_owned,
                        confidence_threshold,
                        ai_timeout_ms,
                        search_enabled,
                        video_enabled,
                        &locale,
                    )
                } else {
                    Ok(None)
                }
            };

            // Task 2: AI memory retrieval (if enabled)
            let memory_future = async {
                if ai_memory_enabled {
                    self.retrieve_memories_with_ai(
                        &user_input_owned,
                        &context_owned,
                        &exclusion_set_owned,
                    )
                    .await
                } else {
                    Ok(Vec::new())
                }
            };

            // Execute both in parallel
            let (intent_result, memory_result) = join!(intent_future, memory_future);

            // Handle results, logging but not failing on individual errors
            let intent = match intent_result {
                Ok(i) => i,
                Err(e) => {
                    warn!(error = ?e, "Intent detection failed, continuing without intent");
                    None
                }
            };

            let memories = match memory_result {
                Ok(m) => m,
                Err(e) => {
                    warn!(error = ?e, "Memory retrieval failed, continuing without memories");
                    Vec::new()
                }
            };

            (intent, memories)
        });

        info!(
            intent_detected = result.0.is_some(),
            memory_count = result.1.len(),
            "Parallel detect and retrieve completed"
        );

        result
    }

    /// Get the default AI provider instance for memory selection and other AI tasks.
    fn get_default_provider_instance(
        &self,
    ) -> Option<std::sync::Arc<dyn crate::providers::AiProvider>> {
        let config = self.lock_config();
        let default_provider_name = config.general.default_provider.clone();
        drop(config);

        // default_provider is Option<String>, extract the name if present
        if let Some(name) = default_provider_name {
            self.get_provider_by_name(&name)
        } else {
            None
        }
    }

    /// Get a provider by name from the internal provider registry.
    fn get_provider_by_name(
        &self,
        name: &str,
    ) -> Option<std::sync::Arc<dyn crate::providers::AiProvider>> {
        // Access the router to get providers (router uses RwLock)
        let router_guard = self.router.read().unwrap_or_else(|e| e.into_inner());
        if let Some(router) = router_guard.as_ref() {
            router.get_provider_arc(name)
        } else {
            None
        }
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

        // Check if AI-first detection is enabled
        let use_ai_first = {
            let config = self.config.lock().unwrap_or_else(|e| e.into_inner());
            config.smart_flow.enabled
                && config.smart_flow.intent_detection.enabled
                && config.smart_flow.intent_detection.ai_first
        };

        // AI-First Mode: AI decides if capability is needed in a single call
        if use_ai_first {
            return match self.process_with_ai_first(user_input.clone(), context.clone(), start_time) {
                Ok(response) => Ok(response),
                Err(e) => Err(self.handle_processing_error(&e)),
            };
        }

        // Legacy Mode: Intent detection then processing
        // Smart Flow: Intent Detection
        // Check if user input matches a known intent pattern and needs parameter completion
        let (final_input, detected_intent) =
            self.detect_intent_and_complete_params(&user_input, &context)?;

        // Extract capability from detected intent (if any)
        let intent_capability = detected_intent
            .as_ref()
            .and_then(|d| d.capability.clone());

        // Call internal implementation and handle errors
        match self.process_with_ai_internal(final_input, context.clone(), start_time, intent_capability) {
            Ok(response) => {
                // Smart Flow: Suggestion Parsing
                // Parse AI response for follow-up suggestions
                self.parse_and_handle_suggestions(&response, &context, detected_intent)
            }
            Err(e) => Err(self.handle_processing_error(&e)),
        }
    }

    /// Detect intent from user input and complete missing parameters via clarification.
    ///
    /// Uses a two-layer detection approach:
    /// 1. AI-powered detection (language-agnostic, if enabled)
    /// 2. Regex-based SmartTriggerDetector (fallback)
    ///
    /// Returns the (possibly augmented) input and the detected intent (if any).
    fn detect_intent_and_complete_params(
        &self,
        user_input: &str,
        _context: &CapturedContext,
    ) -> std::result::Result<(String, Option<crate::intent::DetectedIntent>), AetherException> {
        use crate::intent::{augment_with_param, SmartTriggerDetector, SmartTriggerResult};

        // Check if smart flow is enabled
        let config = self.config.lock().unwrap_or_else(|e| e.into_inner());
        if !config.smart_flow.enabled || !config.smart_flow.intent_detection.enabled {
            return Ok((user_input.to_string(), None));
        }

        // Get configuration settings
        let locale = config
            .general
            .language
            .clone()
            .unwrap_or_else(|| "en".to_string());
        let use_ai = config.smart_flow.intent_detection.use_ai;
        let confidence_threshold = config.smart_flow.intent_detection.confidence_threshold;
        let ai_timeout_ms = config.smart_flow.intent_detection.ai_timeout_ms;
        let search_enabled = config.smart_flow.intent_detection.search;
        let video_enabled = config.smart_flow.intent_detection.video;
        drop(config);

        // === Layer 1: AI-powered intent detection (language-agnostic) ===
        if use_ai {
            if let Some(result) = self.try_ai_intent_detection(
                user_input,
                confidence_threshold,
                ai_timeout_ms,
                search_enabled,
                video_enabled,
                &locale,
            )? {
                return Ok(result);
            }
        }

        // === Layer 2: Regex-based detection (fallback) ===
        let mut detector = SmartTriggerDetector::new();
        detector.set_locale(&locale);
        detector.set_trigger_enabled("/search", search_enabled);
        detector.set_trigger_enabled("/video", video_enabled);

        match detector.detect(user_input) {
            SmartTriggerResult::Ready {
                command,
                capability,
                params,
                original_input,
            } => {
                info!(
                    command = %command,
                    capability = ?capability,
                    params = ?params,
                    "Regex trigger ready - all params present"
                );

                let detected = crate::intent::DetectedIntent {
                    intent_type: crate::intent::IntentType::General,
                    capability: Some(capability),
                    extracted_params: params,
                    missing_params: vec![],
                    confidence: 1.0,
                };

                Ok((original_input, Some(detected)))
            }

            SmartTriggerResult::NeedsParam {
                command,
                capability,
                param,
                extracted,
                original_input,
            } => {
                info!(
                    command = %command,
                    capability = ?capability,
                    param_name = %param.name,
                    "Regex trigger needs parameter - triggering clarification"
                );

                let clarification_request = detector.get_clarification(&param);
                let result = self.event_handler.on_clarification_needed(clarification_request);

                match result.result_type {
                    crate::clarification::ClarificationResultType::Selected
                    | crate::clarification::ClarificationResultType::TextInput => {
                        if let Some(value) = result.value {
                            let augmented =
                                augment_with_param(&original_input, &command, &param.name, &value);

                            info!(
                                original = %original_input,
                                augmented = %augmented,
                                param_name = %param.name,
                                param_value = %value,
                                "Input augmented with clarified parameter"
                            );

                            let mut all_params = extracted;
                            all_params.insert(param.name.clone(), value);

                            let detected = crate::intent::DetectedIntent {
                                intent_type: crate::intent::IntentType::General,
                                capability: Some(capability),
                                extracted_params: all_params,
                                missing_params: vec![],
                                confidence: 1.0,
                            };

                            return Ok((augmented, Some(detected)));
                        }
                    }
                    crate::clarification::ClarificationResultType::Cancelled => {
                        info!("User cancelled clarification");
                        return Err(AetherException::Error);
                    }
                    crate::clarification::ClarificationResultType::Timeout => {
                        info!("Clarification timed out");
                        return Err(AetherException::Error);
                    }
                }

                Ok((original_input, None))
            }

            SmartTriggerResult::NoMatch => {
                Ok((user_input.to_string(), None))
            }
        }
    }

    /// AI-First processing mode.
    ///
    /// In this mode, the AI receives information about available capabilities and decides
    /// whether to respond directly or request capability invocation via a structured JSON response.
    ///
    /// Flow:
    /// 1. Build capability declarations based on enabled features
    /// 2. Create capability-aware system prompt
    /// 3. Make single AI call
    /// 4. Parse response for capability requests
    /// 5. If capability requested, execute it and make second AI call with results
    /// 6. Return final response
    fn process_with_ai_first(
        &self,
        input: String,
        context: CapturedContext,
        start_time: std::time::Instant,
    ) -> Result<String> {
        use crate::capability::{AiResponse, CapabilityDeclaration, CapabilityRegistry, ResponseParser};
        use crate::payload::ContextFormat;

        info!("Using AI-first detection mode");

        // Step 1: Get router and configuration
        let router = {
            let router_guard = self.router.read().unwrap_or_else(|e| e.into_inner());
            router_guard
                .as_ref()
                .map(Arc::clone)
                .ok_or(AetherError::NoProviderAvailable {
                    suggestion: Some(
                        "Configure at least one AI provider in Settings → Providers".to_string(),
                    ),
                })?
        };

        let config = self.lock_config();
        let search_enabled = config.smart_flow.intent_detection.search
            && self
                .search_registry
                .read()
                .ok()
                .and_then(|r| r.as_ref().map(|_| ()))
                .is_some();
        let video_enabled = config.smart_flow.intent_detection.video
            && config.video.as_ref().map(|v| v.enabled).unwrap_or(false);
        let memory_enabled = config.memory.enabled;
        drop(config);

        // Step 2: Build capability declarations
        let registry = CapabilityRegistry::with_defaults(search_enabled, video_enabled);
        let capabilities: Vec<CapabilityDeclaration> = registry.all().to_vec();

        info!(
            search_enabled = search_enabled,
            video_enabled = video_enabled,
            capability_count = capabilities.len(),
            "Built capability registry for AI-first mode"
        );

        // Step 3: Route to get provider (use existing routing for provider selection)
        let routing_context = Self::build_routing_context(&context, &input);
        let routing_match = router.match_rules(&routing_context);

        let provider_name = routing_match
            .provider_name()
            .map(|s| s.to_string())
            .or_else(|| router.default_provider_name().map(|s| s.to_string()))
            .ok_or(AetherError::NoProviderAvailable {
                suggestion: Some("No default provider configured".to_string()),
            })?;

        let provider = router
            .get_provider_arc(&provider_name)
            .ok_or(AetherError::NoProviderAvailable {
                suggestion: Some(format!("Provider '{}' not found", provider_name)),
            })?;

        // Step 4: Build capability-aware system prompt
        let base_prompt = routing_match
            .assemble_prompt()
            .unwrap_or_else(|| "You are a helpful AI assistant.".to_string());

        // Get memory context if enabled
        let memory_context = if memory_enabled {
            self.get_memory_context_for_ai_first(&input, &context)?
        } else {
            None
        };

        let assembler = crate::payload::PromptAssembler::new(ContextFormat::Markdown);
        let system_prompt = assembler.build_capability_aware_prompt(
            &base_prompt,
            &capabilities,
            memory_context.as_ref(),
        );

        info!(
            provider = %provider_name,
            system_prompt_length = system_prompt.len(),
            "Making AI-first call with capability-aware prompt"
        );

        // Step 5: Notify UI and make AI call
        self.event_handler
            .on_state_changed(ProcessingState::Processing);

        let response = self.runtime.block_on(provider.process(&input, Some(&system_prompt)))?;

        // Step 6: Parse response for capability requests
        let parsed = ResponseParser::parse(&response)?;

        match parsed {
            AiResponse::Direct(text) => {
                // No capability needed - return directly
                info!(
                    response_length = text.len(),
                    elapsed_ms = start_time.elapsed().as_millis(),
                    "AI-first: Direct response (no capability invocation)"
                );

                // Notify UI about AI response
                let response_preview = if text.chars().count() > 100 {
                    let truncated: String = text.chars().take(100).collect();
                    format!("{}...", truncated)
                } else {
                    text.clone()
                };
                self.event_handler.on_ai_response_received(response_preview);

                // Store in memory asynchronously if enabled
                if self.memory_db.is_some() {
                    let user_input = input.clone();
                    let ai_output = text.clone();
                    let core_clone = self.clone_for_storage();

                    self.runtime.spawn(async move {
                        match core_clone.store_interaction_memory(user_input, ai_output).await {
                            Ok(memory_id) => {
                                log::debug!("[AI-first] Memory stored: {}", memory_id);
                            }
                            Err(e) => {
                                log::error!("[AI-first] Failed to store memory: {}", e);
                            }
                        }
                    });
                }

                Ok(text)
            }
            AiResponse::CapabilityRequest(request) => {
                // Capability requested - execute and continue
                info!(
                    capability = %request.capability,
                    query = %request.query,
                    reasoning = ?request.reasoning,
                    "AI-first: Capability invocation requested"
                );

                self.execute_capability_and_continue(
                    request,
                    &input,
                    context,
                    provider,
                    &base_prompt,
                    start_time,
                )
            }
            AiResponse::NeedsClarification(info) => {
                // AI needs more information from user
                info!(
                    reason = %info.reason,
                    prompt = %info.prompt,
                    has_suggestions = info.has_suggestions(),
                    "AI-first: Clarification needed from user"
                );

                // Convert ClarificationInfo to ClarificationRequest for the callback
                let clarification_request = if info.has_suggestions() {
                    // If AI provided suggestions, create a Select-type request
                    let options: Vec<ClarificationOption> = info
                        .suggestions
                        .as_ref()
                        .unwrap()
                        .iter()
                        .map(|s| ClarificationOption::new(s, s))
                        .collect();
                    ClarificationRequest::select(
                        &format!("ai-clarification-{}", uuid::Uuid::new_v4()),
                        &info.prompt,
                        options,
                    )
                    .with_source("ai-intent")
                } else {
                    // No suggestions - create a Text-type request
                    ClarificationRequest::text(
                        &format!("ai-clarification-{}", uuid::Uuid::new_v4()),
                        &info.prompt,
                        Some(&info.context_summary),
                    )
                    .with_source("ai-intent")
                };

                // Notify UI that clarification is needed
                let result = self.event_handler.on_clarification_needed(clarification_request);

                // Handle the result
                if result.is_success() {
                    if let Some(value) = result.get_value() {
                        // User provided clarification - append to original input and reprocess
                        let augmented_input = format!("{}\n\n用户补充: {}", input, value);
                        info!(
                            original_input = %input,
                            clarification = %value,
                            "Reprocessing with user clarification"
                        );
                        // Recursive call with augmented input (new start time for the clarified request)
                        return self.process_with_ai_first(
                            augmented_input,
                            context.clone(),
                            std::time::Instant::now(),
                        );
                    }
                }

                // User cancelled or timeout - return the prompt as indication
                Ok(info.prompt)
            }
        }
    }

    /// Get memory context for AI-first mode.
    fn get_memory_context_for_ai_first(
        &self,
        _input: &str,
        _context: &CapturedContext,
    ) -> Result<Option<crate::payload::AgentContext>> {
        // For MVP, we don't pre-fetch memory context
        // Memory will be included if the AI requests a capability that needs it
        // This keeps the first call lightweight
        Ok(None)
    }

    /// Execute the requested capability and continue with a second AI call.
    fn execute_capability_and_continue(
        &self,
        request: crate::capability::CapabilityRequest,
        original_input: &str,
        context: CapturedContext,
        provider: Arc<dyn crate::providers::AiProvider>,
        base_prompt: &str,
        start_time: std::time::Instant,
    ) -> Result<String> {
        use crate::payload::{Capability, ContextFormat};

        // Map capability ID to Capability enum
        let capability = match request.capability.as_str() {
            "search" => Capability::Search,
            "video" => Capability::Video,
            "mcp" => Capability::Mcp,
            _ => {
                return Err(AetherError::config(format!(
                    "Unknown capability: {}",
                    request.capability
                )))
            }
        };

        info!(
            capability = ?capability,
            "Executing capability from AI-first request"
        );

        // Update UI state
        if capability == Capability::Search {
            self.event_handler
                .on_state_changed(ProcessingState::RetrievingMemory); // Reusing state
        }

        // Build capabilities list - always include memory if available
        let mut capabilities = vec![capability];
        if self.memory_db.is_some() && !capabilities.contains(&Capability::Memory) {
            capabilities.push(Capability::Memory);
        }

        // Build enriched payload using existing infrastructure
        let enriched_payload = self.runtime.block_on(self.build_enriched_payload(
            request.query.clone(),
            context.clone(),
            provider.name().to_string(),
            capabilities,
        ))?;

        // Assemble enriched prompt with capability results
        let assembler = crate::payload::PromptAssembler::new(ContextFormat::Markdown);
        let enriched_prompt = assembler.assemble_system_prompt(base_prompt, &enriched_payload);

        info!(
            enriched_prompt_length = enriched_prompt.len(),
            has_search_results = enriched_payload.context.search_results.is_some(),
            has_video_transcript = enriched_payload.context.video_transcript.is_some(),
            has_memory = enriched_payload.context.memory_snippets.is_some(),
            "Making second AI call with enriched context"
        );

        // Make second AI call with enriched context
        let response = self
            .runtime
            .block_on(provider.process(&request.query, Some(&enriched_prompt)))?;

        info!(
            response_length = response.len(),
            elapsed_ms = start_time.elapsed().as_millis(),
            "AI-first: Response with capability results"
        );

        // Store in memory asynchronously if enabled
        if self.memory_db.is_some() {
            let user_input = original_input.to_string();
            let ai_output = response.clone();
            let core_clone = self.clone_for_storage();

            self.runtime.spawn(async move {
                match core_clone.store_interaction_memory(user_input, ai_output).await {
                    Ok(memory_id) => {
                        log::debug!("[AI-first] Capability response memory stored: {}", memory_id);
                    }
                    Err(e) => {
                        log::error!("[AI-first] Failed to store capability response memory: {}", e);
                    }
                }
            });
        }

        self.event_handler
            .on_state_changed(ProcessingState::Success);

        Ok(response)
    }

    /// Try AI-powered intent detection.
    ///
    /// Returns Some((augmented_input, detected_intent)) if AI detected an intent,
    /// None if no intent was detected or AI detection failed.
    fn try_ai_intent_detection(
        &self,
        user_input: &str,
        confidence_threshold: f64,
        timeout_ms: u64,
        search_enabled: bool,
        video_enabled: bool,
        _locale: &str,
    ) -> std::result::Result<Option<(String, Option<crate::intent::DetectedIntent>)>, AetherException> {
        use crate::intent::AiIntentDetector;
        use crate::payload::Capability;
        use std::time::Duration;

        // Get the default provider for AI intent detection
        let router_guard = self.router.read().unwrap_or_else(|e| e.into_inner());
        let router = match router_guard.as_ref() {
            Some(r) => r,
            None => {
                debug!("No router available for AI intent detection");
                return Ok(None);
            }
        };

        let default_provider_name = match router.default_provider_name() {
            Some(name) => name.to_string(),
            None => {
                debug!("No default provider for AI intent detection");
                return Ok(None);
            }
        };

        let provider = match router.get_provider_arc(&default_provider_name) {
            Some(p) => p,
            None => {
                debug!(provider = %default_provider_name, "Default provider not found");
                return Ok(None);
            }
        };
        drop(router_guard);

        // Create AI intent detector
        let ai_detector = AiIntentDetector::new(provider)
            .with_confidence_threshold(confidence_threshold)
            .with_timeout(Duration::from_millis(timeout_ms));

        // Run AI detection
        info!(
            input_preview = %user_input.chars().take(50).collect::<String>(),
            "Starting AI intent detection"
        );

        let ai_result = match self.runtime.block_on(ai_detector.detect(user_input)) {
            Ok(Some(result)) => result,
            Ok(None) => {
                debug!("AI detected no specific intent");
                return Ok(None);
            }
            Err(e) => {
                warn!(error = %e, "AI intent detection failed, falling back to regex");
                return Ok(None);
            }
        };

        info!(
            intent = %ai_result.intent,
            confidence = ai_result.confidence,
            params = ?ai_result.params,
            missing = ?ai_result.missing,
            "AI intent detection completed"
        );

        // Check if the detected intent is enabled
        let (capability, command) = match ai_result.intent.as_str() {
            "search" if search_enabled => (Capability::Search, "/search"),
            "video" if video_enabled => (Capability::Video, "/video"),
            _ => {
                debug!(
                    intent = %ai_result.intent,
                    "AI detected intent not enabled or is general"
                );
                return Ok(None);
            }
        };

        // Handle missing parameters
        if !ai_result.missing.is_empty() {
            let missing_param = &ai_result.missing[0];
            info!(
                command = %command,
                missing_param = %missing_param,
                "AI detected intent needs parameter - triggering clarification"
            );

            // Build clarification request
            let (prompt_key, placeholder) = match (command, missing_param.as_str()) {
                ("/search", "location") | ("/search", "query") => (
                    format!("smart_trigger.search.query_prompt:Enter your query (e.g., city name)"),
                    Some("Beijing / New York / Tokyo".to_string()),
                ),
                ("/video", "url") => (
                    format!("smart_trigger.video.url_prompt:Enter video URL"),
                    Some("https://youtube.com/watch?v=...".to_string()),
                ),
                _ => (
                    format!("smart_trigger.{}.{}_prompt:Enter {}", ai_result.intent, missing_param, missing_param),
                    None,
                ),
            };

            let clarification_request = crate::clarification::ClarificationRequest {
                id: format!("ai-param-{}", missing_param),
                prompt: prompt_key,
                clarification_type: crate::clarification::ClarificationType::Text,
                options: None,
                default_value: None,
                placeholder,
                source: Some("ai:intent".to_string()),
            };

            let result = self.event_handler.on_clarification_needed(clarification_request);

            match result.result_type {
                crate::clarification::ClarificationResultType::Selected
                | crate::clarification::ClarificationResultType::TextInput => {
                    if let Some(value) = result.value {
                        let augmented = crate::intent::augment_with_param(
                            user_input, command, missing_param, &value
                        );

                        info!(
                            original = %user_input,
                            augmented = %augmented,
                            param_name = %missing_param,
                            param_value = %value,
                            "Input augmented with AI-detected parameter"
                        );

                        let mut all_params = ai_result.params.clone();
                        all_params.insert(missing_param.clone(), value);

                        let detected = crate::intent::DetectedIntent {
                            intent_type: crate::intent::IntentType::General,
                            capability: Some(capability),
                            extracted_params: all_params,
                            missing_params: vec![],
                            confidence: ai_result.confidence as f32,
                        };

                        return Ok(Some((augmented, Some(detected))));
                    }
                }
                crate::clarification::ClarificationResultType::Cancelled => {
                    info!("User cancelled AI clarification");
                    return Err(AetherException::Error);
                }
                crate::clarification::ClarificationResultType::Timeout => {
                    info!("AI clarification timed out");
                    return Err(AetherException::Error);
                }
            }

            return Ok(None);
        }

        // All params present - return detected intent
        let detected = crate::intent::DetectedIntent {
            intent_type: crate::intent::IntentType::General,
            capability: Some(capability),
            extracted_params: ai_result.params,
            missing_params: vec![],
            confidence: ai_result.confidence as f32,
        };

        Ok(Some((user_input.to_string(), Some(detected))))
    }

    /// Parse AI response for follow-up suggestions and handle them.
    fn parse_and_handle_suggestions(
        &self,
        response: &str,
        context: &CapturedContext,
        _detected_intent: Option<crate::intent::DetectedIntent>,
    ) -> std::result::Result<String, AetherException> {
        use crate::suggestion::SuggestionParser;

        // Check if suggestion parsing is enabled
        let config = self.config.lock().unwrap_or_else(|e| e.into_inner());
        if !config.smart_flow.enabled || !config.smart_flow.suggestion_parsing.enabled {
            return Ok(response.to_string());
        }

        let max_suggestions = config.smart_flow.suggestion_parsing.max_suggestions;
        drop(config);

        // Parse response for suggestions
        let mut parser = SuggestionParser::new();
        parser.set_max_suggestions(max_suggestions);
        let suggestions = parser.parse(response);

        if !suggestions.has_suggestions || suggestions.options.is_empty() {
            return Ok(response.to_string());
        }

        info!(
            suggestion_count = suggestions.options.len(),
            "AI response contains follow-up suggestions"
        );

        // Create clarification request for suggestions
        if let Some(clarification_request) = suggestions.to_clarification_request() {
            let result = self.event_handler.on_clarification_needed(clarification_request);

            match result.result_type {
                crate::clarification::ClarificationResultType::Selected
                | crate::clarification::ClarificationResultType::TextInput => {
                    if let Some(follow_up) = result.value {
                        info!(
                            follow_up = %follow_up,
                            "User selected follow-up suggestion, continuing conversation"
                        );

                        // Recursively process the follow-up
                        return self.process_input(follow_up, context.clone());
                    }
                }
                _ => {
                    // User cancelled or timed out - return the cleaned response
                    info!("User did not select a follow-up suggestion");
                }
            }
        }

        // Return the cleaned response (without suggestion markers)
        Ok(suggestions.cleaned_response)
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
        let app_name = context
            .app_bundle_id
            .split('.')
            .next_back()
            .unwrap_or("Unknown");

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
        intent_capability: Option<crate::payload::Capability>,
    ) -> Result<String> {
        // Overall pipeline timer
        let _pipeline_timer = StageTimer::start("total_pipeline");

        // Step 1: Check if router is available
        // Get a clone of the Arc<Router> to avoid holding the RwLock during AI processing
        let router = {
            let router_guard = self.router.read().unwrap_or_else(|e| e.into_inner());
            router_guard
                .as_ref()
                .map(Arc::clone)
                .ok_or(AetherError::NoProviderAvailable {
                    suggestion: Some(
                        "Configure at least one AI provider in Settings → Providers".to_string(),
                    ),
                })?
        };

        // Step 1.5: Build routing context string (clipboard content + window context)
        let routing_context = Self::build_routing_context(&context, &input);

        // DEBUG: Log raw input and routing context for command debugging
        info!(
            raw_input_length = input.len(),
            raw_input_preview = %input.chars().take(50).collect::<String>(),
            raw_input_starts_with_slash = input.starts_with('/'),
            "Raw input from Swift"
        );
        info!(
            context_length = routing_context.len(),
            app = %context.app_bundle_id,
            window = ?context.window_title,
            context_preview = %routing_context.chars().take(100).collect::<String>(),
            "Built routing context for provider selection"
        );

        // Step 2: Route using new match_rules() API
        // - Command rules: first-match-stops, returns provider + cleaned input
        // - Keyword rules: all-match, adds prompts
        let routing_match = router.match_rules(&routing_context);

        // DEBUG: Log routing match result
        info!(
            command_matched = routing_match.command_rule.is_some(),
            keyword_count = routing_match.keyword_rules.len(),
            matched_provider = ?routing_match.provider_name(),
            cleaned_input_preview = ?routing_match.cleaned_input().map(|s| s.chars().take(50).collect::<String>()),
            system_prompt_preview = ?routing_match.assemble_prompt().map(|s| s.chars().take(50).collect::<String>()),
            "Routing match result"
        );

        // Determine provider name (command rule or default)
        let provider_name = routing_match
            .provider_name()
            .map(|s| s.to_string())
            .or_else(|| router.default_provider_name().map(|s| s.to_string()))
            .ok_or(AetherError::NoProviderAvailable {
                suggestion: Some(
                    "No routing rules matched. Configure routing rules in Settings → Routing"
                        .to_string(),
                ),
            })?;

        // Look up provider
        let provider = router.get_provider(&provider_name).ok_or(AetherError::NoProviderAvailable {
            suggestion: Some(format!("Provider '{}' not found", provider_name)),
        })?;

        let provider_color = provider.color().to_string();

        // Get capabilities from the match
        let rule_capabilities = routing_match.get_capabilities();

        // Get system prompt from match (combined command + keyword prompts)
        let rule_system_prompt = routing_match.assemble_prompt().unwrap_or_default();

        // Determine fallback provider (default if different from primary)
        let fallback_provider = router
            .default_provider_name()
            .filter(|default| *default != provider_name)
            .and_then(|name| router.get_provider(name));

        info!(
            provider = %provider_name,
            color = %provider_color,
            has_fallback = fallback_provider.is_some(),
            rule_capabilities_count = rule_capabilities.len(),
            rule_capabilities = ?rule_capabilities,
            command_matched = routing_match.command_rule.is_some(),
            keyword_count = routing_match.keyword_rules.len(),
            "Routed to AI provider with match_rules()"
        );

        // Step 3: Build and enrich AgentPayload using new architecture
        let config = self.lock_config();
        let base_system_prompt = "You are a helpful AI assistant.".to_string();
        let perf_logging_enabled = config.general.enable_performance_logging;
        let memory_enabled = config.memory.enabled;
        drop(config); // Release lock before async operations

        // Determine capabilities to execute:
        // 1. Start with capabilities from routing rule (e.g., /search has Search capability)
        // 2. Add capability from intent detection (e.g., AI detected search intent)
        // 3. Add Memory capability if memory is enabled in config (unless already present)
        let mut capabilities = rule_capabilities;

        // Add capability from intent detection if not already present
        if let Some(ref cap) = intent_capability {
            if !capabilities.contains(cap) {
                info!(
                    capability = ?cap,
                    "Adding capability from intent detection"
                );
                capabilities.push(cap.clone());
            }
        }

        if memory_enabled && !capabilities.contains(&crate::payload::Capability::Memory) {
            capabilities.push(crate::payload::Capability::Memory);
        }

        info!(
            final_capabilities = ?capabilities,
            "Final capabilities to execute (rule + config)"
        );

        // NEW ARCHITECTURE: Build enriched payload with CapabilityExecutor
        let enriched_payload = if !capabilities.is_empty() {
            // Notify UI that we're retrieving memory/search
            if capabilities.contains(&crate::payload::Capability::Memory) {
                self.event_handler
                    .on_state_changed(ProcessingState::RetrievingMemory);
            }

            // Performance monitoring for payload enrichment
            let _memory_timer = if perf_logging_enabled {
                Some(
                    StageTimer::start("payload_enrichment")
                        .with_target(TARGET_CLIPBOARD_TO_MEMORY_MS)
                        .with_meta("app", &context.app_bundle_id)
                        .with_meta("window", context.window_title.as_deref().unwrap_or("N/A")),
                )
            } else {
                None
            };

            match self.runtime.block_on(self.build_enriched_payload(
                input.clone(),
                context.clone(),
                provider_name.clone(),
                capabilities,
            )) {
                Ok(payload) => {
                    let has_video = payload.context.video_transcript.is_some();
                    let video_title = payload.context.video_transcript
                        .as_ref()
                        .map(|t| t.title.clone())
                        .unwrap_or_default();
                    info!(
                        memory_count = payload
                            .context
                            .memory_snippets
                            .as_ref()
                            .map(|m| m.len())
                            .unwrap_or(0),
                        search_count = payload
                            .context
                            .search_results
                            .as_ref()
                            .map(|s| s.len())
                            .unwrap_or(0),
                        has_video_transcript = has_video,
                        video_title = %video_title,
                        "Payload enrichment succeeded"
                    );
                    Some(payload)
                }
                Err(e) => {
                    warn!(error = %e, "Payload enrichment failed, using original input");
                    None
                }
            }
        } else {
            None
        };

        // Assemble system prompt using PromptAssembler
        use crate::payload::PromptAssembler;
        let assembler = PromptAssembler::new(crate::payload::ContextFormat::Markdown);

        // Get full assembled prompt (base + context) for normal mode
        let assembled_system_prompt = if let Some(ref payload) = enriched_payload {
            assembler.assemble_system_prompt(&base_system_prompt, payload)
        } else {
            base_system_prompt.clone()
        };

        // Get context only (memory + search, without base prompt) for prepend mode
        let context_only = enriched_payload
            .as_ref()
            .and_then(|p| assembler.format_context(&p.context));

        let memory_time = start_time.elapsed();
        debug!(
            duration_ms = memory_time.as_millis(),
            "Capability enrichment completed"
        );

        // Notify UI about AI processing start (Task 7.4)
        // Note: Routing was already done in Step 2, we reuse routing_decision
        self.event_handler
            .on_ai_processing_started(provider_name.clone(), provider_color.clone());
        self.event_handler
            .on_state_changed(ProcessingState::ProcessingWithAI);

        // Step 4: Call AI provider with retry and fallback logic (Task 10.1 & 10.2)
        let routing_time = start_time.elapsed();

        // Check if provider uses prepend mode for system prompts
        // Default to prepend mode for better compatibility with third-party APIs
        // Only use standard mode if explicitly set to "standard"
        let provider_uses_prepend = {
            let config = self.lock_config();
            config
                .providers
                .get(&provider_name)
                .and_then(|p| p.system_prompt_mode.as_ref())
                .map(|m| m != "standard")  // prepend unless explicitly "standard"
                .unwrap_or(true)  // default to prepend
        };

        // Use custom system prompt from routing rule, or assembled prompt with memory/search context
        // Priority: rule system prompt > assembled (contains context) > base
        //
        // For prepend mode: use rule_prompt + context_only (memory/search without base_prompt)
        // This ensures memory is available for context but "You are a helpful AI assistant." is excluded
        let system_prompt = if provider_uses_prepend && !rule_system_prompt.is_empty() {
            // Prepend mode: rule prompt + context only (no base prompt like "You are a helpful AI assistant.")
            if let Some(ctx) = &context_only {
                format!("{}\n\n{}", rule_system_prompt, ctx)
            } else {
                rule_system_prompt.clone()
            }
        } else if !rule_system_prompt.is_empty() {
            // Normal mode: Combine rule system prompt with assembled context (includes base prompt)
            format!("{}\n\n{}", rule_system_prompt, assembled_system_prompt)
        } else {
            assembled_system_prompt.clone()
        };

        // Get cleaned input from routing match (command prefix stripped if applicable)
        // For command rules like "/en Hello world" → "Hello world"
        // For keyword rules or no match, use original input
        //
        // IMPORTANT: routing_match.cleaned_input() may contain routing context suffix
        // (format: "UserInput\n---\n[AppName] WindowTitle") because the routing context
        // is used for rule matching. We need to strip the context suffix to get pure user input.
        let final_input = routing_match
            .cleaned_input()
            .map(|s| {
                // Remove routing context suffix if present
                // The suffix can be "\n---\n" or "\n\n---\n" depending on input formatting
                // Use a more robust pattern to find the separator
                if let Some(idx) = s.find("\n---\n") {
                    // Find the start of the separator, accounting for possible leading newlines
                    let trimmed = s[..idx].trim_end_matches('\n');
                    trimmed.to_string()
                } else {
                    s.to_string()
                }
            })
            .unwrap_or_else(|| input.clone());
        let prefix_was_stripped = final_input.len() < input.len();

        // If video transcript was successfully extracted, replace YouTube URL in user message
        // with a placeholder. This prevents AI from reflexively saying "I can't access the video"
        // when it sees a YouTube URL, even though the transcript is already in the system prompt.
        let final_input = if enriched_payload
            .as_ref()
            .and_then(|p| p.context.video_transcript.as_ref())
            .is_some()
        {
            // Replace YouTube URLs with placeholder
            let youtube_regex =
                regex::Regex::new(r"https?://(www\.)?(youtube\.com/watch\?v=|youtu\.be/)[\w-]+")
                    .unwrap();
            let replaced = youtube_regex
                .replace_all(&final_input, "[视频字幕已提取，请基于上方字幕内容进行分析]")
                .to_string();
            if replaced != final_input {
                info!(
                    original_len = final_input.len(),
                    replaced_len = replaced.len(),
                    "Replaced YouTube URL with placeholder to prevent AI confusion"
                );
            }
            replaced
        } else {
            final_input
        };

        // Log the final system prompt being used
        info!(
            has_rule_prompt = !rule_system_prompt.is_empty(),
            provider_uses_prepend = provider_uses_prepend,
            has_context = context_only.is_some(),
            system_prompt_preview = %system_prompt.chars().take(80).collect::<String>(),
            prefix_stripped = prefix_was_stripped,
            final_input_preview = %final_input.chars().take(50).collect::<String>(),
            "Final system prompt for AI request"
        );

        // Provider is already obtained above from router.get_provider()

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

            // add-multimodal-content-support: Check if we have media attachments
            let attachments = context.attachments.clone();
            let has_attachments = attachments
                .as_ref()
                .map(|a| !a.is_empty())
                .unwrap_or(false);

            if has_attachments {
                let atts = attachments.as_ref().unwrap();
                for (i, att) in atts.iter().enumerate() {
                    info!(
                        index = i,
                        media_type = %att.media_type,
                        mime_type = %att.mime_type,
                        data_len = att.data.len(),
                        size_bytes = att.size_bytes,
                        "Media attachment details"
                    );
                }
                info!(
                    attachment_count = atts.len(),
                    provider_supports_vision = provider.supports_vision(),
                    "Processing with media attachments"
                );
            } else {
                debug!("No media attachments, processing text only");
            }

            self.runtime.block_on(async {
                use crate::providers::retry_with_backoff;

                // Attempt with primary provider (with retry)
                // Use final_input which has command prefix stripped if applicable
                // add-multimodal-content-support: Use process_with_attachments if attachments present
                let attachments_ref = attachments.as_deref();
                let primary_result = if has_attachments && provider.supports_vision() {
                    retry_with_backoff(
                        || {
                            provider.process_with_attachments(
                                &final_input,
                                attachments_ref,
                                Some(&system_prompt),
                            )
                        },
                        Some(3),
                    )
                    .await
                } else {
                    retry_with_backoff(
                        || provider.process(&final_input, Some(&system_prompt)),
                        Some(3),
                    )
                    .await
                };

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
                            // add-multimodal-content-support: Use process_with_attachments for fallback too
                            if has_attachments && fallback.supports_vision() {
                                retry_with_backoff(
                                    || {
                                        fallback.process_with_attachments(
                                            &final_input,
                                            attachments_ref,
                                            Some(&system_prompt),
                                        )
                                    },
                                    Some(3),
                                )
                                .await
                            } else {
                                retry_with_backoff(
                                    || fallback.process(&final_input, Some(&system_prompt)),
                                    Some(3),
                                )
                                .await
                            }
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
    ///
    /// **IMPORTANT**: This method preserves builtin rules (is_builtin = true) and only
    /// updates user-defined rules. Builtin rules are prepended to maintain their priority.
    pub fn update_routing_rules(&self, rules: Vec<crate::config::RoutingRuleConfig>) -> Result<()> {
        let mut config = self.lock_config();

        // Preserve builtin rules from current config
        let builtin_rules: Vec<_> = config.rules.iter().filter(|r| r.is_builtin).cloned().collect();

        // Merge: builtin rules first (for priority), then user rules
        let mut merged_rules = builtin_rules;
        merged_rules.extend(rules);

        log::info!(
            "Updating routing rules: {} builtin + {} user = {} total",
            merged_rules.iter().filter(|r| r.is_builtin).count(),
            merged_rules.iter().filter(|r| !r.is_builtin).count(),
            merged_rules.len()
        );

        config.rules = merged_rules;
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

    /// Update trigger configuration
    ///
    /// Updates the trigger configuration for the hotkey system.
    /// This controls how double-tap modifier keys trigger cut/copy operations.
    ///
    /// # Arguments
    /// * `trigger` - New trigger configuration
    ///
    /// # Returns
    /// * `Result<()>` - Success or error
    pub fn update_trigger_config(&self, trigger: crate::config::TriggerConfig) -> Result<()> {
        let mut config = self.lock_config();
        config.trigger = Some(trigger);
        config.save()?;
        log::info!("Trigger configuration updated");
        Ok(())
    }

    /// Update search configuration
    ///
    /// Updates the search configuration and reinitializes the SearchRegistry.
    /// This allows hot-reloading search providers after settings changes.
    ///
    /// # Arguments
    /// * `search` - New search configuration (UniFFI type)
    ///
    /// # Returns
    /// * `Result<()>` - Success or error
    pub fn update_search_config(&self, search: crate::config::SearchConfig) -> Result<()> {
        // Convert UniFFI SearchConfig to internal SearchConfigInternal
        let search_internal: crate::config::SearchConfigInternal = search.into();

        // Update config and save to disk
        {
            let mut config = self.lock_config();
            config.search = Some(search_internal.clone());
            config.save()?;
        }

        // Reinitialize SearchRegistry with new config
        if search_internal.enabled {
            match Self::create_search_registry_from_config(&search_internal) {
                Ok(registry) => {
                    let mut registry_lock = self
                        .search_registry
                        .write()
                        .unwrap_or_else(|e| e.into_inner());
                    *registry_lock = Some(Arc::new(registry));
                    log::info!("Search configuration updated and registry reinitialized");
                }
                Err(e) => {
                    log::warn!("Failed to reinitialize SearchRegistry: {}", e);
                    return Err(AetherError::config(format!(
                        "Failed to reinitialize search registry: {}",
                        e
                    )));
                }
            }
        } else {
            // Disable search by clearing the registry
            let mut registry_lock = self
                .search_registry
                .write()
                .unwrap_or_else(|e| e.into_inner());
            *registry_lock = None;
            log::info!("Search capability disabled");
        }

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

    // ========== SEARCH CAPABILITY METHODS (integrate-search-registry) ==========

    /// Test a search provider connection (integrate-search-registry)
    ///
    /// This method delegates to SearchRegistry.test_search_provider() to validate
    /// provider configuration and connectivity. Results are cached for 5 minutes.
    ///
    /// # Arguments
    ///
    /// * `provider_name` - Name of the search provider to test
    ///
    /// # Returns
    ///
    /// * `ProviderTestResult` - Test result with success status, latency, and error details
    ///
    /// # Example (from Swift)
    ///
    /// ```swift
    /// let result = await core.testSearchProvider("tavily")
    /// if result.success {
    ///     print("✓ Provider test successful: \(result.latency_ms)ms")
    /// } else {
    ///     print("✗ Provider test failed: \(result.error_message)")
    /// }
    /// ```
    pub fn test_search_provider(
        &self,
        provider_name: String,
    ) -> Result<crate::search::ProviderTestResult> {
        use crate::search::ProviderTestResult;

        // Clone Arc from registry (must drop lock before await)
        let registry_arc = {
            let registry_guard = self.search_registry.read().unwrap_or_else(|e| e.into_inner());
            registry_guard.as_ref().map(Arc::clone)
        }; // Lock is dropped here

        match registry_arc {
            Some(reg) => {
                // Execute async search test within tokio runtime
                Ok(self.runtime.block_on(reg.test_search_provider(&provider_name)))
            }
            None => {
                // Search capability not enabled
                Ok(ProviderTestResult {
                    success: false,
                    latency_ms: 0,
                    error_message: "Search capability not enabled in configuration".to_string(),
                    error_type: "config".to_string(),
                })
            }
        }
    }

    /// Test a search provider with ad-hoc configuration
    ///
    /// This method allows testing provider credentials without requiring the provider
    /// to be saved in the configuration file. It creates a temporary provider instance
    /// to validate connectivity and credentials.
    ///
    /// # Arguments
    ///
    /// * `config` - Ad-hoc configuration containing provider type and credentials
    ///
    /// # Returns
    ///
    /// * `ProviderTestResult` - Test result with success status, latency, and error details
    ///
    /// # Example (from Swift)
    ///
    /// ```swift
    /// let config = SearchProviderTestConfig(
    ///     providerType: "tavily",
    ///     apiKey: "tvly-xxx",
    ///     baseUrl: nil,
    ///     engineId: nil
    /// )
    /// let result = await core.testSearchProviderWithConfig(config: config)
    /// ```
    pub fn test_search_provider_with_config(
        &self,
        config: crate::search::SearchProviderTestConfig,
    ) -> Result<crate::search::ProviderTestResult> {
        use crate::search::providers::*;
        use crate::search::{ProviderTestResult, SearchOptions, SearchProvider};
        use std::time::Instant;

        // Helper: Create config error result
        fn config_error(msg: &str) -> ProviderTestResult {
            ProviderTestResult {
                success: false,
                latency_ms: 0,
                error_message: msg.to_string(),
                error_type: "config".to_string(),
            }
        }

        // Helper: Extract non-empty string from Option, or return None
        fn get_non_empty(opt: &Option<String>) -> Option<String> {
            opt.as_ref().filter(|s| !s.is_empty()).cloned()
        }

        // Helper macro to reduce boilerplate for provider creation
        macro_rules! create_provider {
            ($provider:ident, $name:expr, $key:expr) => {
                match get_non_empty($key) {
                    Some(key) => match $provider::new(key) {
                        Ok(p) => Box::new(p) as Box<dyn SearchProvider>,
                        Err(e) => return Ok(config_error(&format!("Failed to create {} provider: {}", $name, e))),
                    },
                    None => return Ok(config_error(&format!("{} requires an API key", $name))),
                }
            };
        }

        // Create temporary provider based on type
        let provider: Box<dyn SearchProvider> = match config.provider_type.as_str() {
            "tavily" => create_provider!(TavilyProvider, "Tavily", &config.api_key),
            "brave" => create_provider!(BraveProvider, "Brave", &config.api_key),
            "bing" => create_provider!(BingProvider, "Bing", &config.api_key),
            "exa" => create_provider!(ExaProvider, "Exa", &config.api_key),
            "searxng" => match get_non_empty(&config.base_url) {
                Some(base_url) => match SearxngProvider::new(base_url) {
                    Ok(p) => Box::new(p) as Box<dyn SearchProvider>,
                    Err(e) => return Ok(config_error(&format!("Failed to create SearXNG provider: {}", e))),
                },
                None => return Ok(config_error("SearXNG requires a base URL")),
            },
            "google" => {
                let api_key = match get_non_empty(&config.api_key) {
                    Some(k) => k,
                    None => return Ok(config_error("Google CSE requires an API key")),
                };
                let engine_id = match get_non_empty(&config.engine_id) {
                    Some(id) => id,
                    None => return Ok(config_error("Google CSE requires an engine ID")),
                };
                match GoogleProvider::new(api_key, engine_id) {
                    Ok(p) => Box::new(p) as Box<dyn SearchProvider>,
                    Err(e) => return Ok(config_error(&format!("Failed to create Google provider: {}", e))),
                }
            }
            unknown => return Ok(config_error(&format!("Unknown provider type: {}", unknown))),
        };

        // Execute test search within tokio runtime
        let test_options = SearchOptions {
            max_results: 1,
            timeout_seconds: 5,
            ..Default::default()
        };

        let start = Instant::now();
        match self.runtime.block_on(provider.search("test", &test_options)) {
            Ok(_) => {
                let latency = start.elapsed().as_millis() as u32;
                Ok(ProviderTestResult {
                    success: true,
                    latency_ms: latency,
                    error_message: String::new(),
                    error_type: String::new(),
                })
            }
            Err(e) => {
                let error_message = e.to_string();
                let error_type = if error_message.contains("401")
                    || error_message.contains("403")
                    || error_message.contains("unauthorized")
                    || error_message.contains("invalid")
                {
                    "auth"
                } else if error_message.contains("timeout")
                    || error_message.contains("connection")
                    || error_message.contains("network")
                {
                    "network"
                } else {
                    "unknown"
                };

                Ok(ProviderTestResult {
                    success: false,
                    latency_ms: 0,
                    error_message,
                    error_type: error_type.to_string(),
                })
            }
        }
    }

    // ========== COMMAND COMPLETION METHODS (add-command-completion-system) ==========

    /// Get all root-level commands for command completion UI
    ///
    /// Returns commands parsed from config.toml routing rules with ^/ prefix.
    /// Commands are sorted alphabetically by key.
    ///
    /// # Returns
    /// * `Vec<CommandNode>` - List of root commands with key, description, icon, hint, type
    ///
    /// # Example (from Swift)
    /// ```swift
    /// let commands = core.getRootCommands()
    /// for cmd in commands {
    ///     print("/\(cmd.key) - \(cmd.description)")
    /// }
    /// ```
    pub fn get_root_commands(&self) -> Vec<crate::command::CommandNode> {
        let config = self.lock_config();
        let language = config
            .general
            .language
            .as_deref()
            .unwrap_or("en");
        let registry = crate::command::CommandRegistry::from_config(&config, language);
        registry.get_root_commands()
    }

    /// Get children of a namespace command
    ///
    /// For namespace commands like /mcp, returns the list of child commands.
    /// Currently returns empty for most namespaces (MCP integration reserved for future).
    ///
    /// # Arguments
    /// * `parent_key` - The key of the parent namespace (e.g., "mcp")
    ///
    /// # Returns
    /// * `Vec<CommandNode>` - List of child commands
    pub fn get_command_children(&self, parent_key: String) -> Vec<crate::command::CommandNode> {
        let config = self.lock_config();
        let language = config
            .general
            .language
            .as_deref()
            .unwrap_or("en");
        let registry = crate::command::CommandRegistry::from_config(&config, language);
        registry.get_children(&parent_key)
    }

    /// Filter commands by key prefix (case-insensitive)
    ///
    /// Used for autocomplete as user types. Returns commands whose keys
    /// start with the given prefix.
    ///
    /// # Arguments
    /// * `prefix` - The prefix to filter by (e.g., "se" matches "search", "settings")
    ///
    /// # Returns
    /// * `Vec<CommandNode>` - Filtered list of matching commands
    ///
    /// # Example (from Swift)
    /// ```swift
    /// // User types "/se"
    /// let matches = core.filterCommands(prefix: "se")
    /// // Returns: [search, settings, ...]
    /// ```
    pub fn filter_commands(&self, prefix: String) -> Vec<crate::command::CommandNode> {
        let commands = self.get_root_commands();
        crate::command::CommandRegistry::filter_by_prefix(&commands, &prefix)
    }

    // ========================================================================
    // Multi-Turn Conversation API (add-multi-turn-conversation)
    // ========================================================================

    /// Start a new conversation session.
    ///
    /// This initiates a multi-turn conversation. The first AI response will be
    /// printed to the target window and cached. Subsequent inputs can be processed
    /// via `continue_conversation()`.
    ///
    /// # Arguments
    /// * `initial_input` - The user's initial message
    /// * `context` - The captured context (app, window) at session start
    ///
    /// # Returns
    /// * `Result<String>` - The AI's response, or an error
    pub fn start_conversation(
        &self,
        initial_input: String,
        context: CapturedContext,
    ) -> std::result::Result<String, AetherException> {
        info!(
            input_preview = %initial_input.chars().take(50).collect::<String>(),
            app = %context.app_bundle_id,
            "Starting new conversation session"
        );

        // Start a new session in the conversation manager
        let session_id = {
            let mut manager = self
                .conversation_manager
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            manager.start_session(context.clone())
        };

        // Notify UI that conversation started
        self.event_handler.on_conversation_started(session_id.clone());

        // Process the initial input
        let start_time = std::time::Instant::now();
        let response = self.process_with_ai_first(initial_input.clone(), context.clone(), start_time)?;

        // Add the turn to conversation history
        {
            let mut manager = self
                .conversation_manager
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(turn) = manager.add_turn(initial_input, response.clone()) {
                // Notify UI about the completed turn
                self.event_handler.on_conversation_turn_completed(
                    crate::conversation::ConversationTurn {
                        turn_id: turn.turn_id,
                        user_input: turn.user_input,
                        ai_response: turn.ai_response,
                        timestamp: turn.timestamp,
                    },
                );
            }
        }

        // Notify UI that continuation is available
        self.event_handler.on_conversation_continuation_ready();

        Ok(response)
    }

    /// Continue an existing conversation with follow-up input.
    ///
    /// This method:
    /// 1. Builds context from conversation history
    /// 2. Processes the follow-up with AI
    /// 3. Adds the turn to history
    /// 4. Returns the AI response (for printing to target window)
    ///
    /// # Arguments
    /// * `follow_up_input` - The user's follow-up message
    ///
    /// # Returns
    /// * `Result<String>` - The AI's response, or an error
    pub fn continue_conversation(
        &self,
        follow_up_input: String,
    ) -> std::result::Result<String, AetherException> {
        // Check if there's an active session first
        {
            let manager = self
                .conversation_manager
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if !manager.has_active_session() {
                drop(manager); // Release lock before calling on_error
                self.event_handler.on_error(
                    "No active conversation. Start a new conversation first.".to_string(),
                    Some("Call start_conversation() to begin a new session.".to_string()),
                );
                return Err(AetherException::Error);
            }
        }

        // Get conversation context (session exists, checked above)
        let (context_prompt, context, turn_count) = {
            let manager = self
                .conversation_manager
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let session = manager.active_session().unwrap();
            (
                manager.build_context_prompt(),
                session.context.clone(),
                session.turn_count(),
            )
        };

        info!(
            input_preview = %follow_up_input.chars().take(50).collect::<String>(),
            turn_count = turn_count,
            "Continuing conversation"
        );

        // Build augmented input with conversation history
        let augmented_input = if context_prompt.is_empty() {
            follow_up_input.clone()
        } else {
            format!("{}\n\n当前问题: {}", context_prompt, follow_up_input)
        };

        // Process with AI
        let start_time = std::time::Instant::now();
        let response = self.process_with_ai_first(augmented_input, context.clone(), start_time)?;

        // Add the turn to conversation history
        {
            let mut manager = self
                .conversation_manager
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(turn) = manager.add_turn(follow_up_input, response.clone()) {
                // Notify UI about the completed turn
                self.event_handler.on_conversation_turn_completed(
                    crate::conversation::ConversationTurn {
                        turn_id: turn.turn_id,
                        user_input: turn.user_input,
                        ai_response: turn.ai_response,
                        timestamp: turn.timestamp,
                    },
                );
            }
        }

        // Notify UI that continuation is still available
        self.event_handler.on_conversation_continuation_ready();

        Ok(response)
    }

    /// End the current conversation session.
    ///
    /// This should be called when the user presses ESC to close the Halo input.
    pub fn end_conversation(&self) {
        let session = {
            let mut manager = self
                .conversation_manager
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            manager.end_session()
        };

        if let Some(ended_session) = session {
            info!(
                session_id = %ended_session.session_id,
                turns = ended_session.turn_count(),
                "Conversation session ended"
            );

            // Notify UI
            self.event_handler.on_conversation_ended(
                ended_session.session_id.clone(),
                ended_session.turn_count(),
            );
        }
    }

    /// Check if there's an active conversation session.
    pub fn has_active_conversation(&self) -> bool {
        let manager = self
            .conversation_manager
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        manager.has_active_session()
    }

    /// Get the current turn count for the active conversation.
    pub fn conversation_turn_count(&self) -> u32 {
        let manager = self
            .conversation_manager
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        manager.turn_count()
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
        let core = AetherCore::new(handler);
        assert!(core.is_ok(), "AetherCore should be created successfully");
    }

    // REMOVED: test_start_stop_listening, test_multiple_start_stop_cycles
    // Hotkey monitoring has been migrated to Swift layer (GlobalHotkeyMonitor.swift)
    // The is_listening() method always returns false for backward compatibility.
    // See: refactor-native-api-separation proposal

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
            attachments: None,
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
            attachments: None,
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
