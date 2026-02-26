//! InitializationCoordinator - unified first-time setup

use super::error::InitError;
use crate::config::Config;
use crate::utils::paths::get_config_dir;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};

/// Initialization phase identifier
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitPhase {
    Directories,
    Config,
    Database,
    Runtimes,
    Skills,
}

impl InitPhase {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Directories => "directories",
            Self::Config => "config",
            Self::Database => "database",
            Self::Runtimes => "runtimes",
            Self::Skills => "skills",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Directories => "Creating directories",
            Self::Config => "Generating configuration",
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
    // Phase 3: Initialize database
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
    // Phase 4: Install runtimes (parallel installation)
    // =========================================================================

    async fn install_runtimes(&self) -> Result<(), InitError> {
        use crate::runtimes::ledger::migrate_from_legacy;

        info!("Initializing runtime ledger (zero-install)...");

        let runtimes_dir = crate::utils::paths::get_runtimes_dir()
            .map_err(|e| InitError::new("runtimes", format!("Failed to get runtimes dir: {}", e)))?;

        // Create directory if needed
        if !runtimes_dir.exists() {
            std::fs::create_dir_all(&runtimes_dir)
                .map_err(|e| InitError::new("runtimes", format!("Failed to create runtimes dir: {}", e)))?;
        }

        // Migrate from legacy manifest.json or create fresh ledger
        let _ledger = migrate_from_legacy(&runtimes_dir)
            .map_err(|e| InitError::new("runtimes", format!("Failed to initialize ledger: {}", e)))?;

        info!("Runtime ledger initialized (no downloads, runtimes provisioned on-demand)");
        Ok(())
    }

    // =========================================================================
    // Phase 5: Install skills
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
