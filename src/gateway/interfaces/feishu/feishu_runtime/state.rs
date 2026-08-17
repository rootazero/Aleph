use crate::sync_primitives::{AtomicUsize, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeState {
    Disconnected,
    Connecting,
    Connected,
    Error,
}

pub struct AtomicRuntimeState {
    inner: AtomicUsize,
}

impl AtomicRuntimeState {
    #[must_use]
    pub const fn new(initial: RuntimeState) -> Self {
        Self {
            inner: AtomicUsize::new(initial as usize),
        }
    }

    pub fn get(&self) -> RuntimeState {
        match self.inner.load(Ordering::SeqCst) {
            1 => RuntimeState::Connecting,
            2 => RuntimeState::Connected,
            3 => RuntimeState::Error,
            _ => RuntimeState::Disconnected,
        }
    }

    pub fn set(&self, state: RuntimeState) {
        self.inner.store(state as usize, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_transitions() {
        let state = AtomicRuntimeState::new(RuntimeState::Disconnected);
        assert_eq!(state.get(), RuntimeState::Disconnected);
        state.set(RuntimeState::Connecting);
        assert_eq!(state.get(), RuntimeState::Connecting);
        state.set(RuntimeState::Connected);
        assert_eq!(state.get(), RuntimeState::Connected);
    }
}
