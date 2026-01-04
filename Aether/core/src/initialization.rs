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
    let has_model = check_embedding_model_exists()?;

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

/// Check if embedding model files exist and are valid
pub fn check_embedding_model_exists() -> Result<bool> {
    let model_dir = get_model_dir()?;
    let model_file = model_dir.join("model.onnx");
    let tokenizer_file = model_dir.join("tokenizer.json");

    // Check if files exist
    if !model_file.exists() || !tokenizer_file.exists() {
        debug!("Embedding model files do not exist");
        return Ok(false);
    }

    // Validate file sizes
    match validate_model_files(&model_file, &tokenizer_file) {
        Ok(valid) => {
            if valid {
                info!("✅ Embedding model files exist and are valid");
            } else {
                warn!("⚠️  Embedding model files exist but are invalid");
            }
            Ok(valid)
        }
        Err(e) => {
            warn!("Failed to validate model files: {}", e);
            Ok(false)
        }
    }
}

/// Download embedding model files standalone (for manual triggering)
/// Returns true if download succeeded, false if failed
pub fn download_embedding_model_standalone(
    progress_callback: Option<Box<dyn InitializationProgressHandler>>,
) -> Result<bool> {
    info!("Starting standalone embedding model download");

    let callback = progress_callback.as_ref();

    // Notify start
    if let Some(cb) = callback {
        cb.on_init_started();
        cb.on_step_started("Downloading embedding model".to_string(), 1, 1);
    }

    // Create a Tokio runtime for async operations
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| AetherError::config(format!("Failed to create Tokio runtime: {}", e)))?;

    // Try to download the model
    let result = runtime.block_on(async { download_embedding_model(callback.map(|v| &**v)).await });

    match result {
        Ok(()) => {
            info!("✅ Standalone embedding model download succeeded");
            if let Some(cb) = callback {
                cb.on_step_completed("Downloading embedding model".to_string());
                cb.on_init_completed();
            }
            Ok(true)
        }
        Err(e) => {
            warn!("⚠️  Standalone embedding model download failed: {}", e);
            if let Some(cb) = callback {
                cb.on_init_failed(e.to_string());
            }
            Ok(false) // Return false instead of error to allow graceful handling
        }
    }
}

/// Get the config directory path
pub fn get_config_dir() -> Result<PathBuf> {
    let home_dir = std::env::var("HOME")
        .map_err(|_| AetherError::config("Failed to get HOME environment variable"))?;

    Ok(PathBuf::from(home_dir).join(".config").join("aether"))
}

/// Get the model directory path
pub fn get_model_dir() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("models").join("all-MiniLM-L6-v2"))
}

