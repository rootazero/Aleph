//! InitializationCoordinator - unified first-time setup

use super::error::InitError;
use crate::config::Config;
use crate::utils::paths::get_config_dir;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Initialization phase identifier
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitPhase {
    Directories,
    Config,
    EmbeddingModel,
    Database,
    Runtimes,
    Skills,
}

impl InitPhase {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Directories => "directories",
            Self::Config => "config",
            Self::EmbeddingModel => "embedding_model",
            Self::Database => "database",
            Self::Runtimes => "runtimes",
            Self::Skills => "skills",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Directories => "Creating directories",
            Self::Config => "Generating configuration",
            Self::EmbeddingModel => "Downloading embedding model",
            Self::Database => "Initializing database",
            Self::Runtimes => "Installing runtimes",
            Self::Skills => "Installing skills",
        }
    }
}

/// Result of initialization attempt
#[derive(Debug, Clone)]
pub struct InitializationResult {
    pub success: bool,
    pub completed_phases: Vec<String>,
    pub error_phase: Option<String>,
    pub error_message: Option<String>,
}

/// Progress callback trait for UI updates
pub trait InitProgressHandler: Send + Sync {
    fn on_phase_started(&self, phase: String, current: u32, total: u32);
    fn on_phase_progress(&self, phase: String, progress: f64, message: String);
    fn on_phase_completed(&self, phase: String);
    fn on_download_progress(&self, item: String, downloaded: u64, total: u64);
    fn on_error(&self, phase: String, message: String, is_retryable: bool);
}

/// Main initialization coordinator
pub struct InitializationCoordinator {
    config_dir: PathBuf,
    handler: Option<Arc<dyn InitProgressHandler>>,
}

impl InitializationCoordinator {
    pub fn new(handler: Option<Arc<dyn InitProgressHandler>>) -> Result<Self, InitError> {
        let config_dir =
            get_config_dir().map_err(|e| InitError::non_retryable("setup", e.to_string()))?;

        Ok(Self {
            config_dir,
            handler,
        })
    }

    /// Run the full initialization sequence
    pub async fn run(&self) -> InitializationResult {
        let phases = [
            InitPhase::Directories,
            InitPhase::Config,
            InitPhase::EmbeddingModel,
            InitPhase::Database,
            InitPhase::Runtimes,
            InitPhase::Skills,
        ];

        let total = phases.len() as u32;
        let mut completed_phases = Vec::new();

        for (i, phase) in phases.iter().enumerate() {
            let current = (i + 1) as u32;

            // Notify phase start
            if let Some(h) = &self.handler {
                h.on_phase_started(phase.name().to_string(), current, total);
            }

            // Execute phase
            match self.run_phase(phase).await {
                Ok(()) => {
                    completed_phases.push(phase.name().to_string());
                    if let Some(h) = &self.handler {
                        h.on_phase_completed(phase.name().to_string());
                    }
                }
                Err(e) => {
                    warn!(phase = %phase.name(), error = %e, "Phase failed");

                    if let Some(h) = &self.handler {
                        h.on_error(e.phase.clone(), e.message.clone(), e.is_retryable);
                    }

                    // Rollback completed phases
                    if let Err(rollback_err) = self.rollback(&completed_phases).await {
                        warn!(error = %rollback_err, "Rollback failed");
                    }

                    return InitializationResult {
                        success: false,
                        completed_phases,
                        error_phase: Some(e.phase),
                        error_message: Some(e.message),
                    };
                }
            }
        }

        info!("Initialization completed successfully");
        InitializationResult {
            success: true,
            completed_phases,
            error_phase: None,
            error_message: None,
        }
    }

    /// Dispatch to the appropriate phase handler
    async fn run_phase(&self, phase: &InitPhase) -> Result<(), InitError> {
        match phase {
            InitPhase::Directories => self.create_directories().await,
            InitPhase::Config => self.generate_config().await,
            InitPhase::EmbeddingModel => self.download_embedding_model().await,
            InitPhase::Database => self.initialize_database().await,
            InitPhase::Runtimes => self.install_runtimes().await,
            InitPhase::Skills => self.install_skills().await,
        }
    }

