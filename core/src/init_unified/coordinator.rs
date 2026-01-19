//! InitializationCoordinator - unified first-time setup

use super::error::InitError;
use crate::utils::paths::get_config_dir;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

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
    #[allow(dead_code)] // Will be used in Task 2
    config_dir: PathBuf,
    #[allow(dead_code)] // Will be used in Task 2
    handler: Option<Arc<dyn InitProgressHandler>>,
}

impl InitializationCoordinator {
    pub fn new(handler: Option<Arc<dyn InitProgressHandler>>) -> Result<Self, InitError> {
        let config_dir = get_config_dir()
            .map_err(|e| InitError::non_retryable("setup", e.to_string()))?;

        Ok(Self { config_dir, handler })
    }

    /// Run the full initialization sequence
    pub async fn run(&self) -> InitializationResult {
        // Placeholder - will be implemented in Task 2
        info!("InitializationCoordinator::run() called");
        InitializationResult {
            success: false,
            completed_phases: vec![],
            error_phase: Some("not_implemented".to_string()),
            error_message: Some("Not yet implemented".to_string()),
        }
    }
}
