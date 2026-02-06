//! Tauri event handler
//!
//! In Gateway mode, events are handled directly in handle_gateway_event().
//! This module is kept for backward compatibility but is no longer used.

use tauri::{AppHandle, Runtime};

/// Tauri event handler (deprecated in Gateway mode)
///
/// Events are now handled directly via handle_gateway_event() in mod.rs.
#[allow(dead_code)]
pub struct TauriEventHandler<R: Runtime> {
    app: AppHandle<R>,
}

#[allow(dead_code)]
impl<R: Runtime> TauriEventHandler<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        Self { app }
    }
}