    /// Rollback completed phases in reverse order
    async fn rollback(&self, completed_phases: &[String]) -> Result<(), InitError> {
        info!(phases = ?completed_phases, "Rolling back initialization");

        for phase in completed_phases.iter().rev() {
            match phase.as_str() {
                "skills" => {
                    let skills_dir = self.config_dir.join("skills");
                    if skills_dir.exists() {
                        if let Err(e) = tokio::fs::remove_dir_all(&skills_dir).await {
                            warn!(error = %e, dir = ?skills_dir, "Failed to remove skills directory during rollback");
                        }
                    }
                }
                "runtimes" => {
                    let runtimes_dir = self.config_dir.join("runtimes");
                    if runtimes_dir.exists() {
                        if let Err(e) = tokio::fs::remove_dir_all(&runtimes_dir).await {
                            warn!(error = %e, dir = ?runtimes_dir, "Failed to remove runtimes directory during rollback");
                        }
                    }
                }
                "database" => {
                    let db_path = self.config_dir.join("memory.db");
                    if db_path.exists() {
                        if let Err(e) = tokio::fs::remove_file(&db_path).await {
                            warn!(error = %e, path = ?db_path, "Failed to remove database during rollback");
                        }
                    }
                }
                "embedding_model" => {
                    let models_dir = self.config_dir.join("models");
                    if models_dir.exists() {
                        if let Err(e) = tokio::fs::remove_dir_all(&models_dir).await {
                            warn!(error = %e, dir = ?models_dir, "Failed to remove models directory during rollback");
                        }
                    }
                }
                "config" => {
                    let config_path = self.config_dir.join("config.toml");
                    if config_path.exists() {
                        if let Err(e) = tokio::fs::remove_file(&config_path).await {
                            warn!(error = %e, path = ?config_path, "Failed to remove config during rollback");
                        }
                    }
                }
                "directories" => {
                    // Don't remove entire config_dir to preserve user data
                }
                _ => {}
            }
        }

        info!("Rollback completed");
        Ok(())
    }

    // =========================================================================
    // Phase 1: Create directories
    // =========================================================================

    async fn create_directories(&self) -> Result<(), InitError> {
        let dirs = [
            self.config_dir.clone(),
            self.config_dir.join("logs"),
            self.config_dir.join("cache"),
            self.config_dir.join("output"), // Default output directory for generated files
            self.config_dir.join("skills"),
            self.config_dir.join("models"),
            self.config_dir.join("runtimes"),
        ];

        for dir in &dirs {
            tokio::fs::create_dir_all(dir).await.map_err(|e| {
                InitError::new("directories", format!("Failed to create {:?}: {}", dir, e))
            })?;
        }

        info!(dir = ?self.config_dir, "Directory structure created");
        Ok(())
    }

    // =========================================================================
    // Phase 2: Generate config
    // =========================================================================

    async fn generate_config(&self) -> Result<(), InitError> {
        let config_path = self.config_dir.join("config.toml");

        // Don't overwrite existing config
        if config_path.exists() {
            info!("Config already exists, skipping");
            return Ok(());
        }

        let default_config = Config::default();
        let toml_str = toml::to_string_pretty(&default_config)
            .map_err(|e| InitError::new("config", format!("Failed to serialize config: {}", e)))?;

        tokio::fs::write(&config_path, toml_str)
            .await
            .map_err(|e| InitError::new("config", format!("Failed to write config: {}", e)))?;

        info!(path = ?config_path, "Default config created");
        Ok(())
    }

    // =========================================================================
    // Phase 3: Download embedding model
    // =========================================================================

