//! Legacy adapter for the built-in terminal route.
//!
//! The reusable implementation lives in `aleph-terminal-ui`; this module only
//! adapts the legacy DashboardState to its narrow host services contract.

use leptos::prelude::*;

#[cfg(feature = "legacy-terminal")]
pub use aleph_terminal_ui::TerminalSelection;
#[cfg(not(feature = "legacy-terminal"))]
pub type TerminalSelection = ();

#[cfg(feature = "legacy-terminal")]
mod legacy {
    use super::*;
    use crate::context::{DashboardState, TerminalEventQueue};
    use crate::i18n::{t_string, use_i18n};
    use aleph_terminal_ui::{
        terminal_state_with_selection, TerminalConnection, TerminalEvent, TerminalEventStream,
        TerminalServices, TerminalText,
    };
    use futures::FutureExt;
    use serde_json::Value;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    struct TerminalListener {
        dashboard: DashboardState,
        id: usize,
        disposed: AtomicBool,
        pending: Arc<Mutex<TerminalEventQueue>>,
        lease: Mutex<Option<crate::context::PtyTopicLease>>,
    }

    impl TerminalListener {
        fn dispose(&self) {
            if self.disposed.swap(true, Ordering::AcqRel) {
                return;
            }
            if self.dashboard.is_connected.try_get_untracked().is_some() {
                self.dashboard.unsubscribe_events(self.id);
            }
            self.pending.lock().unwrap().clear();
            self.lease.lock().unwrap().take();
        }
    }

    impl Drop for TerminalListener {
        fn drop(&mut self) {
            self.dispose();
        }
    }

    #[derive(Default)]
    struct TerminalListeners {
        disposed: AtomicBool,
        listeners: Mutex<Vec<std::sync::Weak<TerminalListener>>>,
    }

    impl TerminalListeners {
        fn new() -> Arc<Self> {
            let listeners = Arc::new(Self::default());
            let cleanup = listeners.clone();
            on_cleanup(move || {
                cleanup.disposed.store(true, Ordering::Release);
                for listener in cleanup
                    .listeners
                    .lock()
                    .unwrap()
                    .drain(..)
                    .filter_map(|weak| weak.upgrade())
                {
                    listener.dispose();
                }
            });
            listeners
        }
    }

