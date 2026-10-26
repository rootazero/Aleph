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
    use super::{TerminalConnection, TerminalEvent, TerminalEventStream, TerminalServices};
    use futures::FutureExt;
    use leptos::prelude::Signal;
    use serde_json::json;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    #[test]
    fn fake_service_subscription_drains_events_and_disposes_once() {
        let disposed = Rc::new(Cell::new(0));
        let emitted = Rc::new(RefCell::new(vec![TerminalEvent {
            topic: "pty.screen".to_string(),
            data: json!({"session_id": "s1"}),
        }]));
        let subscribe_disposed = Rc::clone(&disposed);
        let subscribe_emitted = Rc::clone(&emitted);
        let subscribe = Rc::new(move |_topic: String| {
            let disposed = Rc::clone(&subscribe_disposed);
            let emitted = Rc::clone(&subscribe_emitted);
            async move {
                let drain = Rc::new(move || Ok(emitted.borrow_mut().drain(..).collect()));
                let dispose = Rc::new(move || disposed.set(disposed.get() + 1));
                Ok(TerminalEventStream::new(1, drain, dispose))
            }
            .boxed_local()
        });
        let services = TerminalServices {
            invoke: Rc::new(|_, _| async { Ok(json!({})) }.boxed_local()),
            subscribe,
            connection: Signal::derive(|| TerminalConnection {
                connected: true,
                epoch: 1,
            }),
            text: super::TerminalText {
                new_tab: "new".to_string(),
                close_tab: "close".to_string(),
                read_refusal: "read".to_string(),
                write_refusal: "write".to_string(),
            },
        };
        let stream = futures::executor::block_on((services.subscribe)("pty.screen".to_string()))
            .expect("fake host subscription");
        assert_eq!((stream.drain)().unwrap().len(), 1);
        (stream.dispose)();
        (stream.dispose)();
        assert_eq!(disposed.get(), 1);
    }

    #[test]
    fn fake_service_list_failure_is_not_a_spawn_request() {
        let calls = Rc::new(RefCell::new(Vec::<String>::new()));
        let seen = Rc::clone(&calls);
        let invoke = Rc::new(move |method: String, _params| {
            seen.borrow_mut().push(method.clone());
            async move { Err::<serde_json::Value, _>("connection reset".to_string()) }.boxed_local()
        });
        let result = futures::executor::block_on(invoke("pty.list".to_string(), json!({})));
        let outcome = crate::session::resolve_session_list(result);
        assert!(matches!(outcome, crate::session::ListOutcome::Fail(_)));
        assert_eq!(calls.borrow().as_slice(), ["pty.list"]);
        assert!(!calls.borrow().iter().any(|method| method == "pty.spawn"));
    }
}
