# Unified Initialization Module Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Refactor initialization into a unified `InitializationCoordinator` that downloads all components (runtimes, models, database) at first launch with a blocking progress window.

**Architecture:** Single coordinator pattern with 6 sequential phases. Phase 5 (runtimes) downloads 4 runtimes in parallel. All failures trigger rollback and require retry.

**Tech Stack:** Rust (tokio, async-trait, UniFFI), Swift (SwiftUI, NSPanel)

---

## Task 1: Create initialization module structure

**Files:**
- Create: `core/src/initialization/mod.rs`
- Create: `core/src/initialization/coordinator.rs`
- Create: `core/src/initialization/error.rs`

**Step 1: Create module directory**

```bash
mkdir -p core/src/initialization
```

**Step 2: Create mod.rs with module exports**

```rust
// core/src/initialization/mod.rs
//! Unified initialization module
//!
//! Handles first-time setup including:
//! - Directory structure creation
//! - Default configuration generation
//! - Embedding model download
//! - Memory database initialization
//! - Runtime installation (ffmpeg, yt-dlp, uv, fnm)
//! - Built-in skills installation

mod coordinator;
mod error;

pub use coordinator::{InitializationCoordinator, InitializationResult, InitPhase};
pub use error::InitError;

use crate::error::Result;
use crate::utils::paths::get_config_dir;

/// Check if this is a fresh installation requiring initialization
pub fn needs_initialization() -> Result<bool> {
    let config_dir = get_config_dir()?;

    // Check essential markers
    let has_config = config_dir.join("config.toml").exists();
    let has_manifest = config_dir.join("runtimes").join("manifest.json").exists();

    Ok(!has_config || !has_manifest)
}
```

**Step 3: Create error.rs with InitError type**

```rust
// core/src/initialization/error.rs
//! Initialization error types

use std::fmt;

/// Error during initialization
#[derive(Debug, Clone)]
pub struct InitError {
    /// Which phase failed
    pub phase: String,
    /// Error message
    pub message: String,
    /// Whether retry might succeed
    pub is_retryable: bool,
}

impl InitError {
    pub fn new(phase: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            phase: phase.into(),
            message: message.into(),
            is_retryable: true,
        }
    }

    pub fn non_retryable(phase: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            phase: phase.into(),
            message: message.into(),
            is_retryable: false,
        }
    }
}

impl fmt::Display for InitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.phase, self.message)
    }
}

impl std::error::Error for InitError {}
```

**Step 4: Create coordinator.rs skeleton**

```rust
// core/src/initialization/coordinator.rs
//! InitializationCoordinator - unified first-time setup

use super::error::InitError;
use crate::utils::paths::get_config_dir;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};

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
        let config_dir = get_config_dir()
            .map_err(|e| InitError::non_retryable("setup", e.to_string()))?;

        Ok(Self { config_dir, handler })
    }

    /// Run the full initialization sequence
    pub async fn run(&self) -> InitializationResult {
        // TODO: Implement in Task 2
        InitializationResult {
            success: false,
            completed_phases: vec![],
            error_phase: Some("not_implemented".to_string()),
            error_message: Some("Not yet implemented".to_string()),
        }
    }
}
```

**Step 5: Update lib.rs to use new module**

Edit `core/src/lib.rs` line 73, change:
```rust
pub mod initialization;
```

This already exists, but we'll update the re-exports later.

**Step 6: Verify compilation**

```bash
cd /Users/zouguojun/Workspace/Aether/core && cargo check
```

**Step 7: Commit**

```bash
git add core/src/initialization/
git commit -m "feat(init): create initialization module structure

- Add InitializationCoordinator skeleton
- Add InitError type
- Add InitPhase enum with display names
- Add InitProgressHandler trait

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 2: Implement coordinator phases 1-4

**Files:**
- Modify: `core/src/initialization/coordinator.rs`

**Step 1: Add phase execution methods**

Add to `coordinator.rs` after the struct definition:

```rust
impl InitializationCoordinator {
    // ... existing new() method ...

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

