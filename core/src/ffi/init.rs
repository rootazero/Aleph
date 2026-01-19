//! Initialization FFI exports
//!
//! Provides UniFFI-compatible wrappers for the unified initialization module.
//! This module exposes initialization functions for Swift/Kotlin clients to perform
//! first-time setup including directory creation, config generation, embedding model
//! download, database initialization, runtime installation, and skills setup.

use crate::init_unified::{InitProgressHandler, InitializationCoordinator, InitializationResult};
use std::sync::Arc;

/// FFI wrapper for InitializationResult
///
/// This struct mirrors `InitializationResult` but uses UniFFI-compatible types.
#[derive(Debug, Clone, uniffi::Record)]
pub struct InitResultFFI {
    /// Whether initialization completed successfully
    pub success: bool,
    /// List of phase names that completed successfully
    pub completed_phases: Vec<String>,
    /// Phase name where error occurred (if any)
    pub error_phase: Option<String>,
    /// Error message (if any)
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
///
/// Swift/Kotlin clients implement this trait to receive progress updates
/// during the initialization process.
#[uniffi::export(callback_interface)]
pub trait InitProgressHandlerFFI: Send + Sync {
    /// Called when a phase starts
    ///
    /// # Arguments
    /// - `phase`: Phase name (directories, config, embedding_model, database, runtimes, skills)
    /// - `current`: Current phase number (1-based)
    /// - `total`: Total number of phases
    fn on_phase_started(&self, phase: String, current: u32, total: u32);

    /// Called for progress updates within a phase
    ///
    /// # Arguments
    /// - `phase`: Phase name
    /// - `progress`: Progress value (0.0 to 1.0)
    /// - `message`: Status message describing current operation
    fn on_phase_progress(&self, phase: String, progress: f64, message: String);

    /// Called when a phase completes successfully
    ///
    /// # Arguments
    /// - `phase`: Phase name that completed
    fn on_phase_completed(&self, phase: String);

    /// Called for download progress updates (e.g., embedding model download)
    ///
    /// # Arguments
    /// - `item`: Item being downloaded (e.g., "bge-small-zh-v1.5")
    /// - `downloaded`: Bytes downloaded so far
    /// - `total`: Total bytes (0 if unknown)
    fn on_download_progress(&self, item: String, downloaded: u64, total: u64);

    /// Called when an error occurs
    ///
    /// # Arguments
    /// - `phase`: Phase where error occurred
    /// - `message`: Error message
    /// - `is_retryable`: Whether retry might succeed
    fn on_error(&self, phase: String, message: String, is_retryable: bool);
}

/// Adapter to convert FFI callback interface to internal trait
///
/// This adapter bridges the UniFFI callback interface with the internal
/// `InitProgressHandler` trait used by the coordinator.
struct ProgressHandlerAdapter {
    inner: Box<dyn InitProgressHandlerFFI>,
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
///
/// Returns true if any of the following conditions are met:
/// - Config directory (~/.config/aether) doesn't exist
/// - config.toml doesn't exist
/// - runtimes/manifest.json doesn't exist
///
/// This function is safe to call at any time and doesn't modify any files.
/// If an error occurs while checking, logs the error and returns true (safer default).
#[uniffi::export]
pub fn needs_first_time_init() -> bool {
    match crate::init_unified::needs_initialization() {
        Ok(needs_init) => needs_init,
        Err(e) => {
            // Log the error - defaulting to true is safer as it triggers initialization
            // which will properly report any underlying issues
            tracing::warn!(
                "Error checking initialization status, defaulting to needs_init=true: {}",
                e
            );
            true
        }
    }
}

/// Run first-time initialization with progress callback
///
/// This is a blocking function that runs the full initialization sequence:
/// 1. Create directory structure (~/.config/aether/*)
/// 2. Generate default config.toml
/// 3. Download embedding model (bge-small-zh-v1.5)
/// 4. Initialize memory database
/// 5. Install runtimes (ffmpeg, yt-dlp, uv, fnm) in parallel
/// 6. Set up skills directory
///
/// The handler receives progress updates throughout the process.
/// If any phase fails, completed phases are rolled back to ensure clean state.
///
/// # Arguments
/// - `handler`: Progress callback handler for UI updates
///
/// # Returns
/// `InitResultFFI` containing:
/// - `success`: true if all phases completed successfully
/// - `completed_phases`: list of successfully completed phase names
/// - `error_phase`: phase name where error occurred (if any)
/// - `error_message`: error description (if any)
#[uniffi::export]
pub fn run_initialization(handler: Box<dyn InitProgressHandlerFFI>) -> InitResultFFI {
    let adapter = Arc::new(ProgressHandlerAdapter { inner: handler });

    // Create tokio runtime for async operations
    // We create a new runtime here because this function may be called
    // before AetherCore is initialized (which creates its own runtime)
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            // Return error result instead of panicking
            tracing::error!("Failed to create tokio runtime: {}", e);
            return InitResultFFI {
                success: false,
                completed_phases: vec![],
                error_phase: Some("runtime_setup".to_string()),
                error_message: Some(format!("Failed to create async runtime: {}", e)),
            };
        }
    };

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct TestHandler {
        phases_started: AtomicU32,
        phases_completed: AtomicU32,
    }

    impl InitProgressHandlerFFI for TestHandler {
        fn on_phase_started(&self, _phase: String, _current: u32, _total: u32) {
            self.phases_started.fetch_add(1, Ordering::SeqCst);
        }

        fn on_phase_progress(&self, _phase: String, _progress: f64, _message: String) {}

        fn on_phase_completed(&self, _phase: String) {
            self.phases_completed.fetch_add(1, Ordering::SeqCst);
        }

        fn on_download_progress(&self, _item: String, _downloaded: u64, _total: u64) {}

        fn on_error(&self, _phase: String, _message: String, _is_retryable: bool) {}
    }

    #[test]
    fn test_needs_first_time_init_returns_bool() {
        // This test just verifies the function doesn't panic
        let _needs_init = needs_first_time_init();
    }
}
