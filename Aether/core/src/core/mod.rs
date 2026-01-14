//! AetherCore module - Main entry point for the Aether library
//!
//! This module is split into submodules for better organization:
//! - `types`: Shared type definitions
//! - `memory`: Memory storage and retrieval operations
//! - `config_ops`: Configuration management
//! - `mcp_ops`: MCP capability methods
//! - `skill_ops`: Skill management methods
//! - `search_ops`: Search capability methods
//! - `tools`: Dispatcher and tool registry
//! - `conversation`: Multi-turn conversation management
//! - `processing`: AI processing pipeline

// Submodule declarations
mod config_ops;
mod conversation;
mod mcp_ops;
mod memory;
mod processing;
mod search_ops;
mod skill_ops;
pub mod tool_executor;
mod tools;
pub mod types;
mod vision_ops;

#[cfg(test)]
mod tests;

// Re-export public types
pub use types::{
    AppMemoryInfo, CapturedContext, CompressionStats, MediaAttachment, MemoryEntryFFI,
};

// Private re-exports for internal use
use types::RequestContext;

use crate::config::{Config, ConfigWatcher};
use crate::conversation::ConversationManager;
use crate::dispatcher::{AsyncConfirmationHandler, ToolRegistry};
use tool_executor::UnifiedToolExecutor;
use crate::tools::NativeToolRegistry;
use crate::error::{AetherError, Result};
use crate::event_handler::{InternalEventHandler, ProcessingState};
use crate::mcp::McpClient;
use crate::memory::cleanup::CleanupService;
use crate::memory::compression::{CompressionConfig, ConflictConfig, SchedulerConfig};
use crate::memory::database::VectorDatabase;
use crate::memory::CompressionService;
use crate::router::Router;
use crate::routing::IntentRoutingPipeline;
use crate::search::SearchRegistry;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

/// Main AetherCore struct - the central coordinator for Aether functionality
///
/// AetherCore manages:
/// - AI provider routing and processing
/// - Memory storage and retrieval
/// - MCP tool integration
/// - Search capabilities
/// - Configuration management
/// - Event handling and callbacks to Swift layer
pub struct AetherCore {
    /// Event handler for callbacks to Swift layer
    pub(crate) event_handler: Arc<dyn InternalEventHandler>,
    /// Tokio runtime for async operations
    pub(crate) runtime: Arc<tokio::runtime::Runtime>,
    /// Last request context for retry functionality
    pub(crate) last_request: Arc<Mutex<Option<RequestContext>>>,
    /// Configuration (wrapped in Mutex for thread-safe updates)
    pub(crate) config: Arc<Mutex<Config>>,
    /// Memory database (if enabled)
    pub(crate) memory_db: Option<Arc<VectorDatabase>>,
    /// Current context (app + window) captured from Swift
    pub(crate) current_context: Arc<Mutex<Option<CapturedContext>>>,
    /// Memory cleanup service
    pub(crate) cleanup_service: Option<Arc<CleanupService>>,
    /// Background cleanup task handle
    #[allow(dead_code)]
    pub(crate) cleanup_task_handle: Option<JoinHandle<()>>,
    /// Memory compression service
    pub(crate) compression_service: Option<Arc<CompressionService>>,
    /// Background compression task handle
    #[allow(dead_code)]
    pub(crate) compression_task_handle: Option<JoinHandle<()>>,
    /// Router for AI provider selection (wrapped in RwLock for hot-reload)
    pub(crate) router: Arc<RwLock<Option<Arc<Router>>>>,
    /// Search registry (wrapped in RwLock for hot-reload)
    pub(crate) search_registry: Arc<RwLock<Option<Arc<SearchRegistry>>>>,
    /// MCP client for tool integration
    pub(crate) mcp_client: Option<Arc<McpClient>>,
    /// Configuration watcher for hot-reload
    #[allow(dead_code)]
    pub(crate) config_watcher: Option<Arc<ConfigWatcher>>,
    /// Multi-turn conversation manager
    pub(crate) conversation_manager: Arc<Mutex<ConversationManager>>,
    /// Unified tool registry (Dispatcher Layer)
    pub(crate) tool_registry: Arc<ToolRegistry>,
    /// Native tool registry (AgentTool instances for execution)
    pub(crate) native_tool_registry: Arc<NativeToolRegistry>,
    /// Async confirmation handler
    pub(crate) async_confirmation: Arc<AsyncConfirmationHandler>,
    /// Intent routing pipeline (enhanced routing system)
    pub(crate) intent_pipeline: Option<Arc<IntentRoutingPipeline>>,
    /// Unified tool executor for routing tool calls to correct backend
    pub(crate) unified_executor: Arc<UnifiedToolExecutor>,
}