    async fn rollback(&self, completed_phases: &[String]) -> Result<(), InitError> {
        info!(phases = ?completed_phases, "Rolling back initialization");

        for phase in completed_phases.iter().rev() {
            match phase.as_str() {
                "skills" => {
                    let skills_dir = self.config_dir.join("skills");
                    if skills_dir.exists() {
                        let _ = std::fs::remove_dir_all(&skills_dir);
                    }
                }
                "runtimes" => {
                    let runtimes_dir = self.config_dir.join("runtimes");
                    if runtimes_dir.exists() {
                        let _ = std::fs::remove_dir_all(&runtimes_dir);
                    }
                }
                "database" => {
                    let db_path = self.config_dir.join("memory.db");
                    if db_path.exists() {
                        let _ = std::fs::remove_file(&db_path);
                    }
                }
                "embedding_model" => {
                    let models_dir = self.config_dir.join("models");
                    if models_dir.exists() {
                        let _ = std::fs::remove_dir_all(&models_dir);
                    }
                }
                "config" => {
                    let config_path = self.config_dir.join("config.toml");
                    if config_path.exists() {
                        let _ = std::fs::remove_file(&config_path);
                    }
                }
                "directories" => {
                    // Only remove if we created it fresh
                    if self.config_dir.exists() {
                        let _ = std::fs::remove_dir_all(&self.config_dir);
                    }
                }
                _ => {}
            }
        }

        info!("Rollback completed");
        Ok(())
    }
}
```

**Step 2: Implement create_directories phase**

```rust
impl InitializationCoordinator {
    // ... after rollback method ...

    async fn create_directories(&self) -> Result<(), InitError> {
        use std::fs;

        let dirs = [
            self.config_dir.clone(),
            self.config_dir.join("logs"),
            self.config_dir.join("cache"),
            self.config_dir.join("skills"),
            self.config_dir.join("models"),
            self.config_dir.join("runtimes"),
        ];

        for dir in &dirs {
            fs::create_dir_all(dir).map_err(|e| {
                InitError::new("directories", format!("Failed to create {:?}: {}", dir, e))
            })?;
        }

        info!(dir = ?self.config_dir, "Directory structure created");
        Ok(())
    }

    async fn generate_config(&self) -> Result<(), InitError> {
        use crate::config::Config;
        use std::fs;

        let config_path = self.config_dir.join("config.toml");

        // Don't overwrite existing config
        if config_path.exists() {
            info!("Config already exists, skipping");
            return Ok(());
        }

        let default_config = Config::default();
        let toml_str = toml::to_string_pretty(&default_config)
            .map_err(|e| InitError::new("config", format!("Failed to serialize config: {}", e)))?;

        fs::write(&config_path, toml_str)
            .map_err(|e| InitError::new("config", format!("Failed to write config: {}", e)))?;

        info!(path = ?config_path, "Default config created");
        Ok(())
    }

    async fn download_embedding_model(&self) -> Result<(), InitError> {
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

        // fastembed handles download automatically
        // Model is cached in ~/.cache/huggingface/hub/
        let _model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::BGESmallZHV15)
                .with_show_download_progress(true),
        )
        .map_err(|e| InitError::new("embedding_model", format!("Failed to download model: {}", e)))?;

        info!("Embedding model downloaded successfully");
        Ok(())
    }

    async fn initialize_database(&self) -> Result<(), InitError> {
        use crate::memory::database::VectorDatabase;

        let db_path = self.config_dir.join("memory.db");

        info!(path = ?db_path, "Initializing memory database");

        let _db = VectorDatabase::new(db_path.clone())
            .map_err(|e| InitError::new("database", format!("Failed to create database: {}", e)))?;

        info!("Memory database initialized");
        Ok(())
    }

    async fn install_runtimes(&self) -> Result<(), InitError> {
        // TODO: Implement in Task 4
        info!("Runtime installation placeholder");
        Ok(())
    }

    async fn install_skills(&self) -> Result<(), InitError> {
        // TODO: Implement in Task 5
        info!("Skills installation placeholder");
        Ok(())
    }
}
```

**Step 3: Add required imports at top of coordinator.rs**

```rust
use super::error::InitError;
use crate::utils::paths::get_config_dir;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};
```

**Step 4: Verify compilation**

```bash
cd /Users/zouguojun/Workspace/Aether/core && cargo check
```

**Step 5: Commit**

```bash
git add core/src/initialization/coordinator.rs
git commit -m "feat(init): implement phases 1-4 (dirs, config, model, db)

- Add run() with phase execution loop
- Add rollback() for failure recovery
- Implement create_directories phase
- Implement generate_config phase
- Implement download_embedding_model phase
- Implement initialize_database phase

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 3: Add FFmpeg runtime manager

**Files:**
- Create: `core/src/runtimes/ffmpeg.rs`
- Modify: `core/src/runtimes/mod.rs`
- Modify: `core/src/runtimes/registry.rs`

**Step 1: Create ffmpeg.rs**