    async fn subscribe_terminal_with(
        dashboard: DashboardState,
        topic: String,
        listeners: Arc<TerminalListeners>,
        acquire: impl FnOnce(
            u64,
        ) -> futures::future::LocalBoxFuture<
            'static,
            Result<crate::context::PtyTopicLease, String>,
        >,
    ) -> Result<TerminalEventStream, String> {
        if listeners.disposed.load(Ordering::Acquire) {
            return Err("Terminal owner disposed".into());
        }
        let coverage_epoch = dashboard
            .connection_epoch
            .try_get_untracked()
            .ok_or("Panel disposed")?;
        dashboard.check_pty_epoch(coverage_epoch)?;
        let pending = Arc::new(Mutex::new(TerminalEventQueue::default()));
        let pending_for_handler = Arc::clone(&pending);
        let expected = topic.clone();
        let id = dashboard.subscribe_events(move |event| {
            if event.topic == expected && dashboard.check_pty_epoch(coverage_epoch).is_ok() {
                pending_for_handler.lock().unwrap().enqueue(event);
            }
        });
        let listener = Arc::new(TerminalListener {
            dashboard,
            id,
            disposed: AtomicBool::new(false),
            pending: pending.clone(),
            lease: Mutex::new(None),
        });
        listeners
            .listeners
            .lock()
            .unwrap()
            .retain(|weak| weak.strong_count() > 0);
        listeners
            .listeners
            .lock()
            .unwrap()
            .push(Arc::downgrade(&listener));
        let lease = acquire(coverage_epoch).await?;
        dashboard.check_pty_epoch(coverage_epoch)?;
        if lease.epoch() != coverage_epoch {
            return Err("Terminal subscription epoch changed".into());
        }
        if listener.disposed.load(Ordering::Acquire) {
            return Err("Terminal owner disposed".into());
        }
        *listener.lease.lock().unwrap() = Some(lease);
        let pending_for_drain = Arc::clone(&pending);
        let listener_for_drain = Arc::clone(&listener);
        let drain = Rc::new(move || {
            let mut pending = pending_for_drain.lock().unwrap();
            if listener_for_drain.lease.lock().unwrap().is_none()
                || dashboard.connection_epoch.try_get_untracked() != Some(coverage_epoch)
                || dashboard.is_connected.try_get_untracked() != Some(true)
            {
                pending.clear();
                return Err("Terminal subscription epoch changed or disposed".into());
            }
            pending.drain().map(|events| {
                events
                    .into_iter()
                    .map(|event| TerminalEvent {
                        topic: event.topic,
                        data: event.data,
                    })
                    .collect()
            })
        });
        let dispose = Rc::new(move || listener.dispose());
        Ok(TerminalEventStream::new(coverage_epoch, drain, dispose))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::context::{tests::PtyHarness, GatewayEvent};
        use futures::executor::block_on;

        #[test]
        fn terminal_coverage_legacy_collects_callback_frames_before_ack_for_each_holder() {
            Owner::new().with(|| {
                let dashboard = DashboardState::new();
                dashboard.is_connected.set(true);
                dashboard.connection_epoch.set(1);
                let (ack, wait) = futures::channel::oneshot::channel();
                let wait = Arc::new(Mutex::new(Some(wait)));
                let host = PtyHarness::with_rpc(dashboard, move |_, _| {
                    let wait = wait.lock().unwrap().take();
                    async move {
                        if let Some(wait) = wait {
                            wait.await.unwrap();
                        }
                        Ok(Value::Null)
                    }
                    .boxed_local()
                });
                let listeners = TerminalListeners::new();
                let subscribe = || {
                    let io = host.io.clone();
                    subscribe_terminal_with(
                        dashboard,
                        "pty.screen".into(),
                        listeners.clone(),
                        move |epoch| {
                            async move {
                                dashboard
                                    .acquire_pty_topic_at_epoch_with_io(
                                        "pty.screen",
                                        epoch,
                                        io,
                                        async { Ok(()) },
                                    )
                                    .await
                            }
                            .boxed_local()
                        },
                    )
                    .boxed_local()
                };
                let mut first = subscribe();
                let mut second = subscribe();
                assert!(first.as_mut().now_or_never().is_none());
                assert!(second.as_mut().now_or_never().is_none());
                let mut rpc = host.next();
                assert!(rpc.as_mut().now_or_never().is_none());
                dashboard.dispatch_event(GatewayEvent {
                    topic: "pty.screen".into(),
                    data: serde_json::json!({"seq": 1}),
                });
                ack.send(()).unwrap();
                block_on(rpc).unwrap();
                let first = block_on(first).unwrap();
                assert_eq!(host.flush(), [Ok(())]);
                let second = block_on(second).unwrap();
                for stream in [&first, &second] {
                    let events = (stream.drain)().unwrap();
                    assert_eq!(events.len(), 1);
                    assert_eq!(events[0].data, serde_json::json!({"seq": 1}));
                }
                (first.dispose)();
                dashboard.connection_epoch.set(2);
                dashboard.dispatch_event(GatewayEvent {
                    topic: "pty.screen".into(),
                    data: Value::Null,
                });
                assert!((second.drain)().is_err());
                (second.dispose)();
                assert_eq!(host.flush(), [Ok(())]);
                assert_eq!(host.methods(), ["events.subscribe"]);
            });
        }

        #[test]
        fn terminal_coverage_legacy_pending_cleanup_releases_listener_and_lease() {
            for cancel in [false, true] {
                let owner = Owner::new();
                owner.with(|| {
                    let dashboard = DashboardState::new();
                    dashboard.is_connected.set(true);
                    let host = PtyHarness::new(dashboard);
                    let route_owner = Owner::new();
                    let listeners = route_owner.with(TerminalListeners::new);
                    let io = host.io.clone();
                    let mut pending = subscribe_terminal_with(
                        dashboard,
                        "pty.screen".into(),
                        listeners,
                        move |epoch| {
                            async move {
                                dashboard
                                    .acquire_pty_topic_at_epoch_with_io(
                                        "pty.screen",
                                        epoch,
                                        io,
                                        async { Ok(()) },
                                    )
                                    .await
                            }
                            .boxed_local()
                        },
                    )
                    .boxed_local();
                    assert!(pending.as_mut().now_or_never().is_none());
                    if cancel {
                        drop(pending);
                        assert_eq!(host.flush(), [Ok(()), Ok(())]);
                    } else {
                        route_owner.cleanup();
                        assert_eq!(host.flush(), [Ok(())]);
                        assert!(block_on(pending).is_err());
                        assert_eq!(host.flush(), [Ok(())]);
                    }
                    assert_eq!(dashboard.subscribe_events(|_| {}), 0);
                    assert_eq!(host.methods(), ["events.subscribe", "events.unsubscribe"]);
                });
                owner.cleanup();
            }
        }
    }

    #[component]
    pub fn TerminalView(selection: TerminalSelection) -> impl IntoView {
        let dashboard = expect_context::<DashboardState>();
        let i18n = use_i18n();
        let text = TerminalText {
            new_tab: t_string!(i18n, terminal.new_tab).to_string(),
            close_tab: t_string!(i18n, terminal.close_tab).to_string(),
            reconnecting: t_string!(i18n, terminal.reconnecting).to_string(),
            retry: t_string!(i18n, terminal.retry).to_string(),
            read_refusal: t_string!(i18n, settings.admin_refusal.read_config).to_string(),
            write_refusal: t_string!(i18n, settings.admin_refusal.write_config).to_string(),
        };
        let invoke = Rc::new(move |method: String, params: Value| {
            let dashboard = dashboard;
            async move { dashboard.rpc_call(&method, params).await }.boxed_local()
        });
        let listeners = TerminalListeners::new();
        let subscribe = Rc::new(move |topic: String| {
            let listeners = listeners.clone();
            let dashboard = dashboard;
            subscribe_terminal_with(dashboard, topic.clone(), listeners, move |epoch| {
                async move { dashboard.acquire_pty_topic_at_epoch(&topic, epoch).await }
                    .boxed_local()
            })
            .boxed_local()
        });
        let connection = Signal::derive(move || TerminalConnection {
            connected: dashboard.is_connected.get(),
            epoch: dashboard.connection_epoch.get(),
        });
        let state = terminal_state_with_selection(
            TerminalServices { invoke, subscribe, connection, text },
            selection,
        );
        aleph_terminal_ui::TerminalContent(aleph_terminal_ui::TerminalContentProps { state })
    }
}

#[cfg(feature = "legacy-terminal")]
pub use legacy::TerminalView;

#[cfg(not(feature = "legacy-terminal"))]
#[component]
pub fn TerminalView(selection: TerminalSelection) -> impl IntoView {
    let _ = selection;
    ().into_view()
}