impl AetherCore {
    // ========================================================================
    // INITIALIZATION
    // ========================================================================

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
    pub fn new(event_handler: Box<dyn InternalEventHandler>) -> Result<Self> {
        // CRITICAL: Initialize logging system FIRST before any log statements
        // This ensures all log messages are captured to file from the start
        crate::init_logging();

        let event_handler: Arc<dyn InternalEventHandler> = Arc::from(event_handler);

        // Initialize tokio runtime with optimized configuration for macOS
        // Use fewer threads to reduce priority inversion risk with UI thread
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2) // Limit to 2 worker threads
            .max_blocking_threads(2) // Limit blocking threads
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

        // Initialize SearchRegistry (if search enabled)
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
        // Note: We pass runtime reference to start background tasks properly
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

                                // Start background cleanup task using the runtime
                                // This fixes the issue where try_current() would fail
                                #[cfg(not(test))]
                                let task_handle = {
                                    info!(
                                        retention_days = cfg.memory.retention_days,
                                        "Starting daily memory cleanup task"
                                    );
                                    Some(cleanup_arc.start_background_task_with_runtime(&runtime))
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

        // Initialize compression service if enabled
        // Pass runtime reference so background tasks can be spawned properly
        let (compression_service, compression_task_handle) =
            Self::init_compression_service(&config, &memory_db, &runtime);

        // Initialize MCP client with system tools
        let mcp_client = Self::init_mcp_client(&config);

        // Initialize config watcher for hot-reload
        let config_watcher = Self::init_config_watcher(
            Arc::clone(&event_handler),
            Arc::clone(&config),
            Arc::clone(&router),
            Arc::clone(&search_registry),
        );

        // Initialize unified tool registry (Dispatcher Layer)
        let tool_registry = Arc::new(ToolRegistry::new());

        // Initialize native tool registry (AgentTool instances for execution)
        let native_tool_registry = Arc::new(NativeToolRegistry::new());

        // Initialize async confirmation handler
        let async_confirmation = Arc::new(AsyncConfirmationHandler::new());

        // Initialize unified tool executor for routing tool calls
        let unified_executor = Arc::new(UnifiedToolExecutor::new(
            Arc::clone(&native_tool_registry),
            mcp_client.clone(),
        ));

        // Initialize intent routing pipeline if enabled
        let intent_pipeline = Self::init_intent_pipeline(&config, &router);

        let core = Self {
            event_handler,
            runtime: Arc::new(runtime),
            last_request: Arc::new(Mutex::new(None)),
            config,
            memory_db,
            current_context: Arc::new(Mutex::new(None)),
            cleanup_service,
            cleanup_task_handle,
            compression_service,
            compression_task_handle,
            router,
            search_registry,
            mcp_client,
            config_watcher,
            conversation_manager: Arc::new(Mutex::new(ConversationManager::new())),
            tool_registry,
            native_tool_registry,
            async_confirmation,
            intent_pipeline,
            unified_executor,
        };

        // Initialize tool registry with builtin tools and configured tools
        // This populates the registry that the UI will query for commands
        core.refresh_tool_registry();

        Ok(core)
    }

    // ========================================================================
    // INITIALIZATION HELPERS
    // ========================================================================

