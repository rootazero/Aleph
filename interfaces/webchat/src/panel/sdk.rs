//! Opaque host adapter. Browser modules receive only the narrow SDK ABI, never
//! this adapter's private DashboardState or Shell context. Same-origin code is
//! trusted/verified code, not sandboxed by these permission checks.

use super::PanelHostState;
use crate::appearance::{Accent, ThemeMode};
use crate::context::{DashboardState, GatewayEvent, PtyTopicLease};
use aleph_protocol::panel::{PanelTabDescriptor, PanelTabKey, PanelTabStatus};
use futures::future::LocalBoxFuture;
use futures::FutureExt;
#[cfg(any(target_arch = "wasm32", test))]
use leptos::prelude::Update;
use leptos::prelude::{on_cleanup, use_context, Effect, Get, GetUntracked, Set};
use leptos::task::spawn_local;
use serde_json::Value;
use std::collections::VecDeque;
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
type HostPtySubscribe = dyn Fn(String) -> LocalBoxFuture<'static, Result<PtyTopicLease, String>> + Send + Sync;

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
            pty_subscribe: Arc::new(move |topic| {
                async move { state.acquire_pty_topic(&topic).await }.boxed_local()
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

    #[cfg(any(target_arch = "wasm32", test))]
    pub(super) fn asset_project_context(&self) -> Option<(u64, u64)> {
        let state = self.state.panel_discovery.try_get_untracked()?;
        matches!(
            state.status(),
            crate::panel::discovery::PanelDiscoveryStatus::Ready { .. }
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

    // The installer is the executor seam; registration and lease ownership stay
    // on this one production path, including native tests without a global runtime.
    fn subscribe_with_observer(
        &self,
        topic: &str,
        observe: impl Fn(&Arc<SubscriptionState>, String) + 'static,
    ) -> LocalBoxFuture<'static, Result<PanelSubscription, String>> {
        let sdk = self.clone();
        let topic = topic.to_string();
        async move {
            sdk.authorize().map_err(|error| error.to_string())?;
            if topic.is_empty() || topic.contains('*') {
                return Err(PanelSdkError::Denied.to_string());
            }
            let permission = format!("events.subscribe:{topic}");
            sdk.require_permission(&permission)
                .map_err(|error| error.to_string())?;
            let pty_topic = is_pty_topic(&topic);
            let pty_coverage = if pty_topic {
                // PTY coverage is mounted by this consumer. The await is part of
                // subscribe's readiness contract, so a caller never receives a
                // local listener before the host has accepted the topic on the
                // current connection epoch.
                Some(sdk.establish_pty_coverage(&topic, &permission).await?)
            } else {
                None
            };
            let inner = Arc::new(SubscriptionState {
                sdk: sdk.clone(),
                permission,
                resources: Mutex::new(SubscriptionResources {
                    cleanup: None,
                    pty_coverage,
                    pending: VecDeque::new(),
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
            if pty_topic {
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

    async fn establish_pty_coverage(&self, topic: &str, permission: &str) -> Result<PtyTopicLease, String> {
        // One retry is specific to the PTY readiness boundary: a failed host
        // request or a reply superseded by reconnect must not be mistaken for
        // coverage, while an endlessly flapping socket must not create a loop.
        const ATTEMPTS: usize = 2;
        let mut last_error = None;
        for _ in 0..ATTEMPTS {
            self.authorize().map_err(|error| error.to_string())?;
            self.require_permission(permission)
                .map_err(|error| error.to_string())?;
            let coverage = match (self.pty_subscribe)(topic.to_string()).await {
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
}

struct SubscriptionState {
    sdk: PanelSdk,
    permission: String,
    resources: Mutex<SubscriptionResources>,
}

impl SubscriptionState {
    fn pty_epoch(&self) -> Option<u64> {
        self.resources.lock().unwrap().pty_coverage.as_ref().map(PtyTopicLease::epoch)
    }

    fn is_disposed(&self) -> bool {
        self.resources.lock().unwrap().cleanup.is_none()
    }

    fn enqueue(&self, event: GatewayEvent) {
        if !self.authorize() {
            return;
        }
        let mut resources = self.resources.lock().unwrap();
        if resources.cleanup.is_some() {
            if resources.pending.len() == SUBSCRIPTION_PENDING_CAPACITY {
                resources.pending.pop_front();
            }
            resources.pending.push_back(event);
        }
    }

    fn authorize(&self) -> bool {
        if self.is_disposed() { return false; }
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
/// dropping the last clone also releases the listener. The pending buffer keeps
/// at most 256 events, dropping the oldest on overflow. PTY upstream coverage is
/// a lossy notification subscription; sequence-gap recovery belongs to Task 2.
/// Revoked authority clears the buffer and releases local and leased resources.
#[derive(Clone)]
pub struct PanelSubscription {
    inner: Arc<SubscriptionState>,
}

impl PanelSubscription {
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
    use crate::panel::registry::TabRegistry;
    use crate::panel::{PanelHostState, PanelModuleHandle, PanelMountState};
    use aleph_protocol::panel::{
        PanelTabDescriptor, PanelTabKey, PanelTabStatus, PanelUiTrust, PANEL_UI_API_MAJOR,
    };
    use futures::{executor::block_on, FutureExt};
    use leptos::prelude::{provide_context, GetUntracked, Owner, Set, Update};
    use serde_json::{json, Value};
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
            resources: Mutex::new(super::SubscriptionResources {
                cleanup: Some(Box::new(|| {})),
                pty_coverage: None,
                pending: std::collections::VecDeque::new(),
            }),
        })
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
                sdk.pty_subscribe = Arc::new(move |topic| {
                    let io = io.clone();
                    async move { state.acquire_pty_topic_with_io(&topic, io).await }.boxed_local()
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
            sdk.pty_subscribe = Arc::new(move |topic| {
                let io = io.clone();
                async move { state.acquire_pty_topic_with_io(&topic, io).await }.boxed_local()
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
        sdk.pty_subscribe = Arc::new(move |topic| {
            let io = io.clone();
            async move { state.acquire_pty_topic_with_io(&topic, io).await }.boxed_local()
        });
    }

    fn drive_pty<T>(transport: &crate::context::tests::PtyHarness, future: impl std::future::Future<Output = T>) -> T {
        let mut future = Box::pin(future);
        loop {
            if let Some(result) = future.as_mut().now_or_never() { return result; }
            let results = transport.flush();
            assert!(!results.is_empty(), "pending coverage has owned executor work");
            assert!(results.iter().all(Result::is_ok));
        }
    }

    #[test]
    fn successful_pty_readiness_accepts_current_coverage() {
        Owner::new().with(|| {
            let (mut sdk, host, _) = fixture();
            allow_pty(&host);
            sdk.state.is_connected.set(true);
            let transport = crate::context::tests::PtyHarness::new(sdk.state);
            wire_pty(&mut sdk, &transport);
            let lease = drive_pty(&transport, sdk.establish_pty_coverage("pty.screen", "events.subscribe:pty.screen")).unwrap();
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
            drive_pty(&transport, super::reestablish_pty_subscription(Arc::clone(&subscription), sdk.state, "pty.screen".into()));
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
            drive_pty(&transport, super::reestablish_pty_subscription(Arc::clone(&subscription), sdk.state, "pty.screen".into()));
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
            drive_pty(&transport, super::reestablish_pty_subscription(Arc::clone(&subscription), sdk.state, "pty.screen".into()));
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
            drive_pty(&transport, super::reestablish_pty_subscription(Arc::clone(&subscription), sdk.state, "pty.screen".into()));
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