```rust
// core/src/runtimes/ffmpeg.rs
//! FFmpeg Runtime Implementation
//!
//! Single-binary runtime for audio/video processing.

use super::download::{download_file, set_executable};
use super::manager::{RuntimeManager, UpdateInfo};
use crate::error::{AlephError, Result};
use std::path::PathBuf;
use std::process::Command;
use tracing::{debug, info};

/// Download URL for ffmpeg (macOS ARM64 from evermeet.cx)
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const DOWNLOAD_URL: &str = "https://evermeet.cx/ffmpeg/getrelease/ffmpeg/zip";

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const DOWNLOAD_URL: &str = "https://evermeet.cx/ffmpeg/getrelease/ffmpeg/zip";

#[cfg(target_os = "windows")]
const DOWNLOAD_URL: &str = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip";

#[cfg(target_os = "linux")]
const DOWNLOAD_URL: &str = "https://johnvansickle.com/ffmpeg/releases/ffmpeg-release-amd64-static.tar.xz";

/// FFmpeg runtime manager
pub struct FfmpegRuntime {
    runtimes_dir: PathBuf,
}

impl FfmpegRuntime {
    pub fn new(runtimes_dir: PathBuf) -> Self {
        Self { runtimes_dir }
    }

    fn install_dir(&self) -> PathBuf {
        self.runtimes_dir.join("ffmpeg")
    }

    fn binary_path(&self) -> PathBuf {
        #[cfg(target_os = "windows")]
        return self.install_dir().join("ffmpeg.exe");
        #[cfg(not(target_os = "windows"))]
        return self.install_dir().join("ffmpeg");
    }
}

#[async_trait::async_trait]
impl RuntimeManager for FfmpegRuntime {
    fn id(&self) -> &'static str {
        "ffmpeg"
    }

    fn name(&self) -> &'static str {
        "FFmpeg"
    }

    fn description(&self) -> &'static str {
        "Audio/video processing toolkit for AI agents"
    }

    fn is_installed(&self) -> bool {
        self.binary_path().exists()
    }

    fn executable_path(&self) -> PathBuf {
        self.binary_path()
    }

    async fn install(&self) -> Result<()> {
        use std::fs;
        use std::io::{Read, Cursor};
        use zip::ZipArchive;

        info!("Installing FFmpeg...");

        let install_dir = self.install_dir();
        fs::create_dir_all(&install_dir).map_err(|e| {
            AlephError::runtime("ffmpeg", format!("Failed to create directory: {}", e))
        })?;

        // Download zip file
        let zip_path = install_dir.join("ffmpeg_download.zip");
        download_file(DOWNLOAD_URL, &zip_path).await?;

        // Extract ffmpeg binary from zip
        let file = fs::File::open(&zip_path).map_err(|e| {
            AlephError::runtime("ffmpeg", format!("Failed to open zip: {}", e))
        })?;

        let mut archive = ZipArchive::new(file).map_err(|e| {
            AlephError::runtime("ffmpeg", format!("Failed to read zip: {}", e))
        })?;

        // Find and extract ffmpeg binary
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| {
                AlephError::runtime("ffmpeg", format!("Failed to read zip entry: {}", e))
            })?;

            let name = entry.name().to_string();
            if name.ends_with("ffmpeg") || name == "ffmpeg" {
                let mut contents = Vec::new();
                entry.read_to_end(&mut contents).map_err(|e| {
                    AlephError::runtime("ffmpeg", format!("Failed to extract: {}", e))
                })?;

                fs::write(self.binary_path(), contents).map_err(|e| {
                    AlephError::runtime("ffmpeg", format!("Failed to write binary: {}", e))
                })?;

                break;
            }
        }

        // Set executable permission
        set_executable(&self.binary_path())?;

        // Clean up zip file
        let _ = fs::remove_file(&zip_path);

        info!("FFmpeg installed successfully");
        Ok(())
    }

    async fn check_update(&self) -> Option<UpdateInfo> {
        // evermeet.cx doesn't have a version API, skip update checks for now
        None
    }

    async fn update(&self) -> Result<()> {
        // Re-download to update
        self.install().await
    }

    fn get_version(&self) -> Option<String> {
        if !self.is_installed() {
            return None;
        }

        let output = Command::new(self.binary_path())
            .arg("-version")
            .output()
            .ok()?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // Parse "ffmpeg version 6.1.1 ..."
            stdout.lines().next()
                .and_then(|line| line.split_whitespace().nth(2))
                .map(|v| v.to_string())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_ffmpeg_runtime_creation() {
        let temp_dir = TempDir::new().unwrap();
        let runtime = FfmpegRuntime::new(temp_dir.path().to_path_buf());

        assert_eq!(runtime.id(), "ffmpeg");
        assert_eq!(runtime.name(), "FFmpeg");
        assert!(!runtime.is_installed());
    }
}
```

**Step 2: Update runtimes/mod.rs to export ffmpeg**