    /// Initialize compression service
    ///
    /// # Arguments
    /// * `config` - Configuration
    /// * `memory_db` - Memory database (if enabled)
    /// * `runtime` - Tokio runtime for spawning background tasks
    fn init_compression_service(
        config: &Arc<Mutex<Config>>,
        memory_db: &Option<Arc<VectorDatabase>>,
        runtime: &tokio::runtime::Runtime,
    ) -> (Option<Arc<CompressionService>>, Option<JoinHandle<()>>) {
        let cfg = config.lock().unwrap_or_else(|e| e.into_inner());

        if !cfg.memory.compression_enabled {
            debug!("Compression disabled in config");
            return (None, None);
        }

        let Some(ref db) = memory_db else {
            debug!("Memory database not available, compression disabled");
            return (None, None);
        };

        // Get default provider for compression
        let provider_result = if let Some(ref default_provider_name) = cfg.general.default_provider
        {
            if let Some(provider_config) = cfg.providers.get(default_provider_name) {
                use crate::providers::create_provider;
                create_provider(default_provider_name, provider_config.clone()).ok()
            } else {
                warn!(
                    "Default provider '{}' not found in config, compression disabled",
                    default_provider_name
                );
                None
            }
        } else {
            warn!("No default provider configured, compression disabled");
            None
        };

        let Some(provider) = provider_result else {
            return (None, None);
        };

        info!(
            provider = cfg.general.default_provider.as_deref().unwrap_or("unknown"),
            "Compression service using AI provider"
        );

        // Get embedding model directory
        let model_dir = match Self::get_embedding_model_dir() {
            Ok(dir) => dir,
            Err(e) => {
                warn!(
                    "Failed to get embedding model directory for compression: {}",
                    e
                );
                return (None, None);
            }
        };

        use crate::memory::EmbeddingModel;
        let embedding_model = match EmbeddingModel::new(model_dir) {
            Ok(m) => Arc::new(m),
            Err(e) => {
                warn!(
                    "Failed to initialize embedding model for compression: {}",
                    e
                );
                return (None, None);
            }
        };

        // Build compression config from memory config
        let idle_timeout = cfg.memory.compression_idle_timeout_seconds;
        let interval = cfg.memory.compression_interval_seconds;
        let compression_config = CompressionConfig {
            batch_size: cfg.memory.compression_batch_size,
            scheduler: SchedulerConfig {
                idle_timeout_seconds: idle_timeout,
                turn_threshold: cfg.memory.compression_turn_threshold,
                background_interval_seconds: interval,
            },
            conflict: ConflictConfig {
                similarity_threshold: cfg.memory.conflict_similarity_threshold,
            },
            background_interval_seconds: interval,
        };

        let service = Arc::new(CompressionService::new(
            Arc::clone(db),
            provider,
            embedding_model,
            compression_config,
        ));

        // Start background compression task using the provided runtime
        // This fixes the issue where try_current() would fail because we're not
        // inside the runtime context yet during initialization
        #[cfg(not(test))]
        let task_handle = {
            info!(
                interval_seconds = interval,
                turn_threshold = cfg.memory.compression_turn_threshold,
                "Starting background compression task"
            );
            Some(service.start_background_task_with_runtime(runtime))
        };

        #[cfg(test)]
        let task_handle = None;

        info!("Compression service initialized successfully");
        (Some(service), task_handle)
    }

    /// Initialize MCP client for external servers only
    ///
    /// Native tools are now handled via AgentTool infrastructure in `tools` module.
    /// This client is only used for external MCP server connections.
    fn init_mcp_client(config: &Arc<Mutex<Config>>) -> Option<Arc<McpClient>> {
        let cfg = config.lock().unwrap_or_else(|e| e.into_inner());
        let unified_tools = cfg.get_effective_tools_config();

        if !unified_tools.enabled {
            debug!("Tools capability disabled in unified config");
            return None;
        }

        // Create MCP client for external servers only
        // Native tools (fs, git, shell, etc.) are handled via AgentTool infrastructure
        let client = McpClient::new();

        // Log which config format is being used
        if cfg.is_using_unified_tools() {
            info!("Using unified tools configuration [unified_tools]");
        } else {
            debug!("Using legacy tools configuration [tools] + [mcp]");
        }

        // Log MCP external servers (will be started on demand)
        for (server_name, server_config) in unified_tools.enabled_mcp_servers() {
            debug!(
                server = %server_name,
                command = %server_config.command,
                "MCP external server configured (will be started on demand)"
            );
        }

        info!(
            mcp_servers = unified_tools.enabled_mcp_servers().len(),
            "MCP client initialized (native tools handled via AgentTool)"
        );

        Some(Arc::new(client))
    }

