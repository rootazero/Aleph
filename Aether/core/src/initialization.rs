/// First-run initialization module
///
/// This module handles automatic initialization on first launch:
/// - Detects if this is a fresh installation
/// - Creates directory structure
/// - Generates default config file
/// - Downloads embedding model files
/// - Initializes memory database
use crate::config::Config;
use crate::error::{AetherError, Result};
use std::fs;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tracing::{debug, info, warn};

/// Initialization progress callback trait
pub trait InitializationProgressHandler: Send + Sync {
    /// Called when initialization starts
    fn on_init_started(&self);

    /// Called when a step begins
    /// - step_name: "Creating directories", "Downloading model", etc.
    /// - current: Current step number (1-based)
    /// - total: Total number of steps
    fn on_step_started(&self, step_name: String, current: u32, total: u32);

    /// Called when download progress updates
    /// - downloaded_bytes: Bytes downloaded so far
    /// - total_bytes: Total file size (0 if unknown)
    fn on_download_progress(&self, downloaded_bytes: u64, total_bytes: u64);

    /// Called when a step completes successfully
    fn on_step_completed(&self, step_name: String);

    /// Called when initialization completes successfully
    fn on_init_completed(&self);

    /// Called when initialization fails
    fn on_init_failed(&self, error: String);
}

/// Check if this is a fresh installation
pub fn is_fresh_install() -> Result<bool> {
    let config_dir = get_config_dir()?;

    // Check if config directory exists and has essential files
    if !config_dir.exists() {
        info!("Config directory does not exist - fresh install");
        return Ok(true);
    }

    // Check for config file
    let config_file = config_dir.join("config.toml");
    let has_config = config_file.exists();

    // Check for model files
    let model_dir = get_model_dir()?;
    let has_model = model_dir.join("model.onnx").exists()
        && model_dir.join("tokenizer.json").exists();

    // If both config and model exist, this is not a fresh install
    let is_fresh = !has_config || !has_model;

    if is_fresh {
        info!(
            has_config = has_config,
            has_model = has_model,
            "Detected fresh install or incomplete installation"
        );
    } else {
        info!("Detected existing installation");
    }

    Ok(is_fresh)
}

/// Get the config directory path
pub fn get_config_dir() -> Result<PathBuf> {
    let home_dir = std::env::var("HOME")
        .map_err(|_| AetherError::config("Failed to get HOME environment variable"))?;

    Ok(PathBuf::from(home_dir).join(".config").join("aether"))
}

/// Get the model directory path
pub fn get_model_dir() -> Result<PathBuf> {
    Ok(get_config_dir()?
        .join("models")
        .join("all-MiniLM-L6-v2"))
}

/// Run first-time initialization
///
/// This function will:
/// 1. Create directory structure
/// 2. Generate default config file
/// 3. Download embedding model
/// 4. Initialize memory database
///
/// # Arguments
/// * `progress_callback` - Optional callback for progress updates
pub async fn run_first_time_init_async(
    progress_callback: Option<Box<dyn InitializationProgressHandler>>,
) -> Result<()> {
    info!("Starting first-time initialization");

    let callback = progress_callback.as_ref();

    // Notify start
    if let Some(cb) = callback {
        cb.on_init_started();
    }

    const TOTAL_STEPS: u32 = 4;
    let mut current_step = 0;

    // Step 1: Create directory structure
    current_step += 1;
    if let Some(cb) = callback {
        cb.on_step_started("Creating directories".to_string(), current_step, TOTAL_STEPS);
    }

    create_directory_structure().await.map_err(|e| {
        if let Some(cb) = callback {
            cb.on_init_failed(e.to_string());
        }
        e
    })?;

    if let Some(cb) = callback {
        cb.on_step_completed("Creating directories".to_string());
    }

    // Step 2: Generate default config file
    current_step += 1;
    if let Some(cb) = callback {
        cb.on_step_started(
            "Generating default configuration".to_string(),
            current_step,
            TOTAL_STEPS,
        );
    }

    create_default_config().await.map_err(|e| {
        if let Some(cb) = callback {
            cb.on_init_failed(e.to_string());
        }
        e
    })?;

    if let Some(cb) = callback {
        cb.on_step_completed("Generating default configuration".to_string());
    }

    // Step 3: Download embedding model
    current_step += 1;
    if let Some(cb) = callback {
        cb.on_step_started(
            "Downloading embedding model".to_string(),
            current_step,
            TOTAL_STEPS,
        );
    }

    download_embedding_model(callback).await.map_err(|e| {
        if let Some(cb) = callback {
            cb.on_init_failed(e.to_string());
        }
        e
    })?;

    if let Some(cb) = callback {
        cb.on_step_completed("Downloading embedding model".to_string());
    }

    // Step 4: Initialize memory database
    current_step += 1;
    if let Some(cb) = callback {
        cb.on_step_started(
            "Initializing memory database".to_string(),
            current_step,
            TOTAL_STEPS,
        );
    }

    initialize_memory_database().await.map_err(|e| {
        if let Some(cb) = callback {
            cb.on_init_failed(e.to_string());
        }
        e
    })?;

    if let Some(cb) = callback {
        cb.on_step_completed("Initializing memory database".to_string());
    }

    // Notify completion
    if let Some(cb) = callback {
        cb.on_init_completed();
    }

    info!("First-time initialization completed successfully");
    Ok(())
}

