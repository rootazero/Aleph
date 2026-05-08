use std::sync::atomic::{AtomicBool, Ordering};

use aleph_desktop::platform::EscapeAbort;
use aleph_desktop::Result;

pub struct LinuxEscapeListener {
    aborted: AtomicBool,
}

impl LinuxEscapeListener {
    pub fn new() -> Self {
        Self {
            aborted: AtomicBool::new(false),
        }
    }
}

impl Default for LinuxEscapeListener {
    fn default() -> Self {
        Self::new()
    }
}

impl EscapeAbort for LinuxEscapeListener {
    fn start(&self) -> Result<()> {
        Ok(())
    }

    fn stop(&self) {}

    fn is_aborted(&self) -> bool {
        self.aborted.load(Ordering::Acquire)
    }

    fn reset(&self) {
        self.aborted.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle() {
        let listener = LinuxEscapeListener::new();
        listener.start().unwrap();
        assert!(!listener.is_aborted());
        listener.reset();
        listener.stop();
    }
}
