//! Reusable Canvas terminal UI shared by the legacy Panel and the standalone tab.
//!
//! The crate owns terminal state and rendering. Hosts provide only local RPC and
//! exact-topic event services; no host context or transport object crosses this
//! boundary.

mod host;
mod keymap;
mod render;
mod session;
mod tabs;
mod view;

pub use host::{
    terminal_state, TerminalConnection, TerminalEvent, TerminalEventStream, TerminalServices,
    TerminalState, TerminalText,
};
pub use view::{TerminalContent, TerminalContentProps, TerminalSidebar};

#[cfg(test)]
mod tests {
    use super::TerminalEventStream;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn service_stream_disposal_is_idempotent() {
        let disposed = Rc::new(Cell::new(0));
        let count = Rc::clone(&disposed);
        let stream = TerminalEventStream::new(
            Rc::new(|| Ok(Vec::new())),
            Rc::new(move || count.set(count.get() + 1)),
        );
        (stream.dispose)();
        (stream.dispose)();
        assert_eq!(disposed.get(), 1);
    }
}