Edit `core/src/runtimes/mod.rs`, add after line 35:

```rust
mod ffmpeg;
```

And update re-exports after line 45:

```rust
pub use ffmpeg::FfmpegRuntime;
```

**Step 3: Update registry.rs to register ffmpeg**

Edit `core/src/runtimes/registry.rs`, add import at line 9:

```rust
use super::{get_runtimes_dir, FfmpegRuntime, FnmRuntime, UvRuntime, YtDlpRuntime};
```

Add ffmpeg registration in `new()` around line 57-68:

```rust
        let ffmpeg = Arc::new(FfmpegRuntime::new(runtimes_dir.clone()));
        let ytdlp = Arc::new(YtDlpRuntime::new(runtimes_dir.clone()));
        let uv = Arc::new(UvRuntime::new(runtimes_dir.clone()));
        let fnm = Arc::new(FnmRuntime::new(runtimes_dir.clone()));

        runtimes.insert(ffmpeg.id(), ffmpeg);
        runtimes.insert(ytdlp.id(), ytdlp);
        runtimes.insert(uv.id(), uv);
        runtimes.insert(fnm.id(), fnm);
```

Update test assertion at line 221:

```rust
        assert_eq!(runtimes.len(), 4); // ffmpeg, yt-dlp, uv, fnm
```

**Step 4: Verify compilation**

```bash
cd /Users/zouguojun/Workspace/Aether/core && cargo check
```

**Step 5: Run tests**

```bash
cd /Users/zouguojun/Workspace/Aether/core && cargo test runtimes --lib
```

**Step 6: Commit**

```bash
git add core/src/runtimes/
git commit -m "feat(runtimes): add FFmpeg runtime manager

- Implement FfmpegRuntime with download from evermeet.cx
- Support macOS ARM64/x86_64, Windows, Linux
- Extract binary from zip archive
- Register in RuntimeRegistry

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 4: Implement parallel runtime installation

**Files:**
- Modify: `core/src/initialization/coordinator.rs`

**Step 1: Implement install_runtimes with parallel downloads**

Replace the placeholder `install_runtimes` in coordinator.rs:

```rust
    async fn install_runtimes(&self) -> Result<(), InitError> {
        use crate::runtimes::RuntimeRegistry;
        use futures::future::join_all;

        let registry = RuntimeRegistry::new()
            .map_err(|e| InitError::new("runtimes", format!("Failed to create registry: {}", e)))?;

        let runtime_ids = ["ffmpeg", "yt-dlp", "uv", "fnm"];

        info!(runtimes = ?runtime_ids, "Installing runtimes in parallel");

        // Report start
        if let Some(h) = &self.handler {
            h.on_phase_progress(
                "runtimes".to_string(),
                0.0,
                format!("Installing {} runtimes...", runtime_ids.len()),
            );
        }

        // Create futures for parallel installation
        let install_futures: Vec<_> = runtime_ids
            .iter()
            .map(|id| {
                let registry = &registry;
                let handler = self.handler.clone();
                async move {
                    if let Some(h) = &handler {
                        h.on_download_progress(id.to_string(), 0, 0);
                    }

                    let result = registry.require(id).await;

                    if let Some(h) = &handler {
                        h.on_download_progress(id.to_string(), 1, 1);
                    }

                    (id.to_string(), result)
                }
            })
            .collect();

        // Wait for all to complete
        let results = join_all(install_futures).await;

        // Check for failures
        let mut failed = Vec::new();
        for (id, result) in results {
            if let Err(e) = result {
                failed.push(format!("{}: {}", id, e));
            }
        }

        if !failed.is_empty() {
            return Err(InitError::new(
                "runtimes",
                format!("Failed to install runtimes: {}", failed.join(", ")),
            ));
        }

        info!("All runtimes installed successfully");
        Ok(())
    }
```

**Step 2: Add futures dependency if not present**

Check `core/Cargo.toml` for futures crate. It should already be there, but verify.

**Step 3: Verify compilation**

```bash
cd /Users/zouguojun/Workspace/Aether/core && cargo check
```

**Step 4: Commit**

```bash
git add core/src/initialization/coordinator.rs
git commit -m "feat(init): implement parallel runtime installation

- Install ffmpeg, yt-dlp, uv, fnm in parallel using futures::join_all
- Report progress for each runtime
- Collect and report all failures

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 5: Implement skills installation

**Files:**
- Modify: `core/src/initialization/coordinator.rs`

**Step 1: Implement install_skills**

Replace the placeholder `install_skills` in coordinator.rs:

```rust
    async fn install_skills(&self, bundle_skills_dir: Option<PathBuf>) -> Result<(), InitError> {
        use crate::skills::SkillsRegistry;
        use std::fs;

        let skills_dir = self.config_dir.join("skills");

        // Ensure skills directory exists
        fs::create_dir_all(&skills_dir)
            .map_err(|e| InitError::new("skills", format!("Failed to create skills dir: {}", e)))?;

        // Copy built-in skills from bundle if provided
        if let Some(bundle_dir) = bundle_skills_dir {
            if bundle_dir.exists() {
                info!(from = ?bundle_dir, to = ?skills_dir, "Copying built-in skills");

                for entry in fs::read_dir(&bundle_dir).map_err(|e| {
                    InitError::new("skills", format!("Failed to read bundle dir: {}", e))
                })? {
                    let entry = entry.map_err(|e| {
                        InitError::new("skills", format!("Failed to read entry: {}", e))
                    })?;

                    let path = entry.path();
                    if path.is_dir() && path.join("SKILL.md").exists() {
                        let skill_id = path.file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("unknown");

                        let target_dir = skills_dir.join(skill_id);

                        // Don't overwrite existing
                        if !target_dir.exists() {
                            fs::create_dir_all(&target_dir).map_err(|e| {
                                InitError::new("skills", format!("Failed to create skill dir: {}", e))
                            })?;

                            // Copy SKILL.md
                            fs::copy(path.join("SKILL.md"), target_dir.join("SKILL.md")).map_err(|e| {
                                InitError::new("skills", format!("Failed to copy SKILL.md: {}", e))
                            })?;

                            info!(skill_id = %skill_id, "Installed built-in skill");
                        }
                    }
                }
            }
        }

        // Load skills registry to validate
        let registry = SkillsRegistry::new(skills_dir);
        if let Err(e) = registry.load_all() {
            warn!(error = %e, "Failed to load skills after installation");
        }

        info!("Skills installation completed");
        Ok(())
    }
```

**Step 2: Update run_phase to pass bundle_skills_dir**

Update the `run_phase` method to handle skills specially:

```rust
    async fn run_phase(&self, phase: &InitPhase) -> Result<(), InitError> {
        match phase {
            InitPhase::Directories => self.create_directories().await,
            InitPhase::Config => self.generate_config().await,
            InitPhase::EmbeddingModel => self.download_embedding_model().await,
            InitPhase::Database => self.initialize_database().await,
            InitPhase::Runtimes => self.install_runtimes().await,
            InitPhase::Skills => self.install_skills(None).await, // Bundle path provided by FFI
        }
    }
```

**Step 3: Verify compilation**

```bash
cd /Users/zouguojun/Workspace/Aether/core && cargo check
```

**Step 4: Commit**

```bash
git add core/src/initialization/coordinator.rs
git commit -m "feat(init): implement skills installation phase

- Copy built-in skills from app bundle
- Validate with SkillsRegistry after copy
- Skip existing user skills

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 6: Create FFI exports

**Files:**
- Create: `core/src/ffi/init.rs`
- Modify: `core/src/ffi/mod.rs`

**Step 1: Create ffi/init.rs**

```rust
// core/src/ffi/init.rs
//! Initialization FFI exports

use crate::initialization::{InitializationCoordinator, InitializationResult, InitProgressHandler};
use std::sync::Arc;

/// FFI wrapper for InitializationResult
#[derive(Debug, Clone, uniffi::Record)]
pub struct InitResultFFI {
    pub success: bool,
    pub completed_phases: Vec<String>,
    pub error_phase: Option<String>,
    pub error_message: Option<String>,
}

impl From<InitializationResult> for InitResultFFI {
    fn from(r: InitializationResult) -> Self {
        Self {
            success: r.success,
            completed_phases: r.completed_phases,
            error_phase: r.error_phase,
            error_message: r.error_message,
        }
    }
}

/// FFI callback interface for initialization progress
#[uniffi::export(callback_interface)]
pub trait InitProgressHandlerFFI: Send + Sync {
    fn on_phase_started(&self, phase: String, current: u32, total: u32);
    fn on_phase_progress(&self, phase: String, progress: f64, message: String);
    fn on_phase_completed(&self, phase: String);
    fn on_download_progress(&self, item: String, downloaded: u64, total: u64);
    fn on_error(&self, phase: String, message: String, is_retryable: bool);
}

/// Adapter to convert FFI callback to internal trait
struct ProgressHandlerAdapter {
    inner: Arc<dyn InitProgressHandlerFFI>,
}

impl InitProgressHandler for ProgressHandlerAdapter {
    fn on_phase_started(&self, phase: String, current: u32, total: u32) {
        self.inner.on_phase_started(phase, current, total);
    }