    async fn download_embedding_model(&self) -> Result<(), InitError> {
        use crate::memory::EmbeddingModel as AlephEmbeddingModel;
        use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

        info!("Downloading embedding model bge-small-zh-v1.5...");

        // Report progress
        if let Some(h) = &self.handler {
            h.on_phase_progress(
                "embedding_model".to_string(),
                0.0,
                "Initializing model download...".to_string(),
            );
        }

        // Get our custom cache directory (same as EmbeddingModel uses at runtime)
        // This ensures consistency: ~/.aleph/models/fastembed/
        let cache_dir = AlephEmbeddingModel::get_default_model_path().map_err(|e| {
            InitError::new(
                "embedding_model",
                format!("Failed to get model path: {}", e),
            )
        })?;

        // Ensure cache directory exists
        tokio::fs::create_dir_all(&cache_dir).await.map_err(|e| {
            InitError::new(
                "embedding_model",
                format!("Failed to create model directory: {}", e),
            )
        })?;

        debug!(cache_dir = %cache_dir.display(), "Using cache directory for embedding model");

        // fastembed handles download automatically
        // Model is cached in our custom directory for consistency
        let _model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::BGESmallZHV15)
                .with_cache_dir(cache_dir)
                .with_show_download_progress(true),
        )
        .map_err(|e| {
            InitError::new(
                "embedding_model",
                format!("Failed to download model: {}", e),
            )
        })?;

        info!("Embedding model downloaded successfully");
        Ok(())
    }

    // =========================================================================
    // Phase 4: Initialize database
    // =========================================================================

    async fn initialize_database(&self) -> Result<(), InitError> {
        use crate::memory::store::lance::LanceMemoryBackend;

        let db_path = self.config_dir.clone();

        info!(path = ?db_path, "Initializing memory database (LanceDB)");

        let _db = LanceMemoryBackend::open_or_create(&db_path)
            .await
            .map_err(|e| InitError::new("database", format!("Failed to create database: {}", e)))?;

        info!("Memory database initialized");
        Ok(())
    }

    // =========================================================================
    // Phase 5: Install runtimes (parallel installation)
    // =========================================================================

    async fn install_runtimes(&self) -> Result<(), InitError> {
        use crate::runtimes::RuntimeRegistry;
        use std::time::Duration;

        info!("Installing runtimes in parallel...");

        // Create registry
        let registry = RuntimeRegistry::new()
            .map_err(|e| InitError::new("runtimes", format!("Failed to create registry: {}", e)))?;

        let runtime_ids = ["ffmpeg", "yt-dlp", "uv", "fnm"];

        // Report start
        if let Some(h) = &self.handler {
            h.on_phase_progress(
                "runtimes".to_string(),
                0.0,
                format!("Installing {} runtimes...", runtime_ids.len()),
            );
        }

        // Install all runtimes in parallel
        let mut handles = Vec::new();

        for id in runtime_ids {
            let runtime = registry
                .get(id)
                .ok_or_else(|| InitError::new("runtimes", format!("Unknown runtime: {}", id)))?;

            let handler = self.handler.clone();
            let runtime_id = id.to_string();

            let handle = tokio::spawn(async move {
                // Report individual runtime start
                if let Some(h) = &handler {
                    h.on_phase_progress(
                        "runtimes".to_string(),
                        0.0,
                        format!("Installing {}...", runtime_id),
                    );
                }

                let result = if runtime.is_installed() {
                    debug!(runtime_id = %runtime_id, "Runtime already installed, skipping");
                    Ok(())
                } else {
                    info!(runtime_id = %runtime_id, "Installing runtime...");
                    // 5 minute timeout for each runtime installation
                    match tokio::time::timeout(Duration::from_secs(300), runtime.install()).await {
                        Ok(install_result) => install_result,
                        Err(_) => Err(crate::error::AlephError::runtime(
                            &runtime_id,
                            "Installation timed out after 5 minutes",
                        )),
                    }
                };

                // Report completion
                if let Some(h) = &handler {
                    h.on_phase_progress(
                        "runtimes".to_string(),
                        0.0,
                        format!("{} installed", runtime_id),
                    );
                }

                (runtime_id, result)
            });

            handles.push(handle);
        }

        // Wait for all to complete
        let results = futures::future::join_all(handles).await;

        // Collect failures
        let mut failures = Vec::new();
        for result in results {
            match result {
                Ok((id, Ok(()))) => {
                    info!(runtime_id = %id, "Runtime installed successfully");
                }
                Ok((id, Err(e))) => {
                    warn!(runtime_id = %id, error = %e, "Runtime installation failed");
                    failures.push(format!("{}: {}", id, e));
                }
                Err(e) => {
                    warn!(error = %e, "Runtime task panicked");
                    failures.push(format!("task panic: {}", e));
                }
            }
        }

        if !failures.is_empty() {
            return Err(InitError::new(
                "runtimes",
                format!("Failed to install runtimes: {}", failures.join(", ")),
            ));
        }

        info!("All runtimes installed successfully");
        Ok(())
    }

    // =========================================================================
    // Phase 6: Install skills
    // =========================================================================

    async fn install_skills(&self) -> Result<(), InitError> {
        use crate::skills::SkillsRegistry;

        let skills_dir = self.config_dir.join("skills");

        info!(path = ?skills_dir, "Setting up skills directory");

        // Ensure skills directory exists
        tokio::fs::create_dir_all(&skills_dir).await.map_err(|e| {
            InitError::new(
                "skills",
                format!("Failed to create skills directory: {}", e),
            )
        })?;

        // Note: Built-in skills are copied from app bundle by the platform layer (Swift/C#)
        // The bundle_skills_dir path is not available from Rust core
        // This phase just ensures the directory exists and validates the registry

        // Report progress
        if let Some(h) = &self.handler {
            h.on_phase_progress(
                "skills".to_string(),
                0.5,
                "Validating skills registry...".to_string(),
            );
        }

        // Initialize and validate skills registry
        let registry = SkillsRegistry::new(skills_dir.clone());
        if let Err(e) = registry.load_all() {
            // Non-fatal: just warn if skills can't be loaded
            warn!(error = %e, "Failed to load skills registry");
        }

        info!(
            skill_count = registry.count(),
            "Skills directory initialized"
        );
        Ok(())
    }
}
