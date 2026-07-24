//! `InitializationCoordinator` - unified first-time setup

use super::error::InitError;
use crate::config::Config;
use crate::sync_primitives::Arc;
use crate::utils::paths::{get_config_dir, get_runtimes_dir};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{info, warn};

const CONFIG_SUBDIRS: &[&str] = &["logs", "cache", "output", "skills", "models"];

/// Translate a `tokio::task::JoinError` from a `spawn_blocking` into a
/// human-readable `InitError`, attributing the cause to a named phase.
fn join_error_to_init_error(phase: &str, e: tokio::task::JoinError) -> InitError {
    let label = phase.to_string();
    let msg = if e.is_panic() {
        if let Ok(payload) = e.try_into_panic() {
            if let Some(s) = payload.downcast_ref::<String>() {
                format!("{label} init task panicked: {s}")
            } else if let Some(s) = payload.downcast_ref::<&str>() {
                format!("{label} init task panicked: {s}")
            } else {
                format!("{label} init task panicked (unknown payload)")
            }
        } else {
            format!("{label} init task panicked")
        }
    } else {
        format!("{label} init task cancelled: {e}")
    };
    InitError::new(phase, msg)
}

/// Initialization phase identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InitPhase {
    Directories,
    Config,
    Database,
    Runtimes,
    Skills,
}

impl InitPhase {
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Directories => "directories",
            Self::Config => "config",
            Self::Database => "database",
            Self::Runtimes => "runtimes",
            Self::Skills => "skills",
        }
    }

    #[must_use]
    pub const fn display_name(&self) -> &'static str {
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

/// Filesystem state captured BEFORE any phase runs, so rollback can
/// distinguish artifacts created by this initialization from pre-existing
/// user data (which must never be deleted on rollback).
struct PreExistingState {
    database: bool,
    runtimes_dir: bool,
    skills_dir: bool,
    subdirs: BTreeSet<String>,
}

/// Progress callback trait for UI updates
pub trait InitProgressHandler: Send + Sync {
    fn on_phase_started(&self, phase: String, current: u32, total: u32);
    fn on_phase_progress(&self, phase: String, progress: f64, message: String);
    fn on_phase_completed(&self, phase: String);
    fn on_download_progress(&self, item: String, downloaded: u64, total: u64);
    fn on_error(&self, phase: &str, message: &str, is_retryable: bool);
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
        static INITIALIZING: crate::sync_primitives::AtomicBool =
            crate::sync_primitives::AtomicBool::new(false);
        if INITIALIZING.swap(true, crate::sync_primitives::Ordering::SeqCst) {
            return InitializationResult {
                success: false,
                completed_phases: Vec::new(),
                error_phase: Some("setup".to_string()),
                error_message: Some("Initialization already in progress".to_string()),
            };
        }

        // RAII guard ensures INITIALIZING is reset even if run_internal panics
        struct Guard;
        impl Drop for Guard {
            fn drop(&mut self) {
                INITIALIZING.store(false, crate::sync_primitives::Ordering::SeqCst);
            }
        }
        let _guard = Guard;