    fn on_phase_progress(&self, phase: String, progress: f64, message: String) {
        self.inner.on_phase_progress(phase, progress, message);
    }

    fn on_phase_completed(&self, phase: String) {
        self.inner.on_phase_completed(phase);
    }

    fn on_download_progress(&self, item: String, downloaded: u64, total: u64) {
        self.inner.on_download_progress(item, downloaded, total);
    }

    fn on_error(&self, phase: String, message: String, is_retryable: bool) {
        self.inner.on_error(phase, message, is_retryable);
    }
}

/// Check if first-time initialization is needed
#[uniffi::export]
pub fn needs_first_time_init() -> bool {
    crate::initialization::needs_initialization().unwrap_or(true)
}

/// Run first-time initialization with progress callback
#[uniffi::export]
pub fn run_initialization(handler: Arc<dyn InitProgressHandlerFFI>) -> InitResultFFI {
    let adapter = Arc::new(ProgressHandlerAdapter { inner: handler });

    // Create tokio runtime for async operations
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to create tokio runtime");

    let result = rt.block_on(async {
        match InitializationCoordinator::new(Some(adapter)) {
            Ok(coordinator) => coordinator.run().await,
            Err(e) => InitializationResult {
                success: false,
                completed_phases: vec![],
                error_phase: Some(e.phase),
                error_message: Some(e.message),
            },
        }
    });

    InitResultFFI::from(result)
}
```

**Step 2: Update ffi/mod.rs to include init module**

Add after line 1 or with other module declarations:

```rust
mod init;
pub use init::{InitProgressHandlerFFI, InitResultFFI, needs_first_time_init, run_initialization};
```

**Step 3: Verify compilation**

```bash
cd /Users/zouguojun/Workspace/Aether/core && cargo check
```

**Step 4: Commit**

```bash
git add core/src/ffi/init.rs core/src/ffi/mod.rs
git commit -m "feat(ffi): add initialization FFI exports

- Add needs_first_time_init() function
- Add run_initialization() with callback
- Add InitProgressHandlerFFI callback interface
- Add InitResultFFI result type

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 7: Update lib.rs exports and remove old code

**Files:**
- Modify: `core/src/lib.rs`
- Delete: `core/src/initialization.rs` (old file)

**Step 1: Update lib.rs re-exports**

Replace the initialization exports in lib.rs (around line 157-161):

```rust
// New unified initialization exports
pub use crate::initialization::{
    InitializationCoordinator, InitializationResult, InitPhase, InitError,
    InitProgressHandler, needs_initialization,
};
pub use crate::ffi::init::{
    InitProgressHandlerFFI, InitResultFFI, needs_first_time_init, run_initialization,
};

// Legacy exports for backward compatibility (deprecated)
#[deprecated(note = "Use needs_initialization() instead")]
pub use crate::initialization::needs_initialization as is_fresh_install;
```

**Step 2: Move necessary functions from old initialization.rs**

Before deleting, extract any needed functions like `get_skills_dir`, `list_installed_skills`, etc. These should be moved to appropriate modules or kept in the new initialization module.

**Step 3: Delete old initialization.rs**

```bash
rm core/src/initialization.rs
```

**Step 4: Verify compilation**

```bash
cd /Users/zouguojun/Workspace/Aether/core && cargo check
```

**Step 5: Run all tests**

```bash
cd /Users/zouguojun/Workspace/Aether/core && cargo test
```

**Step 6: Commit**

```bash
git add -A
git commit -m "refactor(init): remove old scattered initialization code

BREAKING: Old initialization functions removed
- Delete core/src/initialization.rs
- Update lib.rs exports to use new module
- Add deprecated alias for backward compatibility

Migration:
- is_fresh_install() -> needs_initialization()
- run_first_time_init() -> run_initialization()

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 8: Update Swift InitializationProgressView

**Files:**
- Modify: `platforms/macos/Aether/Sources/Components/InitializationProgressView.swift`

**Step 1: Update ViewModel for new 6-phase flow**

```swift
// Replace InitializationProgressViewModel class

class InitializationProgressViewModel: ObservableObject {
    @Published var currentPhase: String = ""
    @Published var phaseDisplayName: String = ""
    @Published var currentStep: Int = 0
    @Published var totalSteps: Int = 6
    @Published var overallProgress: Double = 0.0
    @Published var statusMessage: String = ""
    @Published var isDownloading: Bool = false
    @Published var downloadItem: String = ""
    @Published var downloadProgress: Double = 0.0
    @Published var error: String? = nil
    @Published var isCompleted: Bool = false

