//! First-Time Initialization Functions
//!
//! Check initialization status, register callbacks, and run first-time setup.

use std::ffi::{c_char, c_int, CString};
use std::sync::Arc;

use super::callback::{
    InitDownloadProgressCallback, InitErrorCallback, InitPhaseCompletedCallback,
    InitPhaseProgressCallback, InitPhaseStartedCallback, CALLBACKS,
};
use super::{AETHER_ERR_CONFIG, AETHER_ERR_UNKNOWN, AETHER_SUCCESS};

// =============================================================================
// First-Time Initialization Functions
// =============================================================================

/// Check if first-time initialization is needed
///
/// Returns:
/// * `1` if initialization is needed
/// * `0` if already initialized
#[no_mangle]
pub extern "C" fn aether_needs_first_time_init() -> c_int {
    match crate::init_unified::needs_initialization() {
        Ok(true) => 1,
        Ok(false) => 0,
        Err(e) => {
            tracing::warn!("Error checking initialization: {}, defaulting to needed", e);
            1
        }
    }
}

/// Check if the embedding model is installed
///
/// Returns:
/// * `1` if model exists
/// * `0` if model does not exist
#[no_mangle]
pub extern "C" fn aether_check_embedding_model_exists() -> c_int {
    use crate::memory::EmbeddingModel;

    match EmbeddingModel::get_default_model_path() {
        Ok(cache_dir) => {
            let model_dir = cache_dir.join("models--BAAI--bge-small-zh-v1.5");
            if !model_dir.exists() {
                return 0;
            }

            let snapshots_dir = model_dir.join("snapshots");
            if !snapshots_dir.exists() {
                return 0;
            }

            if let Ok(entries) = std::fs::read_dir(&snapshots_dir) {
                for entry in entries.flatten() {
                    let snapshot_path = entry.path();
                    if snapshot_path.is_dir() {
                        let model_onnx = snapshot_path.join("model.onnx");
                        let tokenizer_json = snapshot_path.join("tokenizer.json");
                        if model_onnx.exists() && tokenizer_json.exists() {
                            return 1;
                        }
                    }
                }
            }
            0
        }
        Err(_) => 0,
    }
}

/// Register initialization progress callbacks
///
/// # Safety
/// All callback function pointers must be valid for the duration of initialization.
#[no_mangle]
pub unsafe extern "C" fn aether_register_init_callbacks(
    on_phase_started: InitPhaseStartedCallback,
    on_phase_progress: InitPhaseProgressCallback,
    on_phase_completed: InitPhaseCompletedCallback,
    on_download_progress: InitDownloadProgressCallback,
    on_error: InitErrorCallback,
) -> c_int {
    if let Ok(mut cbs) = CALLBACKS.lock() {
        cbs.init_phase_started = Some(on_phase_started);
        cbs.init_phase_progress = Some(on_phase_progress);
        cbs.init_phase_completed = Some(on_phase_completed);
        cbs.init_download_progress = Some(on_download_progress);
        cbs.init_error = Some(on_error);
        AETHER_SUCCESS
    } else {
        AETHER_ERR_UNKNOWN
    }
}

/// Clear initialization callbacks
#[no_mangle]
pub extern "C" fn aether_clear_init_callbacks() {
    if let Ok(mut cbs) = CALLBACKS.lock() {
        cbs.init_phase_started = None;
        cbs.init_phase_progress = None;
        cbs.init_phase_completed = None;
        cbs.init_download_progress = None;
        cbs.init_error = None;
    }
}

/// Adapter to bridge C ABI callbacks to InitProgressHandler trait
struct CAbiProgressHandler;

impl crate::init_unified::InitProgressHandler for CAbiProgressHandler {
    fn on_phase_started(&self, phase: String, current: u32, total: u32) {
        if let Ok(cbs) = CALLBACKS.lock() {
            if let Some(cb) = cbs.init_phase_started {
                if let Ok(phase_cstr) = CString::new(phase) {
                    cb(phase_cstr.as_ptr(), current, total);
                }
            }
        }
    }

    fn on_phase_progress(&self, phase: String, progress: f64, message: String) {
        if let Ok(cbs) = CALLBACKS.lock() {
            if let Some(cb) = cbs.init_phase_progress {
                if let (Ok(phase_cstr), Ok(msg_cstr)) = (CString::new(phase), CString::new(message))
                {
                    cb(phase_cstr.as_ptr(), progress, msg_cstr.as_ptr());
                }
            }
        }
    }

    fn on_phase_completed(&self, phase: String) {
        if let Ok(cbs) = CALLBACKS.lock() {
            if let Some(cb) = cbs.init_phase_completed {
                if let Ok(phase_cstr) = CString::new(phase) {
                    cb(phase_cstr.as_ptr());
                }
            }
        }
    }

    fn on_download_progress(&self, item: String, downloaded: u64, total: u64) {
        if let Ok(cbs) = CALLBACKS.lock() {
            if let Some(cb) = cbs.init_download_progress {
                if let Ok(item_cstr) = CString::new(item) {
                    cb(item_cstr.as_ptr(), downloaded, total);
                }
            }
        }
    }

    fn on_error(&self, phase: String, message: String, is_retryable: bool) {
        if let Ok(cbs) = CALLBACKS.lock() {
            if let Some(cb) = cbs.init_error {
                if let (Ok(phase_cstr), Ok(msg_cstr)) = (CString::new(phase), CString::new(message))
                {
                    cb(
                        phase_cstr.as_ptr(),
                        msg_cstr.as_ptr(),
                        if is_retryable { 1 } else { 0 },
                    );
                }
            }
        }
    }
}

/// Run first-time initialization
///
/// This is a blocking function that runs the full 6-phase initialization.
/// Progress is reported via registered callbacks.
///
/// Returns:
/// * `0` on success
/// * Negative error code on failure
#[no_mangle]
pub extern "C" fn aether_run_first_time_init() -> c_int {
    use crate::init_unified::InitializationCoordinator;

    // Create tokio runtime for async operations
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            tracing::error!("Failed to create tokio runtime: {}", e);
            return AETHER_ERR_UNKNOWN;
        }
    };

    let handler = Arc::new(CAbiProgressHandler);

    let result = rt.block_on(async {
        match InitializationCoordinator::new(Some(handler)) {
            Ok(coordinator) => coordinator.run().await,
            Err(e) => crate::init_unified::InitializationResult {
                success: false,
                completed_phases: vec![],
                error_phase: Some(e.phase),
                error_message: Some(e.message),
            },
        }
    });

    if result.success {
        AETHER_SUCCESS
    } else {
        tracing::error!(
            "Initialization failed at phase {:?}: {:?}",
            result.error_phase,
            result.error_message
        );
        AETHER_ERR_CONFIG
    }
}