/// Synchronous wrapper for run_first_time_init (for UniFFI export)
///
/// This function creates a Tokio runtime and blocks on the async initialization.
/// It's necessary because UniFFI cannot export async functions directly.
pub fn run_first_time_init(
    progress_callback: Option<Box<dyn InitializationProgressHandler>>,
) -> Result<()> {
    // Create a new Tokio runtime for this operation
    // We use a multi-threaded runtime to allow concurrent downloads
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| AetherError::config(format!("Failed to create Tokio runtime: {}", e)))?;

    // Block on the async initialization
    runtime.block_on(run_first_time_init_async(progress_callback))
}

/// Create the directory structure
async fn create_directory_structure() -> Result<()> {
    let config_dir = get_config_dir()?;
    let model_dir = get_model_dir()?;
    let logs_dir = config_dir.join("logs");

    debug!("Creating config directory: {:?}", config_dir);
    fs::create_dir_all(&config_dir).map_err(|e| {
        AetherError::config(format!("Failed to create config directory: {}", e))
    })?;

    debug!("Creating model directory: {:?}", model_dir);
    fs::create_dir_all(&model_dir).map_err(|e| {
        AetherError::config(format!("Failed to create model directory: {}", e))
    })?;

    debug!("Creating logs directory: {:?}", logs_dir);
    fs::create_dir_all(&logs_dir)
        .map_err(|e| AetherError::config(format!("Failed to create logs directory: {}", e)))?;

    info!("✅ Directory structure created");
    Ok(())
}

/// Create default config file
async fn create_default_config() -> Result<()> {
    let config_path = get_config_dir()?.join("config.toml");

    // Check if config already exists
    if config_path.exists() {
        warn!("Config file already exists, skipping");
        return Ok(());
    }

    let default_config = Config::default();

    debug!("Saving default config to: {:?}", config_path);
    default_config.save_to_file(&config_path)?;

    info!("✅ Default configuration file created");
    Ok(())
}

/// Download embedding model files from HuggingFace
async fn download_embedding_model(
    progress_callback: Option<&Box<dyn InitializationProgressHandler>>,
) -> Result<()> {
    let model_dir = get_model_dir()?;
    let model_file = model_dir.join("model.onnx");
    let tokenizer_file = model_dir.join("tokenizer.json");

    // Check if files already exist
    if model_file.exists() && tokenizer_file.exists() {
        warn!("Embedding model files already exist, skipping download");
        return Ok(());
    }

    const BASE_URL: &str = "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main";

    // Download model.onnx
    if !model_file.exists() {
        let url = format!("{}/onnx/model.onnx", BASE_URL);
        info!("Downloading model.onnx from {}", url);

        download_file(&url, &model_file, progress_callback).await?;
    }

    // Download tokenizer.json
    if !tokenizer_file.exists() {
        let url = format!("{}/tokenizer.json", BASE_URL);
        info!("Downloading tokenizer.json from {}", url);

        download_file(&url, &tokenizer_file, progress_callback).await?;
    }

    info!("✅ Embedding model downloaded successfully");
    Ok(())
}

