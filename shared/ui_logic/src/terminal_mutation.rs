//! Payload-free, scope-owned terminal mutation admission and invalidation.
use std::sync::Mutex;

pub trait TerminalMutationScope {
    fn revision(&self) -> u64;
    fn pending(&self) -> bool;
    fn reserve(&self) -> Option<u64>;
    fn cancel(&self, ticket: u64);
    fn settle(&self, ticket: u64);
}

#[derive(Default)]
pub struct TerminalMutationCoordinator(Mutex<State>);

#[derive(Default)]
struct State {
    serial: u64,
    pending: Option<u64>,
    revision: u64,
}

impl TerminalMutationScope for TerminalMutationCoordinator {
    fn revision(&self) -> u64 {
        self.0.lock().unwrap().revision
    }
    fn pending(&self) -> bool {
        self.0.lock().unwrap().pending.is_some()
    }
    fn reserve(&self) -> Option<u64> {
        let mut state = self.0.lock().unwrap();
        if state.pending.is_some() {
            return None;
        }
        state.serial = state.serial.checked_add(1)?;
        let ticket = state.serial;
        state.pending = Some(ticket);
        Some(ticket)
    }
    fn cancel(&self, ticket: u64) {
        let mut state = self.0.lock().unwrap();
        if state.pending == Some(ticket) {
            state.pending = None;
        }
    }
    fn settle(&self, ticket: u64) {
        let mut state = self.0.lock().unwrap();
        if state.pending == Some(ticket) {
            state.pending = None;
            state.revision = state.revision.wrapping_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reservation_cancellation_and_settlement_are_ticketed_and_scope_local() {
        let scope = TerminalMutationCoordinator::default();
        let other = TerminalMutationCoordinator::default();
        let unsent = scope.reserve().unwrap();
        assert!(scope.reserve().is_none());
        assert!(!other.pending());
        scope.cancel(unsent);
        assert_eq!(scope.revision(), 0);
        let sent = scope.reserve().unwrap();
        scope.settle(unsent);
        assert!(scope.pending());
        scope.settle(sent);
        scope.settle(sent);
        assert!(!scope.pending());
        assert_eq!(scope.revision(), 1);
        assert_eq!(other.revision(), 0);
    }
}