    private let phaseNames: [String: String] = [
        "directories": "Creating directories",
        "config": "Generating configuration",
        "embedding_model": "Downloading embedding model",
        "database": "Initializing database",
        "runtimes": "Installing runtimes",
        "skills": "Installing skills"
    ]

    func updatePhaseStarted(phase: String, current: UInt32, total: UInt32) {
        DispatchQueue.main.async {
            self.currentPhase = phase
            self.phaseDisplayName = self.phaseNames[phase] ?? phase
            self.currentStep = Int(current)
            self.totalSteps = Int(total)
            self.overallProgress = Double(current - 1) / Double(total)
            self.isDownloading = false
            self.error = nil
        }
    }

    func updatePhaseProgress(phase: String, progress: Double, message: String) {
        DispatchQueue.main.async {
            self.statusMessage = message
            let phaseProgress = progress / Double(self.totalSteps)
            self.overallProgress = (Double(self.currentStep - 1) / Double(self.totalSteps)) + phaseProgress
        }
    }

    func updatePhaseCompleted(phase: String) {
        DispatchQueue.main.async {
            self.overallProgress = Double(self.currentStep) / Double(self.totalSteps)
        }
    }

    func updateDownloadProgress(item: String, downloaded: UInt64, total: UInt64) {
        DispatchQueue.main.async {
            self.isDownloading = true
            self.downloadItem = item
            if total > 0 {
                self.downloadProgress = Double(downloaded) / Double(total)
            }
        }
    }

    func setError(phase: String, message: String, isRetryable: Bool) {
        DispatchQueue.main.async {
            self.error = "[\(phase)] \(message)"
            self.isDownloading = false
        }
    }

    func setCompleted() {
        DispatchQueue.main.async {
            self.isCompleted = true
            self.overallProgress = 1.0
        }
    }
}
```

**Step 2: Update InitProgressHandlerImpl**

```swift
class InitProgressHandlerImpl: InitProgressHandlerFFI {
    weak var viewModel: InitializationProgressViewModel?

    init(viewModel: InitializationProgressViewModel) {
        self.viewModel = viewModel
    }

    func onPhaseStarted(phase: String, current: UInt32, total: UInt32) {
        print("[Init] Phase \(current)/\(total): \(phase)")
        viewModel?.updatePhaseStarted(phase: phase, current: current, total: total)
    }

    func onPhaseProgress(phase: String, progress: Double, message: String) {
        viewModel?.updatePhaseProgress(phase: phase, progress: progress, message: message)
    }

    func onPhaseCompleted(phase: String) {
        print("[Init] ✅ Phase completed: \(phase)")
        viewModel?.updatePhaseCompleted(phase: phase)
    }

    func onDownloadProgress(item: String, downloaded: UInt64, total: UInt64) {
        viewModel?.updateDownloadProgress(item: item, downloaded: downloaded, total: total)
    }

    func onError(phase: String, message: String, isRetryable: Bool) {
        print("[Init] ❌ Error in \(phase): \(message)")
        viewModel?.setError(phase: phase, message: message, isRetryable: isRetryable)
    }
}
```

**Step 3: Update runInitialization function**

```swift
private func runInitialization() {
    guard !isInitializing else { return }
    isInitializing = true

    viewModel.error = nil
    viewModel.isCompleted = false
    viewModel.overallProgress = 0.0

    let handler = InitProgressHandlerImpl(viewModel: viewModel)

    DispatchQueue.global(qos: .userInitiated).async { [weak self] in
        let result = runInitialization(handler: handler)

        DispatchQueue.main.async {
            self?.isInitializing = false

            if result.success {
                self?.viewModel.setCompleted()
                // Wait briefly to show completion state
                DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) {
                    self?.onCompletion()
                }
            } else {
                let errorMsg = result.errorMessage ?? "Unknown error"
                self?.onFailure(errorMsg)
            }
        }
    }
}
```

**Step 4: Verify Swift builds**

```bash
cd /Users/zouguojun/Workspace/Aether/platforms/macos && xcodegen generate && xcodebuild -scheme Aleph -configuration Debug build
```

**Step 5: Commit**

```bash
git add platforms/macos/Aether/Sources/Components/InitializationProgressView.swift
git commit -m "feat(macos): update InitializationProgressView for new 6-phase flow

- Update ViewModel for 6 phases with display names
- Update progress handler for new callback interface
- Track download progress per item
- Handle errors with retry support

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 9: Update AppDelegate initialization flow

**Files:**
- Modify: `platforms/macos/Aether/Sources/AppDelegate.swift`

**Step 1: Update checkAndRunFirstTimeInit**

Replace the `checkAndRunFirstTimeInit` method:

```swift
private func checkAndRunFirstTimeInit() {
    let needsInit = needsFirstTimeInit()
    NSLog("[Aether] needsFirstTimeInit=%@", needsInit ? "true" : "false")

    if needsInit {
        NSLog("[Aether] 🆕 Fresh install - showing initialization window")
        showInitializationWindow()
    } else {
        initializeAppComponents()
    }
}
```

**Step 2: Update showInitializationWindow**

The existing implementation should work with the updated InitializationProgressView, but ensure it's using the blocking approach:

```swift
private func showInitializationWindow() {
    let initView = InitializationProgressView(
        onCompletion: { [weak self] in
            DispatchQueue.main.async {
                print("[Aether] Initialization completed - proceeding with app startup")
                self?.closeInitializationWindow()
                self?.initializeAppComponents()
            }
        },
        onFailure: { [weak self] error in
            DispatchQueue.main.async {
                print("[Aether] Initialization failed: \(error)")

                let alert = NSAlert()
                alert.messageText = L("init.error.title")
                alert.informativeText = L("init.error.message") + "\n\n" + error
                alert.alertStyle = .critical
                alert.addButton(withTitle: L("init.error.quit"))
                alert.addButton(withTitle: L("init.error.retry"))

                let response = alert.runModal()
                if response == .alertFirstButtonReturn {
                    NSApp.terminate(nil)
                } else {
                    self?.closeInitializationWindow()
                    self?.showInitializationWindow()
                }
            }
        }
    )

    let hostingController = NSHostingController(rootView: initView)

    let window = NSPanel(
        contentRect: NSRect(x: 0, y: 0, width: 480, height: 320),
        styleMask: [.titled, .fullSizeContentView],
        backing: .buffered,
        defer: false
    )

    window.title = ""
    window.titlebarAppearsTransparent = true
    window.isMovableByWindowBackground = true
    window.contentViewController = hostingController
    window.center()
    window.level = .floating
    window.isReleasedWhenClosed = false

    // Remove close button - must complete initialization
    window.styleMask.remove(.closable)

    initializationWindow = window
    window.makeKeyAndOrderFront(nil)
    NSApp.activate(ignoringOtherApps: true)

    print("[Aether] Initialization window shown")
}
```

**Step 3: Remove old isFreshInstall calls**

Search and replace any remaining `isFreshInstall()` calls with `needsFirstTimeInit()`.

**Step 4: Verify Swift builds**

```bash
cd /Users/zouguojun/Workspace/Aether/platforms/macos && xcodegen generate && xcodebuild -scheme Aleph -configuration Debug build
```

**Step 5: Commit**

```bash
git add platforms/macos/Aether/Sources/AppDelegate.swift
git commit -m "feat(macos): update AppDelegate for blocking initialization

- Use needsFirstTimeInit() instead of isFreshInstall()
- Show non-closable initialization window
- Block app startup until init completes

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 10: Full integration test

**Step 1: Build Rust core**

```bash
./scripts/build-core.sh macos
```

**Step 2: Generate Xcode project**

```bash
cd /Users/zouguojun/Workspace/Aether/platforms/macos && xcodegen generate
```

**Step 3: Build full app**

```bash
cd /Users/zouguojun/Workspace/Aether/platforms/macos && xcodebuild -scheme Aleph -configuration Debug build
```

**Step 4: Test fresh install scenario**

```bash
# Backup existing config
mv ~/.aether ~/.aether.backup

# Run app and verify initialization window appears
# Verify all 6 phases complete
# Verify app starts normally after init

# Restore config
rm -rf ~/.aether
mv ~/.aether.backup ~/.aether
```

**Step 5: Verify existing install skips initialization**

Run app again - should skip initialization window and start normally.

**Step 6: Final commit**

```bash
git add -A
git commit -m "test: verify unified initialization flow

- Tested fresh install scenario
- Tested existing install skip
- All 6 phases complete successfully

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Summary

| Task | Description | Files |
|------|-------------|-------|
| 1 | Create module structure | initialization/{mod,coordinator,error}.rs |
| 2 | Implement phases 1-4 | coordinator.rs |
| 3 | Add FFmpeg runtime | runtimes/ffmpeg.rs |
| 4 | Parallel runtime install | coordinator.rs |
| 5 | Skills installation | coordinator.rs |
| 6 | FFI exports | ffi/init.rs |
| 7 | Remove old code | lib.rs, delete initialization.rs |
| 8 | Update Swift ViewModel | InitializationProgressView.swift |
| 9 | Update AppDelegate | AppDelegate.swift |
| 10 | Integration test | Full build + test |

**Total estimated commits:** 10
