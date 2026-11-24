//! Opaque host adapter. Browser modules receive only the narrow SDK ABI, never
//! this adapter's private DashboardState or Shell context. Same-origin code is
//! trusted/verified code, not sandboxed by these permission checks.

use super::PanelHostState;
use crate::appearance::{Accent, ThemeMode};
use crate::context::{DashboardState, GatewayEvent, PtyTopicLease, TerminalEventQueue};
use aleph_protocol::panel::{PanelTabDescriptor, PanelTabKey, PanelTabStatus};
use futures::future::LocalBoxFuture;
use futures::FutureExt;
#[cfg(any(target_arch = "wasm32", test))]
use leptos::prelude::Update;
use leptos::prelude::{on_cleanup, use_context, Effect, Get, GetUntracked, Set, WithUntracked, WithValue};
use leptos::task::spawn_local;
use serde_json::Value;
use std::collections::VecDeque;
use shared_ui_logic::terminal_mutation::{TerminalMutationCoordinator, TerminalMutationScope};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PanelSdkError {
    Denied,
    InvalidMethod,
    InvalidNavigation,
    Disposed,
}

impl fmt::Display for PanelSdkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for PanelSdkError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PanelConnectionSnapshot {
    pub connected: bool,
    pub epoch: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PanelThemeSnapshot {
    pub mode: ThemeMode,
    pub accent: Accent,
}

type HostRpc =
    dyn Fn(String, Value) -> LocalBoxFuture<'static, Result<Value, String>> + Send + Sync;
type HostPtySubscribe = dyn Fn(String, Option<u64>) -> LocalBoxFuture<'static, Result<PtyTopicLease, String>>
    + Send
    + Sync;

/// Private host operations only; no public transport or context accessor.
#[derive(Clone)]
pub struct PanelSdk {
    state: DashboardState,
    host: Option<PanelHostState>,
    tab: PanelTabKey,
    rpc: Arc<HostRpc>,
    pty_subscribe: Arc<HostPtySubscribe>,
    lifecycle: Arc<SdkLifecycle>,
    locale: String,
}

#[derive(Default)]
struct SdkLifecycle {
    disposed: AtomicBool,
    subscriptions: Mutex<Vec<Weak<SubscriptionState>>>,
}

impl SdkLifecycle {
    fn dispose(&self) {
        if self.disposed.swap(true, Ordering::SeqCst) {
            return;
        }
        let subscriptions = std::mem::take(&mut *self.subscriptions.lock().unwrap());
        for subscription in subscriptions
            .into_iter()
            .filter_map(|entry| entry.upgrade())
        {
            subscription.dispose();
        }
    }
}

/// Narrow builtin-only handle. Releasing an issued ticket is independent of
/// admission: disposal must not strand an RPC which already reached Gateway.
pub(super) struct PanelTerminalMutations {
    sdk: PanelSdk,
    context: (u64, u64),
    coordinator: Arc<TerminalMutationCoordinator>,
    ticket: Mutex<Option<u64>>,
}

impl TerminalMutationScope for PanelTerminalMutations {
    fn revision(&self) -> u64 {
        self.coordinator.revision()
    }
    fn pending(&self) -> bool {
        self.coordinator.pending()
    }
    fn reserve(&self) -> Option<u64> {
        self.sdk.authorize_terminal_mutations().ok()?;
        if self.sdk.terminal_project_context() != Some(self.context) {
            return None;
        }
        let mut owned = self.ticket.lock().unwrap();
        if owned.is_some() {
            return None;
        }
        let ticket = self.coordinator.reserve()?;
        *owned = Some(ticket);
        Some(ticket)
    }
    fn cancel(&self, ticket: u64) {
        let mut owned = self.ticket.lock().unwrap();
        if *owned == Some(ticket) {
            *owned = None;
            self.coordinator.cancel(ticket);
        }
    }
    fn settle(&self, ticket: u64) {
        let mut owned = self.ticket.lock().unwrap();
        if *owned == Some(ticket) {
            *owned = None;
            self.coordinator.settle(ticket);
        }
    }
}

impl PanelSdk {
    /// The host context must contain only descriptors accepted by host policy.
    /// A missing context fails closed, rather than granting default authority.
    pub fn new(state: DashboardState, tab: PanelTabKey) -> Self {
        let lifecycle = Arc::new(SdkLifecycle::default());
        let cleanup = lifecycle.clone();
        on_cleanup(move || cleanup.dispose());
        Self {
            state,
            host: use_context::<PanelHostState>(),
            tab,
            rpc: Arc::new(move |method, params| {
                async move { state.rpc_call(&method, params).await }.boxed_local()
            }),
            pty_subscribe: Arc::new(move |topic, epoch| {
                async move {
                    match epoch {
                        Some(epoch) => state.acquire_pty_topic_at_epoch(&topic, epoch).await,
                        None => state.acquire_pty_topic(&topic).await,
                    }
                }
                .boxed_local()
            }),
            lifecycle,
            locale: use_context::<crate::i18n::I18nCtx>()
                .map(|i18n| {
                    match i18n.get_locale_untracked() {
                        crate::i18n::Locale::en => "en",
                        crate::i18n::Locale::zh => "zh",
                    }
                    .to_string()
                })
                .unwrap_or_else(|| "en".into()),
        }
    }

    fn authorize_terminal_mutations(&self) -> Result<(), PanelSdkError> {
        self.authorize()?;
        let host = self.host.as_ref().ok_or(PanelSdkError::Denied)?;
        let registry = host.registry.lock().unwrap();
        // The reserved namespace can only be populated by host builtin startup.
        if self.tab.plugin_id != "aleph"
            || self.tab.tab_id != "terminal"
            || !registry
                .get(&self.tab)
                .is_some_and(|entry| entry.trust == aleph_protocol::panel::PanelUiTrust::Verified)
        {
            return Err(PanelSdkError::Denied);
        }
        Ok(())
    }

    fn terminal_project_context(&self) -> Option<(u64, u64)> {
        // The built-in terminal does not require community discovery to be
        // ready. Reuse its identity counters without using discovery as an
        // authorization check or resetting the scope on a transport refresh.
        self.state.panel_discovery.try_with_untracked(|state| {
            (state.context_epoch(), state.project_context().generation)
        })
    }

    pub(super) fn terminal_mutations(&self) -> Result<PanelTerminalMutations, PanelSdkError> {
        self.authorize_terminal_mutations()?;
        let context = self.terminal_project_context().ok_or(PanelSdkError::Disposed)?;
        let coordinator = self
            .state
            .terminal_mutations
            .try_with_value(|scopes| {
                let mut current = scopes.lock().unwrap();
                if let Some((identity, coordinator)) = current.as_ref() {
                    if *identity == context {
                        return coordinator.clone();
                    }
                }
                let coordinator = Arc::new(TerminalMutationCoordinator::default());
                *current = Some((context, coordinator.clone()));
                coordinator
            })
            .ok_or(PanelSdkError::Disposed)?;
        Ok(PanelTerminalMutations {
            sdk: self.clone(),
            context,
            coordinator,
            ticket: Mutex::new(None),
        })
    }

    pub fn invoke(
        &self,
        method: &str,
        params: Value,
    ) -> LocalBoxFuture<'static, Result<Value, String>> {
        let sdk = self.clone();
        let method = method.to_string();
        async move {
            sdk.authorize().map_err(|error| error.to_string())?;
            // Event registration must use the scoped subscription face, not RPC.
            if method.is_empty()
                || method.split('.').any(|part| {
                    part.is_empty()
                        || !part
                            .bytes()
                            .all(|ch| ch.is_ascii_alphanumeric() || ch == b'_')
                })
                || matches!(method.as_str(), "events.subscribe" | "events.unsubscribe")
            {
                return Err(PanelSdkError::InvalidMethod.to_string());
            }
            sdk.require_permission(&method)
                .map_err(|error| error.to_string())?;
            let result = (sdk.rpc)(method.clone(), params).await;
            sdk.authorize().map_err(|error| error.to_string())?;
            sdk.require_permission(&method)
                .map_err(|error| error.to_string())?;
            result
        }
        .boxed_local()
    }

    /// Read the connection projection without exposing the transport or signals.
    // Host fetch fence, deliberately absent from BrowserPanelSdk's public ABI.
    #[cfg(any(target_arch = "wasm32", test))]
    pub(super) fn asset_is_current(&self, descriptor: &PanelTabDescriptor, epoch: u64) -> bool {
        self.connection()
            .is_ok_and(|connection| connection.connected && connection.epoch == epoch)
            && self.host.as_ref().is_some_and(|host| {
                host.registry.lock().unwrap().get(&self.tab) == Some(descriptor)
            })
    }

    pub(super) fn asset_project_context(&self) -> Option<(u64, u64)> {
        let state = self.state.panel_discovery.try_get_untracked()?;
        matches!(
            state.status(),
            shared_ui_logic::panel::discovery::PanelDiscoveryStatus::Ready { .. }
        )
        .then(|| (state.context_epoch(), state.project_context().generation))
    }

