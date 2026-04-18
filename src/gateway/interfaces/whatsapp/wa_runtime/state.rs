use crate::sync_primitives::{AtomicUsize, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Pairing,
    Connecting,
    Connected,
    Error,
}

pub struct AtomicConnectionState {
    inner: AtomicUsize,
}

impl AtomicConnectionState {
    pub fn new(initial: ConnectionState) -> Self {
        Self {
            inner: AtomicUsize::new(initial as usize),
        }
    }

    pub fn get(&self) -> ConnectionState {
        match self.inner.load(Ordering::SeqCst) {
            1 => ConnectionState::Pairing,
            2 => ConnectionState::Connecting,
            3 => ConnectionState::Connected,
            4 => ConnectionState::Error,
            _ => ConnectionState::Disconnected,
        }
    }

    pub fn set(&self, state: ConnectionState) {
        self.inner.store(state as usize, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_transitions() {
        let state = AtomicConnectionState::new(ConnectionState::Disconnected);
        assert_eq!(state.get(), ConnectionState::Disconnected);
        state.set(ConnectionState::Pairing);
        assert_eq!(state.get(), ConnectionState::Pairing);
        state.set(ConnectionState::Connecting);
        assert_eq!(state.get(), ConnectionState::Connecting);
        state.set(ConnectionState::Connected);
        assert_eq!(state.get(), ConnectionState::Connected);
    }
}