    /// Initialize intent routing pipeline if enabled
    ///
    /// The pipeline provides enhanced routing with:
    /// - Intent caching for fast path
    /// - Confidence calibration
    /// - Multi-layer routing (L1/L2/L3)
    /// - Intent aggregation and clarification flow
    fn init_intent_pipeline(
        config: &Arc<Mutex<Config>>,
        _router: &Arc<RwLock<Option<Arc<Router>>>>,
    ) -> Option<Arc<IntentRoutingPipeline>> {
        let cfg = config.lock().unwrap_or_else(|e| e.into_inner());

        // Check if pipeline is enabled
        if !cfg.pipeline.enabled {
            debug!("Intent routing pipeline disabled in config");
            return None;
        }

        // Create semantic matcher for pipeline with routing rules loaded
        // Uses smart_matching config from Config to configure the matcher
        use crate::semantic::{MatcherConfig, SemanticMatcher};
        let matcher_config = MatcherConfig {
            enabled: cfg.smart_matching.enabled,
            regex_threshold: cfg.smart_matching.regex_threshold,
            keyword_threshold: cfg.smart_matching.keyword_threshold as f32,
            ai_threshold: cfg.smart_matching.ai_threshold,
            ..MatcherConfig::default()
        };

        // Load routing rules into SemanticMatcher for L1 regex matching
        // This is critical for /youtube, /search, /webfetch commands to work
        let semantic_matcher = match SemanticMatcher::from_rules(&cfg.rules, matcher_config.clone()) {
            Ok(matcher) => {
                info!(
                    rule_count = cfg.rules.len(),
                    "SemanticMatcher initialized with routing rules"
                );
                Arc::new(matcher)
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "Failed to load routing rules into SemanticMatcher, using empty matcher"
                );
                Arc::new(SemanticMatcher::new(matcher_config))
            }
        };