    #[cfg(any(target_arch = "wasm32", test))]
    pub(super) fn asset_context_is_current(
        &self,
        descriptor: &PanelTabDescriptor,
        epoch: u64,
        context: (u64, u64),
    ) -> bool {
        self.asset_is_current(descriptor, epoch) && self.asset_project_context() == Some(context)
    }

    #[cfg(any(target_arch = "wasm32", test))]
    pub(super) fn asset_refused(
        &self,
        descriptor: &PanelTabDescriptor,
        epoch: u64,
        context: (u64, u64),
    ) {
        if !self.asset_context_is_current(descriptor, epoch, context) {
            return;
        }
        self.state.panel_discovery.update(|state| {
            state.mark_error_for_request(
                epoch,
                state.request_generation(),
                "Panel project context is unavailable".to_string(),
            );
        });
    }

    #[cfg(target_arch = "wasm32")]
    pub(super) async fn asset_credential(&self) -> Result<(String, String), PanelSdkError> {
        self.authorize()?;
        let credential = self
            .state
            .panel_asset_credential()
            .await
            .map_err(|_| PanelSdkError::Denied)?;
        self.authorize()?;
        Ok(credential)
    }

    pub fn connection(&self) -> Result<PanelConnectionSnapshot, PanelSdkError> {
        self.authorize()?;
        let connected = self
            .state
            .is_connected
            .try_get_untracked()
            .ok_or(PanelSdkError::Disposed)?;
        let epoch = self
            .state
            .connection_epoch
            .try_get_untracked()
            .ok_or(PanelSdkError::Disposed)?;
        Ok(PanelConnectionSnapshot { connected, epoch })
    }

    /// Exact-topic local event subscription. PTY consumers own a shared,
    /// epoch-bound upstream lease; disposing the last consumer releases it
    /// without removing independently recorded Shell subscriptions.
    /// Its explicit permission is `events.subscribe:<topic>` (no wildcard grant).
    pub fn subscribe(
        &self,
        topic: &str,
    ) -> LocalBoxFuture<'static, Result<PanelSubscription, String>> {
        self.subscribe_with_observer(topic, install_pty_reconnect_observer)
    }

    /// Exact PTY-only coverage for one epoch, with no reconnect observer.
    /// Consumers must use `try_drain` to observe loss and reacquire explicitly.
    pub fn subscribe_terminal(
        &self,
        topic: &str,
    ) -> LocalBoxFuture<'static, Result<PanelSubscription, String>> {
        self.subscribe_inner(topic, true, |_, _| {})
    }

    // The installer is the executor seam; registration and lease ownership stay
    // on this one production path, including native tests without a global runtime.
    fn subscribe_with_observer(
        &self,
        topic: &str,
        observe: impl Fn(&Arc<SubscriptionState>, String) + 'static,
    ) -> LocalBoxFuture<'static, Result<PanelSubscription, String>> {
        self.subscribe_inner(topic, false, observe)
    }

    fn subscribe_inner(
        &self,
        topic: &str,
        terminal: bool,
        observe: impl Fn(&Arc<SubscriptionState>, String) + 'static,
    ) -> LocalBoxFuture<'static, Result<PanelSubscription, String>> {
        let sdk = self.clone();
        let topic = topic.to_string();
        async move {
            sdk.authorize().map_err(|error| error.to_string())?;
            if topic.is_empty() || topic.contains('*') || (terminal && !is_pty_topic(&topic)) {
                return Err(PanelSdkError::Denied.to_string());
            }
            let permission = format!("events.subscribe:{topic}");
            sdk.require_permission(&permission)
                .map_err(|error| error.to_string())?;
            let pty_topic = is_pty_topic(&topic);
            let terminal_epoch = if terminal {
                let connection = sdk.connection().map_err(|error| error.to_string())?;
                sdk.state.check_pty_epoch(connection.epoch)?;
                Some(connection.epoch)
            } else {
                None
            };
            // Generic subscriptions retain their reconnect/readiness behavior.
            let pty_coverage = if pty_topic && !terminal {
                Some(sdk.establish_pty_coverage(&topic, &permission).await?)
            } else {
                None
            };
            let inner = Arc::new(SubscriptionState {
                sdk: sdk.clone(),
                permission,
                terminal_epoch,
                resources: Mutex::new(SubscriptionResources {
                    cleanup: None,
                    pty_coverage,
                    pending: VecDeque::new(),
                    terminal_pending: terminal.then(TerminalEventQueue::default),
                }),
            });
            let weak = Arc::downgrade(&inner);
            let event_topic = topic.clone();
            let id = sdk.state.subscribe_events(move |event| {
                if event.topic != event_topic {
                    return;
                }
                if let Some(inner) = weak.upgrade() {
                    inner.enqueue(event);
                }
            });
            let state = sdk.state;
            inner.resources.lock().unwrap().cleanup = Some(Box::new(move || {
                // After owner cleanup the event storage itself is already gone.
                if state.is_connected.try_get_untracked().is_some() {
                    state.unsubscribe_events(id);
                }
            }));
            {
                let mut subscriptions = sdk.lifecycle.subscriptions.lock().unwrap();
                subscriptions.retain(|entry| entry.strong_count() > 0);
                subscriptions.push(Arc::downgrade(&inner));
            }
            if let Some(epoch) = terminal_epoch {
                // `inner` owns listener cleanup even if this future is dropped.
                let lease = (sdk.pty_subscribe)(topic.clone(), Some(epoch)).await?;
                sdk.state.check_pty_epoch(epoch)?;
                if lease.epoch() != epoch || !inner.authorize() {
                    return Err("Terminal subscription disposed or epoch changed".into());
                }
                inner.resources.lock().unwrap().pty_coverage = Some(lease);
            }
            if pty_topic && !terminal {
                observe(&inner, topic.clone());
            }
            Ok(PanelSubscription { inner })
        }
        .boxed_local()
    }

    pub fn navigate(&self, key: PanelTabKey) -> Result<(), PanelSdkError> {
        self.authorize()?;
        let host = self.host.as_ref().ok_or(PanelSdkError::Denied)?;
        if !host
            .registry
            .lock()
            .unwrap()
            .get(&key)
            .is_some_and(|entry| entry.status == PanelTabStatus::Enabled)
        {
            return Err(PanelSdkError::InvalidNavigation);
        }
        if host.active.try_set(Some(key)).is_some() {
            return Err(PanelSdkError::Disposed);
        }
        Ok(())
    }

    pub fn theme(&self) -> PanelThemeSnapshot {
        #[cfg(target_arch = "wasm32")]
        {
            PanelThemeSnapshot {
                mode: crate::appearance::read_mode(),
                accent: crate::appearance::read_accent(),
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            PanelThemeSnapshot {
                mode: ThemeMode::System,
                accent: Accent::Mauve,
            }
        }
    }

    /// Locale at SDK construction; no reactive host handle crosses the ABI.
    pub fn locale(&self) -> String {
        self.locale.clone()
    }

    pub fn dispose(&self) {
        self.lifecycle.dispose();
    }

    fn authorize(&self) -> Result<(), PanelSdkError> {
        if self.lifecycle.disposed.load(Ordering::SeqCst)
            || self.state.is_connected.try_get_untracked().is_none()
        {
            return Err(PanelSdkError::Disposed);
        }
        let host = self.host.as_ref().ok_or(PanelSdkError::Denied)?;
        if !host
            .registry
            .lock()
            .unwrap()
            .get(&self.tab)
            .is_some_and(|entry| entry.status == PanelTabStatus::Enabled)
        {
            return Err(PanelSdkError::Denied);
        }
        Ok(())
    }

    async fn establish_pty_coverage(
        &self,
        topic: &str,
        permission: &str,
    ) -> Result<PtyTopicLease, String> {
        // One retry is specific to the PTY readiness boundary: a failed host
        // request or a reply superseded by reconnect must not be mistaken for
        // coverage, while an endlessly flapping socket must not create a loop.
        const ATTEMPTS: usize = 2;
        let mut last_error = None;
        for _ in 0..ATTEMPTS {
            self.authorize().map_err(|error| error.to_string())?;
            self.require_permission(permission)
                .map_err(|error| error.to_string())?;
            let coverage = match (self.pty_subscribe)(topic.to_string(), None).await {
                Ok(coverage) => coverage,
                Err(error) => {
                    last_error = Some(error);
                    continue;
                }
            };
            // The host call can race a mount revocation or owner disposal.
            // Do not publish a successful late subscription in that case.
            self.authorize().map_err(|error| error.to_string())?;
            self.require_permission(permission)
                .map_err(|error| error.to_string())?;
            let still_current = self.state.is_connected.try_get_untracked() == Some(true)
                && self.state.connection_epoch.try_get_untracked() == Some(coverage.epoch());
            if still_current {
                return Ok(coverage);
            }
            last_error = Some("Connection changed during PTY subscription".into());
        }
        Err(last_error.unwrap_or_else(|| "Connection changed during PTY subscription".into()))
    }

    fn require_permission(&self, permission: &str) -> Result<(), PanelSdkError> {
        let host = self.host.as_ref().ok_or(PanelSdkError::Denied)?;
        if !host
            .registry
            .lock()
            .unwrap()
            .get(&self.tab)
            .is_some_and(|entry| {
                entry
                    .permissions
                    .iter()
                    .any(|allowed| allowed == permission)
            })
        {
            return Err(PanelSdkError::Denied);
        }
        Ok(())
    }
}