/// Run first-time initialization
///
/// This function will:
/// 1. Check disk space
/// 2. Create directory structure
/// 3. Generate default config file
/// 4. Download embedding model
/// 5. Initialize memory database
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

    // Pre-check: Verify sufficient disk space
    check_disk_space()?;

    const TOTAL_STEPS: u32 = 4;
    let mut current_step = 0;

    // Step 1: Create directory structure
    current_step += 1;
    if let Some(cb) = callback {
        cb.on_step_started(
            "Creating directories".to_string(),
            current_step,
            TOTAL_STEPS,
        );
    }

    create_directory_structure().await.inspect_err(|e| {
        if let Some(cb) = callback {
            cb.on_init_failed(e.to_string());
        }
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

    create_default_config().await.inspect_err(|e| {
        if let Some(cb) = callback {
            cb.on_init_failed(e.to_string());
        }
    })?;

    if let Some(cb) = callback {
        cb.on_step_completed("Generating default configuration".to_string());
    }

    // Step 3: Download embedding model (allow graceful fallback on failure)
    current_step += 1;
    if let Some(cb) = callback {
        cb.on_step_started(
            "Downloading embedding model".to_string(),
            current_step,
            TOTAL_STEPS,
        );
    }

    // Try to download the model, but don't fail the entire initialization if it fails
    let model_downloaded = match download_embedding_model(callback.map(|v| &**v)).await {
        Ok(()) => {
            info!("✅ Embedding model downloaded successfully");
            true
        }
        Err(e) => {
            warn!("⚠️  Failed to download embedding model: {}", e);
            warn!("Continuing initialization without memory functionality");

            // Notify callback about the warning
            if let Some(cb) = callback {
                cb.on_step_completed("Embedding model (skipped - offline mode)".to_string());
            }
            false
        }
    };

    if model_downloaded {
        if let Some(cb) = callback {
            cb.on_step_completed("Downloading embedding model".to_string());
        }
    }

    // Step 4: Initialize memory database (only if model was downloaded)
    current_step += 1;
    if let Some(cb) = callback {
        cb.on_step_started(
            "Initializing memory database".to_string(),
            current_step,
            TOTAL_STEPS,
        );
    }

    if model_downloaded {
        initialize_memory_database().await.inspect_err(|e| {
            if let Some(cb) = callback {
                cb.on_init_failed(e.to_string());
            }
        })?;

        if let Some(cb) = callback {
            cb.on_step_completed("Initializing memory database".to_string());
        }
    } else {
        warn!("Skipping memory database initialization (no embedding model)");
        if let Some(cb) = callback {
            cb.on_step_completed("Memory database (skipped - offline mode)".to_string());
        }
    }

    // Update config to disable memory if model wasn't downloaded
    if !model_downloaded {
        update_config_memory_setting(false).await?;
        warn!("⚠️  Initialization completed in offline mode - memory functionality disabled");
    }

    // Notify completion
    if let Some(cb) = callback {
        cb.on_init_completed();
    }

    if model_downloaded {
        info!("✅ First-time initialization completed successfully");
    } else {
        info!("✅ First-time initialization completed in offline mode (memory disabled)");
    }

    Ok(())
}

/// Update the memory.enabled setting in config file
async fn update_config_memory_setting(enabled: bool) -> Result<()> {
    let config_path = get_config_dir()?.join("config.toml");

    if !config_path.exists() {
        return Ok(()); // Config not created yet, will be handled by default config
    }

    // Load existing config
    let mut config = Config::load_from_file(&config_path)?;

    // Update memory enabled setting
    config.memory.enabled = enabled;

    // Save back to file
    config.save_to_file(&config_path)?;

    info!("Updated config: memory.enabled = {}", enabled);

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

/// Check if there is sufficient disk space for initialization
/// This is a lightweight check that attempts to create a test file
fn check_disk_space() -> Result<()> {
    const TEST_FILE_SIZE: usize = 100 * 1024 * 1024; // 100 MB test
    const CHUNK_SIZE: usize = 1024 * 1024; // 1 MB chunks

    let config_dir = get_config_dir()?;

    // Get parent directory if config dir doesn't exist yet
    let check_path = if config_dir.exists() {
        config_dir.clone()
    } else {
        // Create parent directories if needed
        if let Some(parent) = config_dir.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                AetherError::config(format!("Failed to create parent directory: {}", e))
            })?;
        }
        config_dir.clone()
    };

    // Ensure the directory exists for the test
    fs::create_dir_all(&check_path)
        .map_err(|e| AetherError::config(format!("Failed to create test directory: {}", e)))?;

    let test_file = check_path.join(".disk_space_test.tmp");

    debug!("Testing disk space with test file: {:?}", test_file);

    // Try to write a test file to verify disk space
    match std::fs::File::create(&test_file) {
        Ok(mut file) => {
            use std::io::Write;

            let buffer = vec![0u8; CHUNK_SIZE];
            let mut written = 0;

            // Try to write up to 100MB in 1MB chunks
            while written < TEST_FILE_SIZE {
                match file.write(&buffer) {
                    Ok(bytes_written) => written += bytes_written,
                    Err(e) => {
                        let _ = std::fs::remove_file(&test_file);
                        return Err(AetherError::config(format!(
                            "Insufficient disk space (failed after {} MB): {}",
                            written / (1024 * 1024),
                            e
                        )));
                    }
                }
            }

            // Clean up test file
            drop(file);
            let _ = std::fs::remove_file(&test_file);

            info!(
                "✅ Sufficient disk space available (verified {} MB)",
                written / (1024 * 1024)
            );
            Ok(())
        }
        Err(e) => Err(AetherError::config(format!(
            "Failed to create disk space test file: {}",
            e
        ))),
    }
}

