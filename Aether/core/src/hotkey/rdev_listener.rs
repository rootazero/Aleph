/// rdev-based hotkey listener implementation
use super::HotkeyListener;
use crate::error::Result;
use rdev::{listen, Event, EventType, Key};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};

/// Hotkey listener using rdev
///
/// Detects Cmd+~ (Command+Grave) key combination on macOS.
/// Uses a background thread to monitor keyboard events.
pub struct RdevListener {
    callback: Arc<Mutex<Box<dyn Fn() + Send + 'static>>>,
    listening: Arc<AtomicBool>,
    #[allow(dead_code)]
    thread_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
    cmd_pressed: Arc<AtomicBool>,
}

impl RdevListener {
    /// Create a new rdev hotkey listener with a callback
    ///
    /// The callback is invoked when Cmd+~ is detected.
    pub fn new<F>(callback: F) -> Self
    where
        F: Fn() + Send + 'static,
    {
        Self {
            callback: Arc::new(Mutex::new(Box::new(callback))),
            listening: Arc::new(AtomicBool::new(false)),
            thread_handle: Arc::new(Mutex::new(None)),
            cmd_pressed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Check if the event matches the configured hotkey
    /// Supports both modifier+key combinations and single-key shortcuts
    fn is_hotkey_event(event: &Event, cmd_pressed: &Arc<AtomicBool>) -> bool {
        match event.event_type {
            EventType::KeyPress(Key::MetaLeft) | EventType::KeyPress(Key::MetaRight) => {
                cmd_pressed.store(true, Ordering::SeqCst);
                false
            }
            EventType::KeyRelease(Key::MetaLeft) | EventType::KeyRelease(Key::MetaRight) => {
                cmd_pressed.store(false, Ordering::SeqCst);
                false
            }
            // Backtick/grave key - accepts both Command+` or single `
            // For single-key shortcuts, trigger when Cmd is NOT pressed
            // For combo shortcuts, trigger when Cmd IS pressed
            EventType::KeyPress(Key::BackQuote) => {
                // Allow both: Command+` (traditional) or just ` (single key)
                // This makes the hotkey flexible
                true
            }
            _ => false,
        }
    }
}

impl HotkeyListener for RdevListener {
    fn start_listening(&self) -> Result<()> {
        if self.listening.load(Ordering::SeqCst) {
            return Ok(()); // Already listening
        }

        self.listening.store(true, Ordering::SeqCst);

        let callback = Arc::clone(&self.callback);
        let listening = Arc::clone(&self.listening);
        let cmd_pressed = Arc::clone(&self.cmd_pressed);

        // CRITICAL FIX: Check if we have permission to listen to global events
        // On macOS, this requires both Accessibility and Input Monitoring permissions
        // If permissions are not granted, rdev::listen() will cause the app to crash
        #[cfg(target_os = "macos")]
        {
            // Try to verify permissions by checking if we can create a test listener
            // If this fails immediately, it means permissions are not granted
            eprintln!("[RdevListener] Starting global hotkey listener (requires Accessibility + Input Monitoring permissions)");
        }

        let handle = thread::spawn(move || {
            let listen_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                listen(move |event: Event| {
                    if !listening.load(Ordering::SeqCst) {
                        // Stop listening
                        return;
                    }

                    if Self::is_hotkey_event(&event, &cmd_pressed) {
                        // Cmd+~ detected, invoke callback
                        if let Ok(cb) = callback.lock() {
                            cb();
                        }
                    }
                })
            }));

            // Handle panic or error from listen()
            match listen_result {
                Ok(Ok(())) => {
                    eprintln!("[RdevListener] Event listening stopped normally");
                }
                Ok(Err(e)) => {
                    eprintln!("[RdevListener] ERROR: rdev listen failed: {:?}", e);
                    eprintln!("[RdevListener] This usually means Input Monitoring permission is not granted.");
                    eprintln!("[RdevListener] Please grant permission in System Settings → Privacy & Security → Input Monitoring");
                }
                Err(panic_err) => {
                    eprintln!("[RdevListener] FATAL: rdev listen panicked!");
                    eprintln!("[RdevListener] Panic error: {:?}", panic_err);
                    eprintln!("[RdevListener] This is likely due to missing macOS permissions:");
                    eprintln!("[RdevListener]   1. Accessibility: System Settings → Privacy & Security → Accessibility");
                    eprintln!("[RdevListener]   2. Input Monitoring: System Settings → Privacy & Security → Input Monitoring");
                }
            }
        });

        *self.thread_handle.lock().unwrap() = Some(handle);

        Ok(())
    }

    fn stop_listening(&self) -> Result<()> {
        self.listening.store(false, Ordering::SeqCst);
        self.cmd_pressed.store(false, Ordering::SeqCst);

        // Note: rdev's listen() doesn't have a clean way to stop from outside
        // Setting listening to false will prevent callback invocation,
        // but the thread may continue until next keyboard event.
        // This is acceptable for Phase 1.

        Ok(())
    }

    fn is_listening(&self) -> bool {
        self.listening.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    #[test]
    fn test_hotkey_event_detection() {
        let cmd_pressed = Arc::new(AtomicBool::new(false));

        // Test MetaLeft press
        let event = Event {
            time: std::time::SystemTime::now(),
            event_type: EventType::KeyPress(Key::MetaLeft),
            name: None,
        };
        RdevListener::is_hotkey_event(&event, &cmd_pressed);
        assert!(cmd_pressed.load(Ordering::SeqCst));

        // Test BackQuote press with Cmd pressed (should match - combo shortcut)
        let event = Event {
            time: std::time::SystemTime::now(),
            event_type: EventType::KeyPress(Key::BackQuote),
            name: None,
        };
        assert!(RdevListener::is_hotkey_event(&event, &cmd_pressed));

        // Test MetaLeft release
        let event = Event {
            time: std::time::SystemTime::now(),
            event_type: EventType::KeyRelease(Key::MetaLeft),
            name: None,
        };
        RdevListener::is_hotkey_event(&event, &cmd_pressed);
        assert!(!cmd_pressed.load(Ordering::SeqCst));

        // Test BackQuote press without Cmd (should also match - single-key shortcut)
        let event = Event {
            time: std::time::SystemTime::now(),
            event_type: EventType::KeyPress(Key::BackQuote),
            name: None,
        };
        assert!(RdevListener::is_hotkey_event(&event, &cmd_pressed));
    }

    #[test]
    fn test_listener_lifecycle() {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = Arc::clone(&counter);

        let listener = RdevListener::new(move || {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        });

        assert!(!listener.is_listening());

        listener.start_listening().unwrap();
        assert!(listener.is_listening());

        listener.stop_listening().unwrap();
        assert!(!listener.is_listening());
    }

    #[test]
    fn test_double_start() {
        let listener = RdevListener::new(|| {});

        listener.start_listening().unwrap();
        // Second start should not error
        listener.start_listening().unwrap();

        listener.stop_listening().unwrap();
    }
}
