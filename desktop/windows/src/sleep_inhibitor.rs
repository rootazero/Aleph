//! Windows sleep inhibitor using `SetThreadExecutionState`.
//!
//! Uses a reference-counted approach: the first `inhibit_sleep` call sets
//! `ES_CONTINUOUS | ES_SYSTEM_REQUIRED`; the last released guard clears the flag.

use std::sync::{Arc, Mutex};

use aleph_desktop::{
    error::Result,
    traits::{InhibitorGuard, PowerCapability},
};

// ES_CONTINUOUS = 0x80000000, ES_SYSTEM_REQUIRED = 0x00000001
const ES_CONTINUOUS: u32 = 0x80000000;
const ES_SYSTEM_REQUIRED: u32 = 0x00000001;

#[link(name = "kernel32")]
extern "system" {
    fn SetThreadExecutionState(esFlags: u32) -> u32;
}

pub struct WindowsPower {
    count: Arc<Mutex<usize>>,
}

impl WindowsPower {
    pub fn new() -> Self {
        Self {
            count: Arc::new(Mutex::new(0)),
        }
    }
}

impl Default for WindowsPower {
    fn default() -> Self {
        Self::new()
    }
}

impl PowerCapability for WindowsPower {
    fn inhibit_sleep(&self, _reason: &str) -> Result<InhibitorGuard> {
        let count = Arc::clone(&self.count);
        {
            let mut g = count.lock().unwrap_or_else(|e| e.into_inner());
            *g = g.saturating_add(1);
            if *g == 1 {
                // SAFETY: kernel32 stdcall with documented constants.
                let _ = unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) };
            }
        }
        let count_for_release = Arc::clone(&count);
        Ok(InhibitorGuard::new(move || {
            let mut g = count_for_release.lock().unwrap_or_else(|e| e.into_inner());
            *g = g.saturating_sub(1);
            if *g == 0 {
                // SAFETY: kernel32 stdcall, clearing the previous assertion.
                let _ = unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
            }
        }))
    }
}