/// Create the directory structure
async fn create_directory_structure() -> Result<()> {
    let config_dir = get_config_dir()?;
    let model_dir = get_model_dir()?;
    let logs_dir = config_dir.join("logs");

    debug!("Creating config directory: {:?}", config_dir);
    fs::create_dir_all(&config_dir)
        .map_err(|e| AetherError::config(format!("Failed to create config directory: {}", e)))?;

    debug!("Creating model directory: {:?}", model_dir);
    fs::create_dir_all(&model_dir)
        .map_err(|e| AetherError::config(format!("Failed to create model directory: {}", e)))?;

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

/// Expected file sizes for validation (approximate, in bytes)
const EXPECTED_MODEL_SIZE_MIN: u64 = 80 * 1024 * 1024; // 80 MB
const EXPECTED_MODEL_SIZE_MAX: u64 = 100 * 1024 * 1024; // 100 MB
const EXPECTED_TOKENIZER_SIZE_MIN: u64 = 100 * 1024; // 100 KB
const EXPECTED_TOKENIZER_SIZE_MAX: u64 = 2 * 1024 * 1024; // 2 MB

/// Download embedding model files from HuggingFace
async fn download_embedding_model(
    progress_callback: Option<&dyn InitializationProgressHandler>,
) -> Result<()> {
    let model_dir = get_model_dir()?;
    let model_file = model_dir.join("model.onnx");
    let tokenizer_file = model_dir.join("tokenizer.json");

    // Check if files already exist and are valid
    if model_file.exists() && tokenizer_file.exists() {
        if validate_model_files(&model_file, &tokenizer_file)? {
            warn!("Embedding model files already exist and are valid, skipping download");
            return Ok(());
        } else {
            warn!("Existing model files are invalid, re-downloading...");
            // Delete invalid files
            let _ = std::fs::remove_file(&model_file);
            let _ = std::fs::remove_file(&tokenizer_file);
        }
    }

    const BASE_URL: &str =
        "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main";

    // Download model.onnx
    if !model_file.exists() {
        let url = format!("{}/onnx/model.onnx", BASE_URL);
        info!("Downloading model.onnx from {}", url);

        download_file(&url, &model_file, progress_callback).await?;

        // Validate downloaded file
        validate_file_size(
            &model_file,
            EXPECTED_MODEL_SIZE_MIN,
            EXPECTED_MODEL_SIZE_MAX,
        )?;
    }

    // Download tokenizer.json
    if !tokenizer_file.exists() {
        let url = format!("{}/tokenizer.json", BASE_URL);
        info!("Downloading tokenizer.json from {}", url);

        download_file(&url, &tokenizer_file, progress_callback).await?;

        // Validate downloaded file
        validate_file_size(
            &tokenizer_file,
            EXPECTED_TOKENIZER_SIZE_MIN,
            EXPECTED_TOKENIZER_SIZE_MAX,
        )?;
    }

    info!("✅ Embedding model downloaded and validated successfully");
    Ok(())
}

/// Validate model files exist and have reasonable sizes
fn validate_model_files(model_file: &Path, tokenizer_file: &Path) -> Result<bool> {
    if !model_file.exists() || !tokenizer_file.exists() {
        return Ok(false);
    }

    // Check file sizes
    match validate_file_size(model_file, EXPECTED_MODEL_SIZE_MIN, EXPECTED_MODEL_SIZE_MAX) {
        Ok(_) => {}
        Err(_) => return Ok(false),
    }

    match validate_file_size(
        tokenizer_file,
        EXPECTED_TOKENIZER_SIZE_MIN,
        EXPECTED_TOKENIZER_SIZE_MAX,
    ) {
        Ok(_) => {}
        Err(_) => return Ok(false),
    }

    Ok(true)
}

/// Validate file size is within expected range
fn validate_file_size(path: &Path, min_size: u64, max_size: u64) -> Result<()> {
    let metadata = std::fs::metadata(path)
        .map_err(|e| AetherError::config(format!("Failed to read file metadata: {}", e)))?;

    let size = metadata.len();

    if size < min_size {
        return Err(AetherError::config(format!(
            "File {} is too small ({} bytes, expected at least {} bytes). Download may be incomplete.",
            path.file_name().unwrap().to_string_lossy(),
            size,
            min_size
        )));
    }

    if size > max_size {
        return Err(AetherError::config(format!(
            "File {} is too large ({} bytes, expected at most {} bytes). File may be corrupted.",
            path.file_name().unwrap().to_string_lossy(),
            size,
            max_size
        )));
    }

    debug!(
        "✅ File {} validated: {} bytes",
        path.file_name().unwrap().to_string_lossy(),
        size
    );

    Ok(())
}

/// Download a file from URL to local path with automatic retry
async fn download_file(
    url: &str,
    dest_path: &Path,
    progress_callback: Option<&dyn InitializationProgressHandler>,
) -> Result<()> {
    const MAX_RETRIES: u32 = 3;
    const INITIAL_BACKOFF_MS: u64 = 1000; // 1 second

    let mut last_error = None;

    for attempt in 1..=MAX_RETRIES {
        debug!(
            "Downloading {} to {:?} (attempt {}/{})",
            url, dest_path, attempt, MAX_RETRIES
        );

        match download_file_once(url, dest_path, progress_callback).await {
            Ok(()) => {
                info!(
                    "✅ Successfully downloaded {} on attempt {}",
                    dest_path.file_name().unwrap().to_string_lossy(),
                    attempt
                );
                return Ok(());
            }
            Err(e) => {
                warn!("Download attempt {}/{} failed: {}", attempt, MAX_RETRIES, e);
                last_error = Some(e);

                // If not the last attempt, wait with exponential backoff
                if attempt < MAX_RETRIES {
                    let backoff_ms = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                    info!("Retrying in {} ms...", backoff_ms);
                    tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                }
            }
        }
    }

    // All retries exhausted
    Err(last_error.unwrap_or_else(|| AetherError::config("Download failed".to_string())))
}

/// Download a file from URL to local path (single attempt)
async fn download_file_once(
    url: &str,
    dest_path: &Path,
    progress_callback: Option<&dyn InitializationProgressHandler>,
) -> Result<()> {
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
    let mut file = tokio::fs::File::create(&temp_path)
        .await
        .map_err(|e| AetherError::config(format!("Failed to create temp file: {}", e)))?;

    // Download with progress updates
    let mut downloaded: u64 = 0;
    let mut stream = response.bytes_stream();

    use futures_util::StreamExt;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(|e| AetherError::config(format!("Failed to read download chunk: {}", e)))?;

        file.write_all(&chunk)
            .await
            .map_err(|e| AetherError::config(format!("Failed to write to file: {}", e)))?;

        downloaded += chunk.len() as u64;

        // Update progress (every 1MB or at completion)
        if downloaded.is_multiple_of(1024 * 1024) || downloaded == total_size {
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

    let _db = VectorDatabase::new(db_path.clone())
        .map_err(|e| AetherError::config(format!("Failed to initialize memory database: {}", e)))?;

    info!("✅ Memory database initialized");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

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
    #[serial]
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
    #[serial]
    async fn test_create_default_config() {
        use tempfile::TempDir;

        // Save original HOME
        let original_home = std::env::var("HOME").ok();

        let temp_dir = TempDir::new().unwrap();
        std::env::set_var("HOME", temp_dir.path());

        // Create directory structure first
        create_directory_structure().await.unwrap();

        // Verify config_file path
        let config_file = temp_dir
            .path()
            .join(".config")
            .join("aether")
            .join("config.toml");

        // Ensure config file doesn't exist before creation
        if config_file.exists() {
            std::fs::remove_file(&config_file).unwrap();
        }

        // Create default config
        create_default_config().await.unwrap();

        // Verify config file exists
        assert!(
            config_file.exists(),
            "Config file should exist at {:?}",
            config_file
        );

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