        // Get provider for L3 if enabled
        let provider = if cfg.pipeline.layers.l3_enabled {
            if let Some(ref default_name) = cfg.general.default_provider {
                if let Some(provider_config) = cfg.providers.get(default_name) {
                    use crate::providers::create_provider;
                    create_provider(default_name, provider_config.clone()).ok()
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        // Create pipeline
        let pipeline = if let Some(p) = provider {
            IntentRoutingPipeline::with_provider(
                cfg.pipeline.clone(),
                semantic_matcher,
                p,
            )
        } else {
            IntentRoutingPipeline::new(cfg.pipeline.clone(), semantic_matcher)
        };

        info!(
            enabled = cfg.pipeline.enabled,
            cache_enabled = cfg.pipeline.cache.enabled,
            l3_enabled = cfg.pipeline.layers.l3_enabled,
            "Intent routing pipeline initialized"
        );

        Some(Arc::new(pipeline))
    }

    /// Initialize config watcher for hot-reload
    fn init_config_watcher(
        handler: Arc<dyn InternalEventHandler>,
        config: Arc<Mutex<Config>>,
        router: Arc<RwLock<Option<Arc<Router>>>>,
        search_registry: Arc<RwLock<Option<Arc<SearchRegistry>>>>,
    ) -> Option<Arc<ConfigWatcher>> {
        let watcher = Arc::new(ConfigWatcher::new(move |config_result| {
            match config_result {
                Ok(new_config) => {
                    log::info!("Config file changed, reloading configuration");

                    // Update config
                    if let Ok(mut cfg) = config.lock() {
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
                    if let Ok(mut router_guard) = router.write() {
                        *router_guard = new_router;
                    }

                    // Reinitialize SearchRegistry with new config
                    let new_search_registry = if let Some(ref search_config) = new_config.search {
                        if search_config.enabled {
                            match Self::create_search_registry_from_config(search_config) {
                                Ok(registry) => {
                                    log::info!("SearchRegistry hot-reloaded successfully");
                                    Some(Arc::new(registry))
                                }
                                Err(e) => {
                                    log::warn!(
                                        "Failed to reinitialize SearchRegistry during hot-reload: {}",
                                        e
                                    );
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
                    if let Ok(mut registry_guard) = search_registry.write() {
                        *registry_guard = new_search_registry;
                    }

                    // Notify Swift via callback
                    handler.on_config_changed();

                    // Notify Swift that tool registry needs refresh
                    // This is needed because ConfigWatcher callback doesn't have access
                    // to AetherCore to call refresh_tool_registry() directly
                    handler.on_tools_refresh_needed();
                }
                Err(e) => {
                    log::error!("Failed to reload config: {}", e);
                    let suggestion = e.suggestion().map(|s| s.to_string());
                    handler.on_error(format!("Config reload failed: {}", e), suggestion);
                }
            }
        }));

        // Start watching config file asynchronously
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
    }

    // ========================================================================
    // PATH HELPERS
    // ========================================================================

    /// Get the path for the memory database file
    pub(crate) fn get_memory_db_path() -> Result<PathBuf> {
        let home_dir = std::env::var("HOME")
            .map_err(|_| AetherError::config("Failed to get HOME environment variable"))?;

        let config_dir = PathBuf::from(home_dir).join(".config").join("aether");
        Ok(config_dir.join("memory.db"))
    }

    /// Get embedding model directory
    pub(crate) fn get_embedding_model_dir() -> Result<PathBuf> {
        let home_dir = std::env::var("HOME")
            .map_err(|_| AetherError::config("Failed to get HOME environment variable"))?;

        let model_dir = PathBuf::from(home_dir)
            .join(".config")
            .join("aether")
            .join("models")
            .join("bge-small-zh-v1.5");

        // Create directory if it doesn't exist
        std::fs::create_dir_all(&model_dir)
            .map_err(|e| AetherError::config(format!("Failed to create model directory: {}", e)))?;

        Ok(model_dir)
    }

    // ========================================================================
    // CORE HELPER METHODS
    // ========================================================================

    /// Get router with poison-safe read lock
    #[allow(dead_code)]
    pub(crate) fn get_router(&self) -> Option<Arc<Router>> {
        let guard = self.router.read().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().map(Arc::clone)
    }

    /// Get search registry with poison-safe read lock
    #[allow(dead_code)]
    pub(crate) fn get_search_registry(&self) -> Option<Arc<SearchRegistry>> {
        let guard = self.search_registry.read().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().map(Arc::clone)
    }

    /// Acquires the config mutex lock with poison recovery.
    #[inline(always)]
    pub(crate) fn lock_config(&self) -> std::sync::MutexGuard<'_, Config> {
        self.config.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Ensures the memory database is initialized and returns a reference to it.
    #[inline(always)]
    pub(crate) fn require_memory_db(&self) -> Result<&Arc<VectorDatabase>> {
        self.memory_db
            .as_ref()
            .ok_or_else(|| AetherError::config("Memory database not initialized"))
    }

    // ========================================================================
    // LIFECYCLE METHODS
    // ========================================================================

    /// Start listening for hotkey events (DEPRECATED - now handled by Swift layer)
    pub fn start_listening(&self) -> Result<()> {
        info!(
            "[AetherCore] start_listening() called - hotkey monitoring now handled by Swift layer"
        );
        info!("[AetherCore] See GlobalHotkeyMonitor.swift for implementation details");
        Ok(())
    }

    /// Stop listening for hotkey events (DEPRECATED - now handled by Swift layer)
    pub fn stop_listening(&self) -> Result<()> {
        info!("[AetherCore] stop_listening() called - hotkey monitoring handled by Swift layer");
        Ok(())
    }

    /// Check if hotkey listener is active (DEPRECATED - always returns false)
    pub fn is_listening(&self) -> bool {
        false // Hotkey listening is now in Swift layer
    }

    // ========================================================================
    // LOGGING CONTROL METHODS
    // ========================================================================

    /// Get the current log level
    pub fn get_log_level(&self) -> crate::logging::LogLevel {
        crate::logging::get_log_level()
    }

    /// Set the log level dynamically
    pub fn set_log_level(&self, level: crate::logging::LogLevel) -> Result<()> {
        crate::logging::set_log_level(level);
        Ok(())
    }

    /// Get the log directory path
    pub fn get_log_directory(&self) -> Result<String> {
        let log_dir = crate::logging::get_log_directory()
            .map_err(|e| AetherError::config(format!("Failed to get log directory: {}", e)))?;

        Ok(log_dir.to_string_lossy().to_string())
    }

    // ========================================================================
    // REQUEST CONTEXT / RETRY METHODS
    // ========================================================================

    /// Retry the last failed request
    ///
    /// Implements exponential backoff: 2s, 4s, 8s
    /// Max 2 auto-retries, then manual retry only
    #[deprecated(note = "Not used by Swift layer, may be removed in future")]
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

        // Clone data for async operation
        let _clipboard_content = request_ctx.clipboard_content.clone();
        let _provider = request_ctx.provider.clone();
        let _retry_count = request_ctx.retry_count;

        drop(last_request_lock); // Release lock before sleep

        // Wait with exponential backoff
        thread::sleep(Duration::from_secs(backoff_seconds));

        // Notify state change
        self.event_handler
            .on_state_changed(ProcessingState::Processing);

        // Simulate processing
        thread::sleep(Duration::from_millis(500));

        // Simulate success
        self.event_handler
            .on_state_changed(ProcessingState::Success);

        Ok(())
    }

    /// Store request context for retry
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

    // ========================================================================
    // CONTEXT MANAGEMENT
    // ========================================================================

    /// Set the current context (called from Swift when user triggers action)
    pub fn set_current_context(&self, context: CapturedContext) {
        let mut ctx = self
            .current_context
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *ctx = Some(context);
    }
}
