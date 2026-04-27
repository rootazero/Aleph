//! Power-management capability for preventing system idle sleep.

use crate::error::Result;

pub trait PowerCapability: Send + Sync {
    /// Prevent system idle sleep while the returned guard is alive.
    /// `reason` appears in macOS `pmset -g assertions`.
    fn inhibit_sleep(&self, reason: &str) -> Result<InhibitorGuard>;
}

/// RAII guard that releases the underlying platform-specific assertion when
/// dropped. Use [`InhibitorGuard::noop`] for platforms that cannot inhibit
/// sleep but want to return `Ok` to keep callers branch-free.
pub struct InhibitorGuard {
    release: Option<Box<dyn FnOnce() + Send + 'static>>,
}

impl InhibitorGuard {
    pub fn new<F: FnOnce() + Send + 'static>(release: F) -> Self {
        Self {
            release: Some(Box::new(release)),
        }
    }

    pub fn noop() -> Self {
        Self { release: None }
    }
}

impl Drop for InhibitorGuard {
    fn drop(&mut self) {
        if let Some(f) = self.release.take() {
            f();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    #[test]
    fn guard_drop_calls_release() {
        let released = Arc::new(AtomicBool::new(false));
        let flag = released.clone();
        let g = InhibitorGuard::new(move || flag.store(true, Ordering::SeqCst));
        drop(g);
        assert!(released.load(Ordering::SeqCst));
    }

    #[test]
    fn noop_guard_is_safe_to_drop() {
        drop(InhibitorGuard::noop());
    }
}