/// Download a file from URL to local path
async fn download_file(
    url: &str,
    dest_path: &Path,
    progress_callback: Option<&Box<dyn InitializationProgressHandler>>,
) -> Result<()> {
    debug!("Downloading {} to {:?}", url, dest_path);

    // Create HTTP client
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300)) // 5 minute timeout
        .build()
        .map_err(|e| AetherError::config(format!("Failed to create HTTP client: {}", e)))?;

    // Send GET request
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| AetherError::config(format!("Failed to download file: {}", e)))?;

    // Check response status
    if !response.status().is_success() {
        return Err(AetherError::config(format!(
            "HTTP error {}: {}",
            response.status(),
            url
        )));
    }

    // Get total size from Content-Length header
    let total_size = response.content_length().unwrap_or(0);

    debug!(
        "Download started - total size: {} bytes",
        if total_size > 0 {
            format!("{:.2} MB", total_size as f64 / 1024.0 / 1024.0)
        } else {
            "unknown".to_string()
        }
    );

    // Create temp file
    let temp_path = dest_path.with_extension("tmp");
    let mut file = tokio::fs::File::create(&temp_path).await.map_err(|e| {
        AetherError::config(format!("Failed to create temp file: {}", e))
    })?;

    // Download with progress updates
    let mut downloaded: u64 = 0;
    let mut stream = response.bytes_stream();

    use futures_util::StreamExt;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(|e| AetherError::config(format!("Failed to read download chunk: {}", e)))?;

        file.write_all(&chunk).await.map_err(|e| {
            AetherError::config(format!("Failed to write to file: {}", e))
        })?;

        downloaded += chunk.len() as u64;

        // Update progress (every 1MB or at completion)
        if downloaded % (1024 * 1024) == 0 || downloaded == total_size {
            if let Some(cb) = progress_callback {
                cb.on_download_progress(downloaded, total_size);
            }

            if total_size > 0 {
                let percentage = (downloaded as f64 / total_size as f64) * 100.0;
                debug!(
                    "Download progress: {:.2}% ({}/{} MB)",
                    percentage,
                    downloaded / 1024 / 1024,
                    total_size / 1024 / 1024
                );
            }
        }
    }

    // Flush and sync
    file.sync_all()
        .await
        .map_err(|e| AetherError::config(format!("Failed to sync file: {}", e)))?;

    drop(file); // Close file before rename

    // Atomic rename
    tokio::fs::rename(&temp_path, dest_path)
        .await
        .map_err(|e| {
            let _ = std::fs::remove_file(&temp_path); // Cleanup on error
            AetherError::config(format!("Failed to rename temp file: {}", e))
        })?;

    info!(
        "✅ Downloaded {} ({:.2} MB)",
        dest_path.file_name().unwrap().to_string_lossy(),
        downloaded as f64 / 1024.0 / 1024.0
    );

    Ok(())
}

/// Initialize memory database
async fn initialize_memory_database() -> Result<()> {
    let db_path = get_config_dir()?.join("memory.db");

    debug!("Initializing memory database at: {:?}", db_path);

    // Import VectorDatabase to trigger schema creation
    use crate::memory::database::VectorDatabase;

    let _db = VectorDatabase::new(db_path.clone()).map_err(|e| {
        AetherError::config(format!("Failed to initialize memory database: {}", e))
    })?;

    info!("✅ Memory database initialized");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_config_dir() {
        let dir = get_config_dir().unwrap();
        assert!(dir.to_string_lossy().contains(".config/aether"));
    }

    #[test]
    fn test_get_model_dir() {
        let dir = get_model_dir().unwrap();
        assert!(dir.to_string_lossy().contains("all-MiniLM-L6-v2"));
    }

    #[tokio::test]
    async fn test_create_directory_structure() {
        use tempfile::TempDir;

        // Save original HOME
        let original_home = std::env::var("HOME").ok();

        // Create temp directory
        let temp_dir = TempDir::new().unwrap();
        std::env::set_var("HOME", temp_dir.path());

        // Run directory creation
        create_directory_structure().await.unwrap();

        // Verify directories exist
        let config_dir = temp_dir.path().join(".config").join("aether");
        assert!(config_dir.exists());
        assert!(config_dir.join("models").join("all-MiniLM-L6-v2").exists());
        assert!(config_dir.join("logs").exists());

        // Restore original HOME
        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }
    }

    #[tokio::test]
    async fn test_create_default_config() {
        use tempfile::TempDir;

        // Save original HOME
        let original_home = std::env::var("HOME").ok();

        let temp_dir = TempDir::new().unwrap();
        std::env::set_var("HOME", temp_dir.path());

        // Create directory structure first
        create_directory_structure().await.unwrap();

        // Create default config
        create_default_config().await.unwrap();

        // Verify config file exists
        let config_file = temp_dir.path().join(".config").join("aether").join("config.toml");
        assert!(config_file.exists());

        // Verify config can be loaded
        let config = Config::load_from_file(&config_file).unwrap();
        assert_eq!(config.default_hotkey, "Grave");

        // Restore original HOME
        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }
    }
}