// Local notifications are lossy: retain the newest 256 events per subscription,
// dropping the oldest on overflow. This is not a durable replay queue.
const SUBSCRIPTION_PENDING_CAPACITY: usize = 256;

fn is_pty_topic(topic: &str) -> bool {
    matches!(
        topic,
        aleph_protocol::pty::PTY_SCREEN_TOPIC | aleph_protocol::pty::PTY_EXIT_TOPIC
    )
}

fn reestablish_pty_subscription(
    subscription: Arc<SubscriptionState>,
    state: DashboardState,
    topic: String,
) -> LocalBoxFuture<'static, ()> {
    async move {
        if !subscription.authorize() { return; }
        let permission = subscription.permission.clone();
        let result = subscription.sdk.establish_pty_coverage(&topic, &permission).await;
        if let Ok(coverage) = result {
            let still_current = state.is_connected.try_get_untracked() == Some(true)
                && state.connection_epoch.try_get_untracked() == Some(coverage.epoch());
            if still_current && subscription.authorize() {
                let previous = {
                    let mut resources = subscription.resources.lock().unwrap();
                    if resources.cleanup.is_none() { return; }
                    resources.pty_coverage.replace(coverage)
                };
                drop(previous);
            }
        }
    }
    .boxed_local()
}

fn install_pty_reconnect_observer(subscription: &Arc<SubscriptionState>, topic: String) {
    let state = subscription.sdk.state;
    let weak = Arc::downgrade(subscription);
    Effect::new(move |_| {
        let epoch = state.connection_epoch.get();
        let Some(subscription) = weak.upgrade() else { return; };
        if !state.is_connected.get()
            || subscription.is_disposed()
            || subscription.pty_epoch() == Some(epoch)
        {
            return;
        }
        let subscription = Arc::clone(&subscription);
        let topic = topic.clone();
        spawn_local(reestablish_pty_subscription(subscription, state, topic));
    });
}

struct SubscriptionResources {
    cleanup: Option<Box<dyn FnOnce() + Send + Sync>>,
    pty_coverage: Option<PtyTopicLease>,
    pending: VecDeque<GatewayEvent>,
    terminal_pending: Option<TerminalEventQueue>,
}

struct SubscriptionState {
    sdk: PanelSdk,
    permission: String,
    terminal_epoch: Option<u64>,
    resources: Mutex<SubscriptionResources>,
}

impl SubscriptionState {
    fn pty_epoch(&self) -> Option<u64> {
        self.resources
            .lock()
            .unwrap()
            .pty_coverage
            .as_ref()
            .map(PtyTopicLease::epoch)
    }

    fn is_disposed(&self) -> bool {
        self.resources.lock().unwrap().cleanup.is_none()
    }

    fn enqueue(&self, event: GatewayEvent) {
        if !self.authorize() {
            return;
        }
        if self
            .terminal_epoch
            .is_some_and(|epoch| self.sdk.state.check_pty_epoch(epoch).is_err())
        {
            self.dispose();
            return;
        }
        let mut resources = self.resources.lock().unwrap();
        if resources.cleanup.is_some() {
            if let Some(pending) = resources.terminal_pending.as_mut() {
                pending.enqueue(event);
                return;
            }
            if resources.pending.len() == SUBSCRIPTION_PENDING_CAPACITY {
                resources.pending.pop_front();
            }
            resources.pending.push_back(event);
        }
    }

    fn authorize(&self) -> bool {
        if self.is_disposed() {
            return false;
        }
        if self
            .sdk
            .authorize()
            .and_then(|()| self.sdk.require_permission(&self.permission))
            .is_ok()
        {
            return true;
        }
        // Drop queued data and release the listener before exposing any events.
        self.dispose();
        false
    }

    fn dispose(&self) {
        let (cleanup, coverage) = {
            let mut resources = self.resources.lock().unwrap();
            resources.pending.clear();
            if let Some(pending) = resources.terminal_pending.as_mut() {
                pending.clear();
            }
            (resources.cleanup.take(), resources.pty_coverage.take())
        };
        if let Some(cleanup) = cleanup {
            cleanup();
        }
        drop(coverage);
    }
}

impl Drop for SubscriptionState {
    fn drop(&mut self) {
        self.dispose();
    }
}

/// Clones share one registration. Consuming either clone disposes it for all;
/// dropping the last clone also releases the listener. Generic queues keep the
/// newest 256 events. Terminal-only queues instead report count/byte overflow
/// through `try_drain`; their confirmed epoch never advances on reconnect.
/// Revoked authority clears the buffer and releases local and leased resources.
#[derive(Clone)]
pub struct PanelSubscription {
    inner: Arc<SubscriptionState>,
}

impl PanelSubscription {
    pub fn coverage_epoch(&self) -> Option<u64> {
        self.inner.terminal_epoch
    }

    /// Terminal loss is sticky until observed, even if no later frame arrives.
    pub fn try_drain(&self) -> Result<Vec<GatewayEvent>, String> {
        let Some(epoch) = self.inner.terminal_epoch else {
            return Ok(self.drain());
        };
        if !self.inner.authorize() {
            return Err("Terminal subscription disposed or denied".into());
        }
        let mut resources = self.inner.resources.lock().unwrap();
        let pending = resources.terminal_pending.as_mut().expect("terminal queue");
        if self.inner.sdk.state.is_connected.try_get_untracked() != Some(true)
            || self.inner.sdk.state.connection_epoch.try_get_untracked() != Some(epoch)
        {
            pending.clear();
            return Err("Terminal subscription epoch changed".into());
        }
        pending.drain()
    }

    pub fn dispose(self) {
        self.inner.dispose();
    }

    pub fn is_disposed(&self) -> bool {
        self.inner.resources.lock().unwrap().cleanup.is_none()
    }

