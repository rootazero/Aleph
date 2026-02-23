//! macOS Accessibility API observer.
//!
//! This module provides event-driven access to UI state changes via the
//! macOS Accessibility API. It runs in a dedicated OS thread with a CFRunLoop
//! to avoid conflicts with tokio's async runtime.

use super::types::AxEvent;
use crate::error::{AlephError, Result};
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// AX Observer - listens for Accessibility API notifications.
pub struct AxObserver {
    event_tx: mpsc::UnboundedSender<AxEvent>,
}

impl AxObserver {
    /// Start the AX observer in a dedicated thread.
    ///
    /// Returns the observer handle and a receiver for AX events.
    /// The receiver should be consumed in a tokio task.
    pub fn start() -> Result<(Self, mpsc::UnboundedReceiver<AxEvent>)> {
        let (tx, rx) = mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        // Spawn dedicated OS thread for CFRunLoop
        std::thread::Builder::new()
            .name("ax-observer".to_string())
            .spawn(move || {
                if let Err(e) = run_ax_observer_loop(tx_clone) {
                    warn!("AX observer loop failed: {}", e);
                }
            })
            .map_err(|e| AlephError::tool(format!("Failed to spawn AX observer thread: {}", e)))?;

        Ok((Self { event_tx: tx }, rx))
    }
}

/// Run the AX observer loop (blocking, runs in dedicated thread).
fn run_ax_observer_loop(tx: mpsc::UnboundedSender<AxEvent>) -> Result<()> {
    debug!("Starting AX observer loop");

    // TODO: Implement actual AX API integration
    // For now, this is a stub that demonstrates the architecture

    // In the real implementation, this would:
    // 1. Get list of running applications
    // 2. Create AXObserverRef for each app
    // 3. Register for notifications (kAXValueChangedNotification, etc.)
    // 4. Start CFRunLoop
    //
    // Example (pseudo-code):
    // unsafe {
    //     let observer = AXObserverCreate(pid, ax_callback, &tx as *const _ as *mut _);
    //     AXObserverAddNotification(observer, element, kAXValueChangedNotification, ptr::null());
    //     CFRunLoopRun();  // Blocks until stopped
    // }

    warn!("AX observer loop is currently a stub - full implementation pending");

    // Keep thread alive (in real implementation, CFRunLoop would block here)
    std::thread::park();

    Ok(())
}

// Callback invoked by macOS AX API (runs on CFRunLoop thread)
//
// In the real implementation, this would be:
// extern "C" fn ax_callback(
//     observer: AXObserverRef,
//     element: AXUIElementRef,
//     notification: CFStringRef,
//     user_data: *mut c_void,
// ) {
//     let tx = unsafe { &*(user_data as *const mpsc::UnboundedSender<AxEvent>) };
//
//     let event = match notification_to_string(notification).as_str() {
//         "AXValueChanged" => {
//             let value = get_element_value(element);
//             AxEvent::ValueChanged {
//                 app_id: get_app_id(element),
//                 element_id: get_element_id(element),
//                 new_value: value,
//             }
//         }
//         "AXFocusedUIElementChanged" => {
//             AxEvent::FocusChanged {
//                 app_id: get_app_id(element),
//                 from: get_previous_focus(),
//                 to: get_element_id(element),
//             }
//         }
//         _ => return,
//     };
//
//     let _ = tx.send(event);
// }

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_ax_observer_start() {
        // Test that observer can be started without crashing
        let result = AxObserver::start();
        assert!(result.is_ok());

        let (_observer, mut rx) = result.unwrap();

        // In stub mode, no events will be received
        // In real implementation, we would test event reception
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            rx.recv()
        ).await.ok();
    }
}
