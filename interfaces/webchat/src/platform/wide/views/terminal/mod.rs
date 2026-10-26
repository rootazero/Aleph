//! Legacy adapter for the built-in terminal route.
//!
//! The reusable implementation lives in `aleph-terminal-ui`; this module only
//! adapts the legacy DashboardState to its narrow host services contract.

use crate::context::DashboardState;
use crate::i18n::{t_string, use_i18n};
use aleph_terminal_ui::{
    terminal_state, TerminalConnection, TerminalEvent, TerminalEventStream, TerminalServices,
    TerminalText,
};
use futures::FutureExt;
use leptos::prelude::*;
use serde_json::Value;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

#[component]
pub fn TerminalView() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let text = TerminalText {
        new_tab: t_string!(i18n, terminal.new_tab).to_string(),
        close_tab: t_string!(i18n, terminal.close_tab).to_string(),
        read_refusal: t_string!(i18n, settings.admin_refusal.read_config).to_string(),
        write_refusal: t_string!(i18n, settings.admin_refusal.write_config).to_string(),
    };
    let invoke = Rc::new(move |method: String, params: Value| {
        let dashboard = dashboard;
        async move { dashboard.rpc_call(&method, params).await }.boxed_local()
    });
    let subscribe = Rc::new(move |topic: String| {
        let dashboard = dashboard;
        async move {
            let pending = Arc::new(Mutex::new(Vec::<TerminalEvent>::new()));
            let pending_for_handler = Arc::clone(&pending);
            let expected = topic.clone();
            let id = dashboard.subscribe_events(move |event| {
                if event.topic == expected {
                    pending_for_handler.lock().unwrap().push(TerminalEvent {
                        topic: event.topic,
                        data: event.data,
                    });
                }
            });
            let pending_for_drain = Arc::clone(&pending);
            let drain = Rc::new(move || Ok(pending_for_drain.lock().unwrap().drain(..).collect()));
            let dispose = Rc::new(move || dashboard.unsubscribe_events(id));
            Ok(TerminalEventStream::new(drain, dispose))
        }
        .boxed_local()
    });
    let connection = Signal::derive(move || TerminalConnection {
        connected: dashboard.is_connected.get(),
        epoch: dashboard.connection_epoch.get(),
    });
    let state = terminal_state(TerminalServices {
        invoke,
        subscribe,
        connection,
        text,
    });
    aleph_terminal_ui::TerminalContent(aleph_terminal_ui::TerminalContentProps { state })
}
