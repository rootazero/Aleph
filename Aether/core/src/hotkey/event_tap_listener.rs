/// CGEventTap-based hotkey listener for single-key global hotkeys
///
/// This implementation uses macOS CGEventTap API to INTERCEPT keyboard events,
/// allowing us to prevent the default behavior (character input) when the hotkey is pressed.
///
/// Key differences from rdev_listener:
/// - rdev: passive monitoring, events still propagate to system (cannot prevent input)
/// - CGEventTap: active interception, can swallow events to prevent default behavior
///
/// This is essential for single-key hotkeys like ` (backtick) which would otherwise
/// type the character in the active application.
use super::HotkeyListener;
use crate::error::Result;
use core_foundation::runloop::CFRunLoop;
use core_graphics::event::{CGEvent, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventTapProxy, CGEventType, EventField};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};

/// CGEventTap-based hotkey listener
///
/// Uses macOS CGEventTap API to intercept keyboard events globally.
/// When the configured hotkey (` key) is detected, it swallows the event
/// to prevent character input.
pub struct EventTapListener {
    callback: Arc<Mutex<Box<dyn Fn() + Send + 'static>>>,
    listening: Arc<AtomicBool>,
    #[allow(dead_code)]
    thread_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl EventTapListener {
    /// Create a new event tap hotkey listener
    ///
    /// The callback is invoked when ` (grave) key is detected.
    /// The key event is swallowed to prevent character input.
    pub fn new<F>(callback: F) -> Self
    where
        F: Fn() + Send + 'static,
    {
        Self {
            callback: Arc::new(Mutex::new(Box::new(callback))),
            listening: Arc::new(AtomicBool::new(false)),
            thread_handle: Arc::new(Mutex::new(None)),
        }
    }
}

impl HotkeyListener for EventTapListener {
    fn start_listening(&self) -> Result<()> {
        if self.listening.load(Ordering::SeqCst) {
            return Ok(()); // Already listening
        }

        self.listening.store(true, Ordering::SeqCst);

        let callback = Arc::clone(&self.callback);
        let listening = Arc::clone(&self.listening);

        eprintln!("[EventTapListener] Starting CGEventTap for single-key hotkey interception");

        // IMPORTANT: CGEventTap must run on a thread with a run loop
        let handle = thread::spawn(move || {
            // Create event tap callback
            // This closure will be called for EVERY keyboard event
            let event_callback = move |_proxy: CGEventTapProxy,
                                        event_type: CGEventType,
                                        event: &CGEvent|
                  -> Option<CGEvent> {
                // Only process key down events
                match event_type {
                    CGEventType::KeyDown => {
                        // Get the key code
                        let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);

                        // macOS keycode for ` (grave/backtick) is 50
                        const KEYCODE_GRAVE: i64 = 50;

                        if keycode == KEYCODE_GRAVE {
                            eprintln!("[EventTapListener] Detected ` key press - triggering Aether");

                            // Trigger Aether callback
                            if let Ok(cb) = callback.lock() {
                                cb();
                            }

                            // CRITICAL: Return None to SWALLOW the event
                            // This prevents the ` character from being typed
                            return None;
                        }

                        // For all other keys, allow the event to propagate
                        Some(event.to_owned())
                    }
                    _ => {
                        // For all non-KeyDown events, allow propagation
                        Some(event.to_owned())
                    }
                }
            };

            // Create the event tap
            // kCGEventTapOptionDefault = intercept and allow modification/deletion of events
            match CGEventTap::new(
                CGEventTapLocation::HID,                       // Hardware event tap (before app)
                CGEventTapPlacement::HeadInsertEventTap,       // Insert at head of queue
                CGEventTapOptions::Default,                    // Intercept mode (not listen-only)
                vec![CGEventType::KeyDown, CGEventType::KeyUp], // Monitor key events
                event_callback,
            ) {
                Ok(tap) => {
                    eprintln!("[EventTapListener] CGEventTap created successfully");

                    // Enable the tap
                    tap.enable();

                    // Create a run loop source from the tap
                    let source = tap
                        .mach_port
                        .create_runloop_source(0)
                        .expect("Failed to create run loop source");

                    // Add source to current run loop
                    let run_loop = CFRunLoop::get_current();
                    run_loop.add_source(&source, unsafe {
                        core_foundation::runloop::kCFRunLoopCommonModes
                    });

                    eprintln!("[EventTapListener] Starting run loop");

                    // Run the event loop
                    // This blocks until stop_listening() is called
                    while listening.load(Ordering::SeqCst) {
                        CFRunLoop::run_in_mode(
                            unsafe { core_foundation::runloop::kCFRunLoopDefaultMode },
                            std::time::Duration::from_millis(100),
                            true,
                        );
                    }

                    eprintln!("[EventTapListener] Run loop stopped");
                }
                Err(e) => {
                    eprintln!("[EventTapListener] ERROR: Failed to create CGEventTap: {:?}", e);
                    eprintln!("[EventTapListener] This usually means Accessibility permission is not granted");
                }
            }
        });

        // Store thread handle
        *self.thread_handle.lock().unwrap() = Some(handle);

        Ok(())
    }

    fn stop_listening(&self) -> Result<()> {
        if !self.listening.load(Ordering::SeqCst) {
            return Ok(()); // Not listening
        }

        eprintln!("[EventTapListener] Stopping CGEventTap listener");
        self.listening.store(false, Ordering::SeqCst);

        // Wait for thread to finish
        if let Some(handle) = self.thread_handle.lock().unwrap().take() {
            let _ = handle.join();
        }

        Ok(())
    }

    fn is_listening(&self) -> bool {
        self.listening.load(Ordering::SeqCst)
    }
}