    pub fn drain(&self) -> Vec<GatewayEvent> {
        if !self.inner.authorize() {
            return Vec::new();
        }
        self.inner
            .resources
            .lock()
            .unwrap()
            .pending
            .drain(..)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{PanelSdk, PanelSdkError};
    use crate::context::{DashboardState, GatewayEvent};
    use crate::panel::{PanelHostState, PanelModuleHandle, PanelMountState};
    use aleph_protocol::panel::{
        PanelTabDescriptor, PanelTabKey, PanelTabStatus, PanelUiTrust, PANEL_UI_API_MAJOR,
    };
    use futures::{executor::block_on, FutureExt};
    use leptos::prelude::{provide_context, GetUntracked, Owner, Set, Update};
    use serde_json::{json, Value};
    use shared_ui_logic::panel::registry::TabRegistry;
    use shared_ui_logic::terminal_mutation::TerminalMutationScope;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    fn key(id: &str) -> PanelTabKey {
        PanelTabKey {
            plugin_id: "test".into(),
            tab_id: id.into(),
        }
    }

    fn fixture() -> (PanelSdk, PanelHostState, Arc<Mutex<Vec<(String, Value)>>>) {
        let mut registry = TabRegistry::default();
        registry
            .insert_or_replace(PanelTabDescriptor {
                key: key("home"),
                label: "Home".into(),
                icon: "home".into(),
                order: 0,
                entry: "home.js".into(),
                api_major: PANEL_UI_API_MAJOR,
                trust: PanelUiTrust::Verified,
                permissions: vec![
                    "sessions.list".into(),
                    "events.subscribe:session.updated".into(),
                ],
                status: PanelTabStatus::Enabled,
                status_detail: None,
            })
            .unwrap();
        let host = PanelHostState::new(registry);
        provide_context(host.clone());
        let mut sdk = PanelSdk::new(DashboardState::new(), key("home"));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = calls.clone();
        sdk.rpc = Arc::new(move |method, params| {
            recorded.lock().unwrap().push((method, params));
            async { Ok(json!({"sessions": []})) }.boxed_local()
        });
        (sdk, host, calls)
    }

    fn allow_pty(host: &PanelHostState) {
        let mut row = host
            .registry
            .lock()
            .unwrap()
            .get(&key("home"))
            .unwrap()
            .clone();
        row.permissions.push("events.subscribe:pty.screen".into());
        host.registry
            .lock()
            .unwrap()
            .insert_or_replace(row)
            .unwrap();
    }

    fn pty_subscription_state(sdk: &PanelSdk) -> Arc<super::SubscriptionState> {
        Arc::new(super::SubscriptionState {
            sdk: sdk.clone(),
            permission: "events.subscribe:pty.screen".into(),
            terminal_epoch: None,
            resources: Mutex::new(super::SubscriptionResources {
                cleanup: Some(Box::new(|| {})),
                pty_coverage: None,
                pending: std::collections::VecDeque::new(),
                terminal_pending: None,
            }),
        })
    }

    fn terminal_fixture() -> (PanelSdk, PanelHostState) {
        let (mut sdk, _, _) = fixture();
        let tab = PanelTabKey {
            plugin_id: "aleph".into(),
            tab_id: "terminal".into(),
        };
        let descriptor = PanelTabDescriptor {
            key: tab.clone(),
            label: "Terminal".into(),
            icon: "terminal".into(),
            order: 0,
            entry: "./tabs/terminal/dev/terminal.js".into(),
            api_major: PANEL_UI_API_MAJOR,
            trust: PanelUiTrust::Verified,
            permissions: vec!["pty.spawn".into(), "pty.close".into()],
            status: PanelTabStatus::Enabled,
            status_detail: None,
        };
        let host = PanelHostState::new(TabRegistry::new(vec![descriptor]).unwrap());
        sdk.host = Some(host.clone());
        sdk.tab = tab;
        sdk.state.is_connected.set(true);
        sdk.state.connection_epoch.set(7);
        terminal_project(&sdk, "A", 1);
        (sdk, host)
    }

    fn terminal_project(sdk: &PanelSdk, project: &str, generation: u64) {
        sdk.state.panel_discovery.update(|state| {
            state.invalidate_context();
            let request = state.begin_connection(7).unwrap();
            state.accept_context_snapshot_for_request(
                7,
                request,
                aleph_protocol::panel::PanelUiContextSnapshot {
                    snapshot: aleph_protocol::panel::PanelUiCapabilitySnapshot::new(10, Vec::new()),
                    project_context: aleph_protocol::panel::PanelProjectSelection {
                        project_id: Some(project.into()),
                        generation,
                    },
                },
            );
        });
    }

    #[test]
    fn terminal_mutation_scope_does_not_require_discovery_ready() {
        Owner::new().with(|| {
            let (sdk, _) = terminal_fixture();
            let first = sdk.terminal_mutations().unwrap();
            let pending = first.reserve().unwrap();
            sdk.state.panel_discovery.update(|state| {
                assert!(state.begin_connection(8).is_some());
            });
            assert!(sdk.asset_project_context().is_none());
            let current = sdk.terminal_mutations().unwrap();
            assert!(current.pending());
            first.settle(pending);
            assert_eq!(current.revision(), 1);
            assert!(!current.pending());
        });
    }

    #[test]
    fn terminal_dispose_before_send_cancels_without_dispatch_or_invalidation() {
        Owner::new().with(|| {
            let (mut sdk, host) = terminal_fixture();
            sdk.rpc = Arc::new(|_, _| panic!("disposed invoke reached Gateway"));
            let scope = sdk.terminal_mutations().unwrap();
            let ticket = scope.reserve().unwrap();
            sdk.dispose();
            assert_eq!(
                block_on(sdk.invoke("pty.spawn", json!({}))),
                Err("Disposed".into())
            );
            assert!(scope.reserve().is_none());
            scope.cancel(ticket);
            let mut next = PanelSdk::new(sdk.state, sdk.tab.clone());
            next.host = Some(host);
            let scope = next.terminal_mutations().unwrap();
            assert!(!scope.pending());
            assert_eq!(scope.revision(), 0);
        });
    }

    #[test]
    fn terminal_mutations_survive_disposed_shell_but_never_cross_dashboard_or_project() {
        use futures::task::noop_waker_ref;
        use std::task::{Context, Poll};
        Owner::new().with(|| {
            let (mut sdk, host) = terminal_fixture();
            let scope = sdk.terminal_mutations().unwrap();
            let ticket = scope.reserve().unwrap();
            let (separate_app, _) = terminal_fixture();
            let separate_scope = separate_app.terminal_mutations().unwrap();
            assert!(
                !separate_scope.pending(),
                "separate application with same project identity"
            );
            let mut separate_shell = PanelSdk::new(sdk.state, sdk.tab.clone());
            separate_shell.host = Some(PanelHostState::new(host.registry.lock().unwrap().clone()));
            assert!(
                separate_shell.terminal_mutations().unwrap().pending(),
                "remount creates a new Shell but must share the pending action"
            );
            let (settle, rx) = futures::channel::oneshot::channel();
            let rx = Arc::new(Mutex::new(Some(rx)));
            let reached = Arc::new(AtomicUsize::new(0));
            sdk.rpc = Arc::new({
                let reached = reached.clone();
                move |_, _| {
                    reached.fetch_add(1, Ordering::SeqCst);
                    let rx = rx.lock().unwrap().take().unwrap();
                    async move { rx.await.unwrap() }.boxed_local()
                }
            });
            let mut invoke = sdk.invoke("pty.spawn", json!({}));
            let mut cx = Context::from_waker(noop_waker_ref());
            assert!(matches!(invoke.as_mut().poll(&mut cx), Poll::Pending));
            assert_eq!(reached.load(Ordering::SeqCst), 1);
            sdk.dispose();
            let mut replacement = PanelSdk::new(sdk.state, sdk.tab.clone());
            replacement.host = separate_shell.host;
            let next = replacement.terminal_mutations().unwrap();
            assert!(next.pending());
            next.settle(ticket);
            assert!(next.pending(), "replacement cannot release another SDK's ticket");
            assert!(next.reserve().is_none());
            assert!(scope.reserve().is_none());
            settle
                .send(Ok(json!({"session_id":"stale-payload"})))
                .unwrap();
            assert_eq!(
                block_on(invoke),
                Err("Disposed".into()),
                "old invoke reached Gateway but its wrapper still rejects delivery"
            );
            scope.settle(ticket);
            scope.settle(ticket);
            assert!(!next.pending());
            assert_eq!(next.revision(), 1);
            let unsent = next.reserve().unwrap();
            next.cancel(unsent);
            assert_eq!(next.revision(), 1);
            let old_ticket = next.reserve().unwrap();
            terminal_project(&replacement, "B", 2);
            let project_b = replacement.terminal_mutations().unwrap();
            assert!(!project_b.pending());
            next.settle(old_ticket);
            assert_eq!(project_b.revision(), 0);
            assert!(next.reserve().is_none(), "old project cannot reserve");
            let (separate, _) = terminal_fixture();
            let separate = separate.terminal_mutations().unwrap();
            assert_eq!(separate.revision(), 0);
            assert!(!separate.pending());
            let mut untrusted = host
                .registry
                .lock()
                .unwrap()
                .get(&replacement.tab)
                .unwrap()
                .clone();
            untrusted.trust = PanelUiTrust::Trusted;
            replacement.host = Some(PanelHostState::new(
                TabRegistry::new(vec![untrusted]).unwrap(),
            ));
            assert!(replacement.terminal_mutations().is_err());
        });
    }

    #[test]
    fn project_context_fences_delivery_and_old_refusals_cannot_clear_new_content() {
        Owner::new().with(|| {
            let (sdk, host, _) = fixture();
            sdk.state.is_connected.set(true);
            sdk.state.connection_epoch.set(7);
            let descriptor = host
                .registry
                .lock()
                .unwrap()
                .get(&key("home"))
                .unwrap()
                .clone();
            let accept = |project: &str, generation| {
                sdk.state.panel_discovery.update(|state| {
                    let request = state.begin_connection(7).unwrap();
                    state.accept_context_snapshot_for_request(
                        7,
                        request,
                        aleph_protocol::panel::PanelUiContextSnapshot {
                            snapshot: aleph_protocol::panel::PanelUiCapabilitySnapshot::new(
                                10,
                                Vec::new(),
                            ),
                            project_context: aleph_protocol::panel::PanelProjectSelection {
                                project_id: Some(project.into()),
                                generation,
                            },
                        },
                    );
                })
            };
            accept("A", 4);
            let old = sdk.asset_project_context().unwrap();
            assert!(sdk.asset_context_is_current(&descriptor, 7, old));
            sdk.state.panel_discovery.update(|state| {
                state.invalidate_context();
            });
            assert!(!sdk.asset_context_is_current(&descriptor, 7, old));
            accept("B", 5);
            sdk.asset_refused(&descriptor, 7, old);
            assert!(sdk
                .state
                .panel_discovery
                .get_untracked()
                .snapshot()
                .is_some());
            sdk.asset_refused(&descriptor, 7, sdk.asset_project_context().unwrap());
            assert!(sdk
                .state
                .panel_discovery
                .get_untracked()
                .snapshot()
                .is_none());
        });
    }

    #[test]
    fn asset_delivery_fences_connection_epoch_descriptor_and_disposal() {
        Owner::new().with(|| {
            let (sdk, host, _) = fixture();
            let descriptor = host
                .registry
                .lock()
                .unwrap()
                .get(&key("home"))
                .unwrap()
                .clone();
            assert!(!sdk.asset_is_current(&descriptor, 7));
            sdk.state.is_connected.set(true);
            sdk.state.connection_epoch.set(7);
            assert!(sdk.asset_is_current(&descriptor, 7));
            sdk.state.connection_epoch.set(8);
            assert!(!sdk.asset_is_current(&descriptor, 7));
            sdk.state.connection_epoch.set(7);
            let mut changed = descriptor.clone();
            changed.entry = "changed.js".into();
            host.registry
                .lock()
                .unwrap()
                .insert_or_replace(changed)
                .unwrap();
            assert!(!sdk.asset_is_current(&descriptor, 7));
            host.registry
                .lock()
                .unwrap()
                .insert_or_replace(descriptor.clone())
                .unwrap();
            assert!(sdk.asset_is_current(&descriptor, 7));
            sdk.lifecycle.dispose();
            assert!(!sdk.asset_is_current(&descriptor, 7));
        });
    }

    #[test]
    fn invoke_forwards_only_declared_valid_methods() {
        Owner::new().with(|| {
            let (sdk, _, calls) = fixture();
            assert_eq!(
                block_on(sdk.invoke("sessions.list", json!({"limit": 3}))),
                Ok(json!({"sessions": []}))
            );
            assert_eq!(
                block_on(sdk.invoke("provider.stop", json!({}))),
                Err(PanelSdkError::Denied.to_string())
            );
            assert_eq!(
                block_on(sdk.invoke("", json!({}))),
                Err(PanelSdkError::InvalidMethod.to_string())
            );
            assert_eq!(
                block_on(sdk.invoke("events.subscribe", json!({"topics": ["**"]}))),
                Err(PanelSdkError::InvalidMethod.to_string())
            );
            assert_eq!(
                *calls.lock().unwrap(),
                vec![("sessions.list".into(), json!({"limit": 3}))]
            );
        });
    }

    #[test]
    fn navigation_uses_current_enabled_registry() {
        Owner::new().with(|| {
            let (sdk, host, _) = fixture();
            assert_eq!(
                sdk.navigate(key("missing")),
                Err(PanelSdkError::InvalidNavigation)
            );
            assert_eq!(host.active.get_untracked(), None);
            assert_eq!(sdk.navigate(key("home")), Ok(()));
            assert_eq!(host.active.get_untracked(), Some(key("home")));
            let mut row = host
                .registry
                .lock()
                .unwrap()
                .get(&key("home"))
                .unwrap()
                .clone();
            row.status = PanelTabStatus::Disabled;
            host.registry
                .lock()
                .unwrap()
                .insert_or_replace(row)
                .unwrap();
            assert_eq!(sdk.navigate(key("home")), Err(PanelSdkError::Denied));
            assert_eq!(
                block_on(sdk.invoke("sessions.list", json!({}))),
                Err(PanelSdkError::Denied.to_string())
            );
        });
    }

    #[test]
    fn subscription_disposal_is_shared_and_idempotent() {
        Owner::new().with(|| {
            let (sdk, _, calls) = fixture();
            assert!(matches!(block_on(sdk.subscribe("**")), Err(error) if error == PanelSdkError::Denied.to_string()));
            let subscription = block_on(sdk.subscribe("session.updated")).unwrap();
            assert_eq!(sdk.lifecycle.subscriptions.lock().unwrap().len(), 1);
            // Actual DashboardState slot reuse proves the listener was removed.
            subscription.clone().dispose();
            let replacement = sdk.state.subscribe_events(|_| {});
            assert_eq!(replacement, 0);
            subscription.clone().dispose();
            // A second disposal must not remove the new occupant of the old slot.
            assert_eq!(sdk.state.subscribe_events(|_| {}), 1);
            assert!(subscription.is_disposed());
            assert!(calls.lock().unwrap().is_empty());
        });
    }

    #[test]
    fn disposal_fences_queued_and_later_operations_without_stopping_provider() {
        let owner = Owner::new();
        let (sdk, _, calls) = owner.with(fixture);
        let pending = sdk.invoke("sessions.list", json!({}));
        let pending_subscription = sdk.subscribe("session.updated");
        let subscription = block_on(sdk.subscribe("session.updated")).unwrap();
        sdk.dispose();
        sdk.dispose();
        assert!(subscription.is_disposed());
        assert_eq!(block_on(pending), Err(PanelSdkError::Disposed.to_string()));
        assert!(
            matches!(block_on(pending_subscription), Err(error) if error == PanelSdkError::Disposed.to_string())
        );
        assert_eq!(
            block_on(sdk.invoke("sessions.list", json!({}))),
            Err(PanelSdkError::Disposed.to_string())
        );
        assert_eq!(sdk.navigate(key("home")), Err(PanelSdkError::Disposed));
        assert!(calls.lock().unwrap().is_empty());
        owner.cleanup();
    }

    #[test]
    fn late_rpc_response_is_not_delivered_to_disposed_sdk() {
        Owner::new().with(|| {
            let (mut sdk, _, calls) = fixture();
            let (reply_tx, reply_rx) = futures::channel::oneshot::channel();
            let reply = Mutex::new(Some(reply_rx));
            sdk.rpc = Arc::new(move |_, _| {
                let reply = reply.lock().unwrap().take().unwrap();
                async move { reply.await.unwrap() }.boxed_local()
            });
            let mut pending = sdk.invoke("sessions.list", json!({}));
            assert!(pending.as_mut().now_or_never().is_none());
            sdk.dispose();
            reply_tx.send(Ok(json!({"late": true}))).unwrap();
            assert_eq!(block_on(pending), Err(PanelSdkError::Disposed.to_string()));
            assert!(calls.lock().unwrap().is_empty());
        });
    }

    #[test]
    fn permission_removal_is_observed_without_rebuilding_sdk() {
        Owner::new().with(|| {
            let (sdk, host, calls) = fixture();
            let mut row = host.registry.lock().unwrap().get(&key("home")).unwrap().clone();
            row.permissions.clear();
            host.registry.lock().unwrap().insert_or_replace(row).unwrap();
            assert_eq!(block_on(sdk.invoke("sessions.list", json!({}))), Err(PanelSdkError::Denied.to_string()));
            assert!(matches!(block_on(sdk.subscribe("session.updated")), Err(error) if error == PanelSdkError::Denied.to_string()));
            assert!(calls.lock().unwrap().is_empty());
        });
    }

    #[test]
    fn subscription_revocation_fences_enqueue_and_drain() {
        // Each boundary must independently detect all current-authority failures.
        for change in ["permission", "disabled", "removed"] {
            for enqueue_after_revocation in [false, true] {
                Owner::new().with(|| {
                    let (sdk, host, calls) = fixture();
                    let subscription = block_on(sdk.subscribe("session.updated")).unwrap();
                    let event = GatewayEvent {
                        topic: "session.updated".into(),
                        data: json!({"sequence": 0}),
                    };
                    subscription.inner.enqueue(event.clone());
                    assert_eq!(
                        subscription.inner.resources.lock().unwrap().pending.len(),
                        1
                    );
                    {
                        let mut registry = host.registry.lock().unwrap();
                        let mut row = registry.get(&key("home")).unwrap().clone();
                        match change {
                            "permission" => {
                                row.permissions.clear();
                                registry.insert_or_replace(row).unwrap();
                            }
                            "disabled" => {
                                row.status = PanelTabStatus::Disabled;
                                registry.insert_or_replace(row).unwrap();
                            }
                            "removed" => assert!(registry.remove(&key("home"))),
                            _ => unreachable!(),
                        }
                    }
                    if enqueue_after_revocation {
                        subscription.inner.enqueue(event.clone());
                        assert!(subscription.is_disposed(), "enqueue: {change}");
                        assert!(subscription
                            .inner
                            .resources
                            .lock()
                            .unwrap()
                            .pending
                            .is_empty());
                    }
                    assert!(subscription.drain().is_empty(), "drain: {change}");
                    assert!(subscription.is_disposed());
                    // Fail-closed cleanup releases only the local listener slot.
                    assert_eq!(sdk.state.subscribe_events(|_| {}), 0);
                    subscription.inner.enqueue(event);
                    assert!(subscription.drain().is_empty());
                    assert_eq!(sdk.state.subscribe_events(|_| {}), 1);
                    assert!(calls.lock().unwrap().is_empty());
                });
            }
        }
    }

    #[test]
    fn connection_snapshot_is_revoked_with_mount() {
        Owner::new().with(|| {
            let (sdk, host, _) = fixture();
            assert_eq!(
                sdk.connection(),
                Ok(super::PanelConnectionSnapshot {
                    connected: false,
                    epoch: 0,
                })
            );
            let mut row = host
                .registry
                .lock()
                .unwrap()
                .get(&key("home"))
                .unwrap()
                .clone();
            row.status = PanelTabStatus::Disabled;
            host.registry
                .lock()
                .unwrap()
                .insert_or_replace(row)
                .unwrap();
            assert_eq!(sdk.connection(), Err(PanelSdkError::Denied));
            sdk.dispose();
            assert_eq!(sdk.connection(), Err(PanelSdkError::Disposed));
        });
    }

    #[test]
    fn pty_topic_is_ready_before_subscribe_returns() {
        assert!(super::is_pty_topic(aleph_protocol::pty::PTY_SCREEN_TOPIC));
        assert!(super::is_pty_topic(aleph_protocol::pty::PTY_EXIT_TOPIC));
        assert!(!super::is_pty_topic("pty.*"));
        Owner::new().with(|| {
            let (mut sdk, host, _) = fixture();
            assert!(matches!(
                block_on(sdk.subscribe(aleph_protocol::pty::PTY_SCREEN_TOPIC)),
                Err(error) if error == PanelSdkError::Denied.to_string()
            ));
            let mut row = host
                .registry
                .lock()
                .unwrap()
                .get(&key("home"))
                .unwrap()
                .clone();
            row.permissions.push("events.subscribe:pty.screen".into());
            host.registry
                .lock()
                .unwrap()
                .insert_or_replace(row)
                .unwrap();
            sdk.state.is_connected.set(true);
            let transport = crate::context::tests::PtyHarness::with_rpc(sdk.state, |method, _| {
                async move { if method == "events.subscribe" { Err("Not connected".into()) } else { Ok(Value::Null) } }.boxed_local()
            });
            wire_pty(&mut sdk, &transport);
            assert!(matches!(
                drive_pty(&transport, sdk.subscribe(aleph_protocol::pty::PTY_SCREEN_TOPIC)),
                Err(error) if error == "Not connected"
            ));
            // The host readiness failure is returned before a local listener is published.
            assert_eq!(sdk.state.subscribe_events(|_| {}), 0);
        });
    }

    #[test]
    fn pty_lease_disposal_drives_one_release_for_clones_sdk_cleanup_and_permission_loss() {
        for disposal in 0..4 {
            Owner::new().with(|| {
                let (mut sdk, host, _) = fixture();
                allow_pty(&host);
                let tab_owner = Owner::new();
                sdk = tab_owner.with(|| PanelSdk::new(sdk.state, key("home")));
                sdk.state.is_connected.set(true);
                let transport = crate::context::tests::PtyHarness::new(sdk.state);
                let state = sdk.state;
                let io = transport.io.clone();
                sdk.pty_subscribe = Arc::new(move |topic, epoch| {
                    let io = io.clone();
                    async move {
                        match epoch {
                            Some(epoch) => {
                                state
                                    .acquire_pty_topic_at_epoch_with_io(&topic, epoch, io, async {
                                        Ok(())
                                    })
                                    .await
                            }
                            None => state.acquire_pty_topic_with_io(&topic, io).await,
                        }
                    }
                    .boxed_local()
                });
                let observed = Arc::new(Mutex::new(Vec::new()));
                let scheduled = observed.clone();
                let mut pending = sdk.subscribe_with_observer("pty.screen", move |subscription, topic| {
                    scheduled.lock().unwrap().push((Arc::downgrade(subscription), topic));
                });
                assert!(pending.as_mut().now_or_never().is_none());
                assert_eq!(transport.flush(), [Ok(())]);
                let subscription = block_on(pending).unwrap();
                assert_eq!(observed.lock().unwrap()[0].1, "pty.screen");
                if disposal == 1 {
                    let mut row = host.registry.lock().unwrap().get(&key("home")).unwrap().clone();
                    row.permissions.clear();
                    host.registry.lock().unwrap().insert_or_replace(row).unwrap();
                    assert!(subscription.drain().is_empty());
                } else if disposal == 0 {
                    subscription.clone().dispose();
                } else if disposal == 2 {
                    tab_owner.cleanup();
                }
                if disposal != 3 {
                    subscription.clone().dispose();
                    sdk.dispose();
                    sdk.dispose();
                }
                drop(subscription);
                assert!(observed.lock().unwrap()[0].0.upgrade().is_none());
                assert_eq!(transport.flush(), [Ok(())]);
                assert_eq!(transport.methods(), ["events.subscribe", "events.unsubscribe"]);
            });
        }
    }

    #[test]
    fn pty_pending_subscribe_disposed_sdk_never_publishes_and_releases_confirmed_coverage() {
        Owner::new().with(|| {
            let (mut sdk, host, _) = fixture();
            allow_pty(&host);
            sdk.state.is_connected.set(true);
            let transport = crate::context::tests::PtyHarness::new(sdk.state);
            let state = sdk.state;
            let io = transport.io.clone();
            sdk.pty_subscribe = Arc::new(move |topic, epoch| {
                let io = io.clone();
                async move { match epoch {
                Some(epoch) => state.acquire_pty_topic_at_epoch_with_io(&topic, epoch, io, async { Ok(()) }).await,
                None => state.acquire_pty_topic_with_io(&topic, io).await,
            } }.boxed_local()
            });
            let mut pending = sdk.subscribe("pty.screen");
            assert!(pending.as_mut().now_or_never().is_none());
            sdk.dispose();
            assert_eq!(transport.flush(), [Ok(())]);
            assert!(matches!(block_on(pending), Err(error) if error == PanelSdkError::Disposed.to_string()));
            assert_eq!(sdk.state.subscribe_events(|_| {}), 0);
            assert_eq!(transport.flush(), [Ok(())]);
            assert_eq!(transport.methods(), ["events.subscribe", "events.unsubscribe"]);
        });
    }

    fn wire_pty(sdk: &mut PanelSdk, transport: &crate::context::tests::PtyHarness) {
        let state = sdk.state;
        let io = transport.io.clone();
        sdk.pty_subscribe = Arc::new(move |topic, epoch| {
            let io = io.clone();
            async move {
                match epoch {
                    Some(epoch) => {
                        state
                            .acquire_pty_topic_at_epoch_with_io(&topic, epoch, io, async { Ok(()) })
                            .await
                    }
                    None => state.acquire_pty_topic_with_io(&topic, io).await,
                }
            }
            .boxed_local()
        });
    }

    fn drive_pty<T>(
        transport: &crate::context::tests::PtyHarness,
        future: impl std::future::Future<Output = T>,
    ) -> T {
        let mut future = Box::pin(future);
        loop {
            if let Some(result) = future.as_mut().now_or_never() { return result; }
            let results = transport.flush();
            assert!(!results.is_empty(), "pending coverage has owned executor work");
            assert!(results.iter().all(Result::is_ok));
        }
    }

    #[test]
    fn terminal_coverage_callbacks_capture_pre_ack_frames_for_each_holder() {
        Owner::new().with(|| {
            let (mut sdk, host, _) = fixture();
            allow_pty(&host);
            sdk.state.is_connected.set(true);
            sdk.state.connection_epoch.set(1);
            let (ack, wait) = futures::channel::oneshot::channel();
            let wait = Arc::new(Mutex::new(Some(wait)));
            let transport = crate::context::tests::PtyHarness::with_rpc(sdk.state, move |_, _| {
                let wait = wait.lock().unwrap().take();
                async move {
                    if let Some(wait) = wait {
                        wait.await.unwrap();
                    }
                    Ok(Value::Null)
                }
                .boxed_local()
            });
            wire_pty(&mut sdk, &transport);
            let mut first = sdk.subscribe_terminal("pty.screen");
            let mut second = sdk.subscribe_terminal("pty.screen");
            assert!(first.as_mut().now_or_never().is_none());
            assert!(second.as_mut().now_or_never().is_none());
            let mut upstream = transport.next();
            assert!(upstream.as_mut().now_or_never().is_none());
            // Real host callback dispatch, while the subscribe RPC is still pending.
            sdk.state.dispatch_event(GatewayEvent {
                topic: "pty.screen".into(),
                data: json!({"seq": 1}),
            });
            sdk.state.dispatch_event(GatewayEvent {
                topic: "pty.exit".into(),
                data: Value::Null,
            });
            ack.send(()).unwrap();
            block_on(upstream).unwrap();
            let first = block_on(first).unwrap();
            let second = drive_pty(&transport, second).unwrap();
            for holder in [&first, &second] {
                let events = holder.try_drain().unwrap();
                assert_eq!(events.len(), 1);
                assert_eq!(events[0].data, json!({"seq": 1}));
            }
            first.dispose();
            sdk.state.dispatch_event(GatewayEvent {
                topic: "pty.screen".into(),
                data: json!({"seq": 2}),
            });
            assert_eq!(second.try_drain().unwrap()[0].data, json!({"seq": 2}));
            sdk.state.connection_epoch.set(2);
            sdk.state.dispatch_event(GatewayEvent {
                topic: "pty.screen".into(),
                data: json!({"seq": 3}),
            });
            assert!(second.try_drain().is_err());
            second.dispose();
            assert!(transport.flush().iter().all(Result::is_ok));
            assert_eq!(transport.methods(), ["events.subscribe"]);
        });
    }

    #[test]
    fn terminal_coverage_pending_listener_is_raii_on_cancel_deny_and_owner_dispose() {
        for cleanup in 0..3 {
            Owner::new().with(|| {
                let (mut sdk, host, _) = fixture();
                allow_pty(&host);
                sdk.state.is_connected.set(true);
                let transport = crate::context::tests::PtyHarness::new(sdk.state);
                wire_pty(&mut sdk, &transport);
                let mut pending = sdk.subscribe_terminal("pty.screen");
                assert!(pending.as_mut().now_or_never().is_none());
                if cleanup == 0 {
                    drop(pending);
                    assert_eq!(transport.flush(), [Ok(()), Ok(())]);
                } else {
                    if cleanup == 1 {
                        let mut row = host
                            .registry
                            .lock()
                            .unwrap()
                            .get(&key("home"))
                            .unwrap()
                            .clone();
                        row.permissions.clear();
                        host.registry
                            .lock()
                            .unwrap()
                            .insert_or_replace(row)
                            .unwrap();
                    } else {
                        sdk.dispose();
                    }
                    assert_eq!(transport.flush(), [Ok(())]);
                    assert!(block_on(pending).is_err());
                    assert_eq!(transport.flush(), [Ok(())]);
                }
                assert_eq!(sdk.state.subscribe_events(|_| {}), 0, "listener released");
                assert_eq!(
                    transport.methods(),
                    ["events.subscribe", "events.unsubscribe"]
                );
            });
        }
    }

    #[test]
    fn terminal_coverage_drops_partial_stale_acquisition() {
        for dispose_during_acquire in [false, true] {
            Owner::new().with(|| {
                let (mut sdk, host, _) = fixture();
                allow_pty(&host);
                let mut row = host
                    .registry
                    .lock()
                    .unwrap()
                    .get(&key("home"))
                    .unwrap()
                    .clone();
                row.permissions.push("events.subscribe:pty.exit".into());
                host.registry
                    .lock()
                    .unwrap()
                    .insert_or_replace(row)
                    .unwrap();
                sdk.state.is_connected.set(true);
                sdk.state.connection_epoch.set(1);
                let (reply, waiting) = futures::channel::oneshot::channel::<()>();
                let waiting = Arc::new(Mutex::new(Some(waiting)));
                let transport =
                    crate::context::tests::PtyHarness::with_rpc(sdk.state, move |method, params| {
                        let waiting =
                            if method == "events.subscribe" && params["topics"][0] == "pty.exit" {
                                waiting.lock().unwrap().take()
                            } else {
                                None
                            };
                        async move {
                            if let Some(waiting) = waiting {
                                waiting.await.unwrap();
                            }
                            Ok(Value::Null)
                        }
                        .boxed_local()
                    },
                );
                wire_pty(&mut sdk, &transport);
                let screen = drive_pty(&transport, sdk.subscribe_terminal("pty.screen")).unwrap();
                assert_eq!(screen.coverage_epoch(), Some(1));
                let mut exit = sdk.subscribe_terminal("pty.exit");
                assert!(exit.as_mut().now_or_never().is_none());
                let mut completing = transport.next();
                assert!(completing.as_mut().now_or_never().is_none());
                let old_owners = transport.owner_count_probe();
                sdk.state.connection_epoch.set(2);
                let mut replacement_sdk = PanelSdk::new(sdk.state, key("home"));
                wire_pty(&mut replacement_sdk, &transport);
                let replacement_screen =
                    drive_pty(&transport, replacement_sdk.subscribe_terminal("pty.screen")).unwrap();
                let replacement_exit =
                    drive_pty(&transport, replacement_sdk.subscribe_terminal("pty.exit")).unwrap();
                if dispose_during_acquire {
                    sdk.dispose();
                }
                reply.send(()).unwrap();
                block_on(completing).unwrap();
                assert!(
                    drive_pty(&transport, exit).is_err(),
                    "old epoch must not retry into the replacement"
                );
                assert!(screen.try_drain().is_err());
                screen.dispose();
                assert!(transport.flush().iter().all(Result::is_ok));
                let old_leases = old_owners();
                let current_unsubscribes = transport
                    .methods()
                    .iter()
                    .filter(|method| **method == "events.unsubscribe")
                    .count();
                assert_eq!((old_leases, current_unsubscribes), (0, 0));
                assert!(!replacement_screen.is_disposed());
                assert!(!replacement_exit.is_disposed());
                replacement_screen.dispose();
                replacement_exit.dispose();
                assert!(transport.flush().iter().all(Result::is_ok));
            });
        }
    }

    #[test]
    fn terminal_coverage_shares_confirmed_lease_and_rejects_other_topics() {
        Owner::new().with(|| {
            let (mut sdk, host, _) = fixture();
            allow_pty(&host);
            sdk.state.is_connected.set(true);
            sdk.state.connection_epoch.set(u64::MAX - 1);
            let transport = crate::context::tests::PtyHarness::new(sdk.state);
            wire_pty(&mut sdk, &transport);
            assert!(block_on(sdk.subscribe_terminal("session.updated")).is_err());
            let first = drive_pty(&transport, sdk.subscribe_terminal("pty.screen")).unwrap();
            let second = drive_pty(&transport, sdk.subscribe_terminal("pty.screen")).unwrap();
            assert_eq!(first.coverage_epoch(), Some(u64::MAX - 1));
            assert_eq!(second.coverage_epoch(), first.coverage_epoch());
            assert_eq!(transport.methods(), ["events.subscribe"]);
            first.dispose();
            assert!(transport.flush().is_empty());
            second.inner.enqueue(GatewayEvent {
                topic: "pty.screen".into(),
                data: json!({"seq":1}),
            });
            assert_eq!(second.try_drain().unwrap().len(), 1);
            sdk.state.connection_epoch.set(u64::MAX);
            assert!(second.try_drain().is_err());
            assert_eq!(
                transport.methods(),
                ["events.subscribe"],
                "terminal stream never installs reconnect work"
            );
            second.dispose();
            assert_eq!(transport.flush(), [Ok(())]);
            assert_eq!(
                transport.methods(),
                ["events.subscribe"],
                "stale lease never unsubscribes a replacement"
            );
        });
    }

    #[test]
    fn terminal_queue_reports_loss_without_followup_event() {
        Owner::new().with(|| {
            let (mut sdk, host, _) = fixture();
            allow_pty(&host);
            sdk.state.is_connected.set(true);
            let transport = crate::context::tests::PtyHarness::new(sdk.state);
            wire_pty(&mut sdk, &transport);
            for byte_bound in [false, true] {
                let subscription = drive_pty(&transport, sdk.subscribe_terminal("pty.screen")).unwrap();
                let event = |data| GatewayEvent {
                    topic: "pty.screen".into(),
                    data,
                };
                let fill = || {
                    if byte_bound {
                        // Serialized envelope is 32 bytes, including the string quotes.
                        let data = Value::String("x".repeat(8 * 1024 * 1024 - 32));
                        assert_eq!(
                            serde_json::to_vec(&json!({"topic":"pty.screen", "data":data}))
                                .unwrap()
                                .len(),
                            8 * 1024 * 1024
                        );
                        subscription.inner.enqueue(event(data));
                    } else {
                        for _ in 0..256 {
                            subscription.inner.enqueue(event(Value::Null));
                        }
                    }
                };
                fill();
                assert_eq!(
                    subscription.try_drain().unwrap().len(),
                    if byte_bound { 1 } else { 256 }
                );
                fill();
                subscription.inner.enqueue(event(Value::Null));
                assert!(
                    subscription.try_drain().is_err(),
                    "overflow is observable without another frame"
                );
                assert!(
                    subscription.try_drain().unwrap().is_empty(),
                    "never expose a partial queue after loss"
                );
                subscription.inner.enqueue(event(json!({"seq": 1})));
                assert_eq!(subscription.try_drain().unwrap().len(), 1);
                subscription.dispose();
                assert!(transport.flush().iter().all(Result::is_ok));
            }
            // Generic subscribers keep their pre-existing newest-256 behavior.
            let generic = block_on(sdk.subscribe("session.updated")).unwrap();
            for seq in 0..300 {
                generic.inner.enqueue(GatewayEvent {
                    topic: "session.updated".into(),
                    data: json!(seq),
                });
            }
            let drained = generic.drain();
            assert_eq!(drained.len(), 256);
            assert_eq!(drained[0].data, json!(44));
        });
    }

    #[test]
    fn successful_pty_readiness_accepts_current_coverage() {
        Owner::new().with(|| {
            let (mut sdk, host, _) = fixture();
            allow_pty(&host);
            sdk.state.is_connected.set(true);
            let transport = crate::context::tests::PtyHarness::new(sdk.state);
            wire_pty(&mut sdk, &transport);
            let lease = drive_pty(
                &transport,
                sdk.establish_pty_coverage("pty.screen", "events.subscribe:pty.screen"),
            )
            .unwrap();
            assert_eq!(lease.epoch(), 0);
            assert_eq!(transport.methods(), ["events.subscribe"]);
            drop(lease);
            assert_eq!(transport.flush(), [Ok(())]);
        });
    }

    #[test]
    fn reconnect_resubscription_records_epoch_after_success() {
        Owner::new().with(|| {
            let (mut sdk, host, _) = fixture();
            allow_pty(&host);
            sdk.state.is_connected.set(true);
            let transport = crate::context::tests::PtyHarness::new(sdk.state);
            wire_pty(&mut sdk, &transport);
            let subscription = pty_subscription_state(&sdk);
            sdk.state.connection_epoch.set(1);
            drive_pty(
                &transport,
                super::reestablish_pty_subscription(
                    Arc::clone(&subscription),
                    sdk.state,
                    "pty.screen".into(),
                ),
            );
            assert_eq!(transport.methods(), ["events.subscribe"]);
            assert_eq!(subscription.pty_epoch(), Some(1));
            subscription.dispose();
            assert_eq!(transport.flush(), [Ok(())]);
        });
    }

    #[test]
    fn stale_pty_completion_does_not_record_coverage() {
        Owner::new().with(|| {
            let (mut sdk, host, _) = fixture();
            allow_pty(&host);
            sdk.state.is_connected.set(true);
            let state = sdk.state;
            let calls = Arc::new(AtomicUsize::new(0));
            let recorded = calls.clone();
            let transport = crate::context::tests::PtyHarness::with_rpc(state, move |method, _| {
                if method == "events.subscribe" {
                    let call = recorded.fetch_add(1, Ordering::SeqCst) + 1;
                    state.connection_epoch.set(if call == 1 { 2 } else { 3 });
                }
                async { Ok(Value::Null) }.boxed_local()
            });
            wire_pty(&mut sdk, &transport);
            let subscription = pty_subscription_state(&sdk);
            sdk.state.connection_epoch.set(1);
            drive_pty(
                &transport,
                super::reestablish_pty_subscription(
                    Arc::clone(&subscription),
                    sdk.state,
                    "pty.screen".into(),
                ),
            );
            assert_eq!(calls.load(Ordering::SeqCst), 2);
            assert_eq!(subscription.pty_epoch(), None);
            subscription.dispose();
            assert!(transport.flush().iter().all(Result::is_ok));
        });
    }

    #[test]
    fn stale_retry_records_coverage_for_new_epoch() {
        Owner::new().with(|| {
            let (mut sdk, host, _) = fixture();
            allow_pty(&host);
            sdk.state.is_connected.set(true);
            let state = sdk.state;
            let calls = Arc::new(AtomicUsize::new(0));
            let recorded = calls.clone();
            let transport = crate::context::tests::PtyHarness::with_rpc(state, move |method, _| {
                if method == "events.subscribe" && recorded.fetch_add(1, Ordering::SeqCst) == 0 {
                    state.connection_epoch.set(2);
                }
                async { Ok(Value::Null) }.boxed_local()
            });
            wire_pty(&mut sdk, &transport);
            let subscription = pty_subscription_state(&sdk);
            sdk.state.connection_epoch.set(1);
            drive_pty(
                &transport,
                super::reestablish_pty_subscription(
                    Arc::clone(&subscription),
                    sdk.state,
                    "pty.screen".into(),
                ),
            );
            assert_eq!(calls.load(Ordering::SeqCst), 2);
            assert_eq!(subscription.pty_epoch(), Some(2));
            subscription.dispose();
            assert!(transport.flush().iter().all(Result::is_ok));
        });
    }

    #[test]
    fn failed_pty_coverage_is_retried_without_premature_epoch_latch() {
        Owner::new().with(|| {
            let (mut sdk, host, _) = fixture();
            allow_pty(&host);
            sdk.state.is_connected.set(true);
            let calls = Arc::new(AtomicUsize::new(0));
            let recorded = calls.clone();
            let transport = crate::context::tests::PtyHarness::with_rpc(sdk.state, move |method, _| {
                let fail = method == "events.subscribe" && recorded.fetch_add(1, Ordering::SeqCst) == 0;
                async move { if fail { Err("temporary coverage failure".into()) } else { Ok(Value::Null) } }.boxed_local()
            });
            wire_pty(&mut sdk, &transport);
            let subscription = pty_subscription_state(&sdk);
            sdk.state.connection_epoch.set(1);
            drive_pty(
                &transport,
                super::reestablish_pty_subscription(
                    Arc::clone(&subscription),
                    sdk.state,
                    "pty.screen".into(),
                ),
            );
            assert_eq!(calls.load(Ordering::SeqCst), 2);
            assert_eq!(subscription.pty_epoch(), Some(1));
            subscription.dispose();
            assert_eq!(transport.flush(), [Ok(())]);
        });
    }

    #[test]
    fn resubscribe_after_epoch_change_preserves_other_consumers() {
        Owner::new().with(|| {
            let (sdk, _, _) = fixture();
            let first = block_on(sdk.subscribe("session.updated")).unwrap();
            let second = block_on(sdk.subscribe("session.updated")).unwrap();
            first.clone().dispose();
            second.inner.enqueue(GatewayEvent {
                topic: "session.updated".into(),
                data: json!({"sequence": 1}),
            });
            assert_eq!(second.drain().len(), 1);
            assert!(!second.is_disposed());
        });
    }

    #[test]
    fn subscription_queue_is_bounded_and_disposed() {
        Owner::new().with(|| {
            let (sdk, _, _) = fixture();
            let subscription = block_on(sdk.subscribe("session.updated")).unwrap();
            for sequence in 0..300 {
                subscription.inner.enqueue(GatewayEvent {
                    topic: "session.updated".into(),
                    data: json!({"sequence": sequence}),
                });
            }
            let events = subscription.drain();
            assert_eq!(events.len(), 256);
            assert_eq!(events.first().unwrap().data, json!({"sequence": 44}));
            assert_eq!(events.last().unwrap().data, json!({"sequence": 299}));
            assert!(events.windows(2).all(|pair| {
                pair[1].data["sequence"].as_u64().unwrap()
                    == pair[0].data["sequence"].as_u64().unwrap() + 1
            }));
            subscription.clone().dispose();
            assert!(subscription.is_disposed());
            assert!(subscription.drain().is_empty());
        });
    }

    #[test]
    fn subscription_buffer_retains_newest_events_in_order() {
        Owner::new().with(|| {
            let (sdk, _, _) = fixture();
            let subscription = block_on(sdk.subscribe("session.updated")).unwrap();
            for sequence in 0..70 {
                subscription.inner.enqueue(GatewayEvent {
                    topic: "session.updated".into(),
                    data: json!({"sequence": sequence}),
                });
            }
            let events = subscription.drain();
            assert_eq!(events.len(), 70);
            assert_eq!(events.first().unwrap().data, json!({"sequence": 0}));
            assert_eq!(events.last().unwrap().data, json!({"sequence": 69}));
            assert!(events.windows(2).all(|pair| {
                pair[1].data["sequence"].as_u64().unwrap()
                    == pair[0].data["sequence"].as_u64().unwrap() + 1
            }));
            assert!(subscription.drain().is_empty());
            assert!(!subscription.is_disposed());
        });
    }

    #[test]
    fn owner_cleanup_disposes_sdk_and_subscriptions() {
        let owner = Owner::new();
        let (sdk, _, calls) = owner.with(fixture);
        let subscription = block_on(sdk.subscribe("session.updated")).unwrap();
        owner.cleanup();
        assert!(subscription.is_disposed());
        assert_eq!(
            block_on(sdk.invoke("sessions.list", json!({}))),
            Err(PanelSdkError::Disposed.to_string())
        );
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn missing_host_context_is_denied() {
        Owner::new().with(|| {
            let sdk = PanelSdk::new(DashboardState::new(), key("home"));
            assert_eq!(
                block_on(sdk.invoke("sessions.list", json!({}))),
                Err(PanelSdkError::Denied.to_string())
            );
        });
    }

    #[test]
    fn mount_transitions_dispose_only_ui_and_reject_late_mounts() {
        let count = Arc::new(AtomicUsize::new(0));
        let handle = || {
            let count = count.clone();
            PanelModuleHandle::new(move || {
                count.fetch_add(1, Ordering::SeqCst);
            })
        };
        let mut state = PanelMountState::Idle;
        assert!(state.begin_load(key("home")));
        assert!(state.mount(key("home"), handle()));
        state.unmount();
        state.unmount();
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(!state.mount(key("home"), handle()));
        assert_eq!(count.load(Ordering::SeqCst), 2);
        assert!(state.begin_load(key("home")));
        assert!(!state.fail(&key("other"), "stale".into()));
        assert!(state.fail(&key("home"), "import failed".into()));
        assert!(matches!(state, PanelMountState::Failed { .. }));
        assert!(state.begin_load(key("home")));
        assert!(state.mount(key("home"), handle()));
        state.disable();
        assert_eq!(count.load(Ordering::SeqCst), 3);
        assert!(!state.begin_load(key("home")));
        state.enable();
        assert!(state.begin_load(key("home")));
    }
}