        self.run_internal().await
    }

    async fn run_internal(&self) -> InitializationResult {
        let phases = [
            InitPhase::Directories,
            InitPhase::Config,
            InitPhase::Database,
            InitPhase::Runtimes,
            InitPhase::Skills,
        ];

        let total = phases.len() as u32;
        let mut completed_phases: Vec<InitPhase> = Vec::new();

        // Snapshot which artifacts already exist: initialization can be
        // triggered by a single missing marker (e.g. config.toml) on an
        // otherwise populated install, and rollback must not delete
        // pre-existing user data.
        let mut pre_existing_subdirs = BTreeSet::new();
        for subdir in CONFIG_SUBDIRS {
            if fs::try_exists(self.config_dir.join(subdir))
                .await
                .unwrap_or(false)
            {
                pre_existing_subdirs.insert(subdir.to_string());
            }
        }
        let pre_existing = PreExistingState {
            database: fs::try_exists(self.config_dir.join("memory.db"))
                .await
                .unwrap_or(false),
            runtimes_dir: match get_runtimes_dir() {
                Ok(d) => fs::try_exists(&d).await.unwrap_or(false),
                Err(_) => false,
            },
            skills_dir: fs::try_exists(self.config_dir.join("skills"))
                .await
                .unwrap_or(false),
            subdirs: pre_existing_subdirs,
        };

        for (i, phase) in phases.iter().enumerate() {
            let current = (i + 1) as u32;

            // Notify phase start
            if let Some(h) = &self.handler {
                h.on_phase_started(phase.name().to_string(), current, total);
            }

            // Execute phase
            match self.run_phase(phase).await {
                Ok(()) => {
                    completed_phases.push(*phase);
                    if let Some(h) = &self.handler {
                        h.on_phase_completed(phase.name().to_string());
                    }
                }
                Err(e) => {
                    warn!(phase = %phase.name(), error = %e, "Phase failed");

                    if let Some(h) = &self.handler {
                        h.on_error(&e.phase, &e.message, e.is_retryable);
                    }

                    // Rollback completed phases
                    let error_message = match self.rollback(&completed_phases, &pre_existing).await
                    {
                        Ok(()) => e.message,
                        Err(rollback_err) => {
                            warn!(error = %rollback_err, "Rollback failed");
                            format!("{} (rollback also failed: {})", e.message, rollback_err)
                        }
                    };

                    return InitializationResult {
                        success: false,
                        completed_phases: completed_phases
                            .iter()
                            .map(|p| p.name().to_string())
                            .collect(),
                        error_phase: Some(e.phase),
                        error_message: Some(error_message),
                    };
                }
            }
        }

        info!("Initialization completed successfully");
        InitializationResult {
            success: true,
            completed_phases: completed_phases
                .iter()
                .map(|p| p.name().to_string())
                .collect(),
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
    ///
    /// `pre_existing` records which artifacts existed before this run started;
    /// those are user data and are skipped (never deleted) here.
    async fn rollback(
        &self,
        completed_phases: &[InitPhase],
        pre_existing: &PreExistingState,
    ) -> Result<(), InitError> {
        info!(phases = ?completed_phases, "Rolling back initialization");

        let mut errors: Vec<String> = Vec::new();

        for phase in completed_phases.iter().rev() {
            match phase {
                InitPhase::Skills => self.rollback_skills(pre_existing, &mut errors).await,
                InitPhase::Runtimes => self.rollback_runtimes(pre_existing, &mut errors).await,
                InitPhase::Database => self.rollback_database(pre_existing, &mut errors).await,
                InitPhase::Config => {
                    warn!("Skipping config rollback to avoid deleting pre-existing user configuration");
                }
                InitPhase::Directories => {
                    self.rollback_directories(pre_existing, &mut errors).await
                }
            }
        }

        if errors.is_empty() {
            info!("Rollback completed");
            Ok(())
        } else {
            Err(InitError::non_retryable(
                "rollback",
                format!("Partial rollback failed: {}", errors.join("; ")),
            ))
        }
    }

    async fn rollback_skills(&self, pre_existing: &PreExistingState, errors: &mut Vec<String>) {
        let skills_dir = self.config_dir.join("skills");
        if pre_existing.skills_dir {
            warn!(dir = ?skills_dir, "Skills directory pre-existed; skipping rollback to preserve user skills");
        } else if fs::try_exists(&skills_dir).await.unwrap_or(false) {
            if let Err(e) = fs::remove_dir_all(&skills_dir).await {
                warn!(error = %e, dir = ?skills_dir, "Failed to remove skills directory during rollback");
                errors.push(format!("skills dir: {e}"));
            }
        }
    }

    async fn rollback_runtimes(&self, pre_existing: &PreExistingState, errors: &mut Vec<String>) {
        match get_runtimes_dir() {
            Ok(runtimes_dir) => {
                if pre_existing.runtimes_dir {
                    warn!(dir = ?runtimes_dir, "Runtimes directory pre-existed; skipping rollback to preserve installed runtimes");
                } else if fs::try_exists(&runtimes_dir).await.unwrap_or(false) {
                    if let Err(e) = fs::remove_dir_all(&runtimes_dir).await {
                        warn!(error = %e, dir = ?runtimes_dir, "Failed to remove runtimes directory during rollback");
                        errors.push(format!("runtimes dir: {e}"));
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "Failed to get runtimes dir during rollback");
                errors.push(format!("runtimes dir: {e}"));
            }
        }
    }

    async fn rollback_database(&self, pre_existing: &PreExistingState, errors: &mut Vec<String>) {
        // Don't delete memory.db (may pre-exist), but clean up WAL files
        // that were created during this initialization. If the database
        // pre-existed, its WAL may hold committed-but-uncheckpointed
        // transactions (or belong to a live connection) — leave it alone.
        if pre_existing.database {
            warn!("memory.db pre-existed; skipping WAL cleanup during rollback");
            return;
        }
        for suffix in ["-wal", "-shm"] {
            let wal_path = self.config_dir.join(format!("memory.db{suffix}"));
            if fs::try_exists(&wal_path).await.unwrap_or(false) {
                if let Err(e) = fs::remove_file(&wal_path).await {
                    warn!(error = %e, path = ?wal_path, "Failed to remove WAL file during rollback");
                    errors.push(format!("wal {suffix}: {e}"));
                }
            }
        }
    }

    async fn rollback_directories(
        &self,
        pre_existing: &PreExistingState,
        errors: &mut Vec<String>,
    ) {
        for subdir in CONFIG_SUBDIRS {
            let path = self.config_dir.join(subdir);
            if pre_existing.subdirs.contains(*subdir) {
                continue;
            }
            if fs::try_exists(&path).await.unwrap_or(false) {
                match fs::read_dir(&path).await {
                    Ok(mut entries) => match entries.next_entry().await {
                        Ok(None) => {
                            if let Err(e) = fs::remove_dir(&path).await {
                                warn!(error = %e, dir = ?path, "Failed to remove empty directory during rollback");
                                errors.push(format!("dir {subdir}: {e}"));
                            }
                        }
                        Ok(Some(_)) => {
                            warn!(dir = ?path, "Directory not empty, skipping rollback");
                        }
                        Err(e) => {
                            warn!(error = %e, dir = ?path, "Failed to read directory during rollback");
                            errors.push(format!("dir {subdir}: {e}"));
                        }
                    },
                    Err(e) => {
                        warn!(error = %e, dir = ?path, "Failed to open directory during rollback");
                        errors.push(format!("dir {subdir}: {e}"));
                    }
                }
            }
        }
    }

    // =========================================================================
    // Phase 1: Create directories
    // =========================================================================

    async fn create_directories(&self) -> Result<(), InitError> {
        // Create root config dir (no clone needed — use the reference)
        Self::ensure_dir(self.config_dir.as_path()).await?;

        let subdirs: [PathBuf; 5] = [
            self.config_dir.join("logs"),
            self.config_dir.join("cache"),
            self.config_dir.join("output"),
            self.config_dir.join("skills"),
            self.config_dir.join("models"),
        ];

        for dir in &subdirs {
            Self::ensure_dir(dir).await?;
        }

        info!(dir = ?self.config_dir, "Directory structure created");
        Ok(())
    }

    async fn ensure_dir(path: &Path) -> Result<(), InitError> {
        tokio::fs::create_dir_all(path).await.map_err(|e| {
            InitError::new("directories", format!("Failed to create {path:?}: {e}"))
        })?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = tokio::fs::metadata(path).await.map_err(|e| {
                InitError::new(
                    "directories",
                    format!("Failed to get metadata for {path:?}: {e}"),
                )
            })?;
            let mut perms = metadata.permissions();
            perms.set_mode(0o700);
            tokio::fs::set_permissions(path, perms).await.map_err(|e| {
                InitError::new(
                    "directories",
                    format!("Failed to set permissions for {path:?}: {e}"),
                )
            })?;
        }

        Ok(())
    }

    // =========================================================================
    // Phase 2: Generate config
    // =========================================================================

    async fn generate_config(&self) -> Result<(), InitError> {
        let config_path = self.config_dir.join("config.toml");

        // Don't overwrite existing config
        if fs::try_exists(&config_path).await.unwrap_or(false) {
            info!("Config already exists, skipping");
            return Ok(());
        }

        let default_config = Config::default();
        let toml_str = toml::to_string_pretty(&default_config)
            .map_err(|e| InitError::new("config", format!("Failed to serialize config: {e}")))?;

        // Use process-specific temp filename to avoid collisions with concurrent
        // or crashed initialization attempts.
        let temp_path = config_path.with_extension(format!("tmp.{}", std::process::id()));
        tokio::fs::write(&temp_path, toml_str).await.map_err(|e| {
            InitError::new("config", format!("Failed to write temporary config: {e}"))
        })?;

        if let Err(e) = tokio::fs::rename(&temp_path, &config_path).await {
            // Clean up temp file on rename failure to avoid leaving stale artifacts
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(InitError::new(
                "config",
                format!("Failed to finalize config: {e}"),
            ));
        }

        info!(path = ?config_path, "Default config created");
        Ok(())
    }

    // =========================================================================
    // Phase 3: Initialize database
    // =========================================================================

    async fn initialize_database(&self) -> Result<(), InitError> {
        use crate::memory::store::sqlite::SqliteMemoryBackend;

        let db_path = self.config_dir.join("memory.db");

        info!(path = ?db_path, "Initializing memory database (SQLite + sqlite-vec)");

        // Database creation performs synchronous file I/O and schema init;
        // run it on the blocking thread pool.
        tokio::task::spawn_blocking(move || {
            SqliteMemoryBackend::new(&db_path)
                .map(|_| ())
                .map_err(|e| format!("{e}"))
        })
        .await
        .map_err(|e| join_error_to_init_error("database", e))?
        .map_err(|e| InitError::new("database", format!("Failed to create database: {e}")))?;

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
            .map_err(|e| InitError::new("runtimes", format!("Failed to get runtimes dir: {e}")))?;

        // Create directory if needed
        tokio::fs::create_dir_all(&runtimes_dir)
            .await
            .map_err(|e| {
                InitError::new("runtimes", format!("Failed to create runtimes dir: {e}"))
            })?;

        // Migrate from legacy manifest.json or create fresh ledger
        // Run in spawn_blocking since migrate_from_legacy does sync file I/O.
        // Persist the ledger so ledger.json exists on disk: the fresh-install
        // path of migrate_from_legacy returns an in-memory-only ledger, and
        // first-run detection (needs_initialization) requires the file to be
        // present for the runtimes phase to count as completed.
        tokio::task::spawn_blocking(move || {
            migrate_from_legacy(&runtimes_dir).and_then(|l| l.persist())
        })
            .await
            .map_err(|e| join_error_to_init_error("runtimes", e))?
            .map_err(|e| InitError::new("runtimes", format!("Failed to initialize ledger: {e}")))?;

        info!("Runtime ledger initialized (no downloads, runtimes provisioned on-demand)");
        Ok(())
    }

    // =========================================================================
    // Phase 5: Install skills
    // =========================================================================

    async fn install_skills(&self) -> Result<(), InitError> {
        use crate::skill::shared_skill_system;

        let skills_dir = self.config_dir.join("skills");

        info!(path = ?skills_dir, "Setting up skills directory");

        // Note: Built-in skills are copied from app bundle by the platform layer (Swift/C#)
        // The bundle_skills_dir path is not available from Rust core
        // Directory was created in phase 1; this phase validates the skills system

        let system = shared_skill_system();
        system.init(vec![skills_dir]).await.map_err(|e| {
            InitError::new("skills", format!("Failed to initialize skill system: {e}"))
        })?;

        let skill_count = system.list_skills().await.len();
        info!(skill_count = skill_count, "Skills directory initialized");
        Ok(())
    }
}
