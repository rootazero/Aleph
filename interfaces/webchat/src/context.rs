use crate::api::ExecApprovalApi;
use crate::components::sidebar::SystemAlert;
use crate::state::notifications::{IncomingPairing, PendingApprovalView};
use futures::channel::{mpsc, oneshot};
use futures::{FutureExt, StreamExt};
use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::Value;
use shared_ui_logic::connection::connector::AlephConnector;
use shared_ui_logic::connection::wasm::WasmConnector;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

#[cfg(target_arch = "wasm32")]
fn get_local_storage(key: &str) -> Option<String> {
    web_sys::window()?
        .local_storage()
        .ok()??
        .get_item(key)
        .ok()?
}

#[cfg(target_arch = "wasm32")]
fn set_local_storage(key: &str, value: &str) {
    if let Some(storage) = web_sys::window()
        .and_then(|w| w.local_storage().ok())
        .flatten()
    {
        let _ = storage.set_item(key, value);
    }
}

#[cfg(target_arch = "wasm32")]
fn remove_local_storage(key: &str) {
    if let Some(storage) = web_sys::window()
        .and_then(|w| w.local_storage().ok())
        .flatten()
    {
        let _ = storage.remove_item(key);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn get_local_storage(_key: &str) -> Option<String> {
    None
}

#[cfg(not(target_arch = "wasm32"))]
fn set_local_storage(_key: &str, _value: &str) {}

#[cfg(not(target_arch = "wasm32"))]
fn remove_local_storage(_key: &str) {}

/// State carried by the PairingModal while the wizard handshake is in flight.
#[derive(Debug, Clone, Default)]
pub struct PairingPrompt {
    /// wizard.start session_id (set after wizard.start returns)
    pub session_id: Option<String>,
    /// Pairing code extracted from the wizard confirm step message
    pub pairing_code: Option<String>,
    /// Non-fatal warning shown inside the modal
    pub last_error: Option<String>,
}

// RPC request sent to the message loop
struct RpcRequest {
    id: String,
    method: String,
    params: Value,
    response_tx: oneshot::Sender<Result<Value, String>>,
}

// Event received from Gateway
#[derive(Clone, Debug)]
pub struct GatewayEvent {
    pub topic: String,
    pub data: Value,
}

// Event handler callback type
type EventHandler = Arc<dyn Fn(GatewayEvent) + Send + Sync>;

/// Pure predicate mirroring the gateway's `tier::role_for_permissions`
/// classification: only the literal `"operator"` role (config tier) grants
/// control-plane access. Extracted for host-side unit testing.
pub(crate) fn role_is_operator(role: Option<&str>) -> bool {
    role == Some("operator")
}

#[derive(Clone, Copy)]
pub struct DashboardState {
    pub is_connected: RwSignal<bool>,
    pub reconnect_count: RwSignal<u32>,
    pub gateway_url: RwSignal<String>,
    pub connection_error: RwSignal<Option<String>>,
    pub is_reconnecting: RwSignal<bool>,
    /// Latched true on the first successful authenticate; never reset.
    /// Lets the boot gate disengage and the service gate engage — two
    /// surfaces that differ only in "have we ever been live?".
    pub has_connected_once: RwSignal<bool>,

    // Phase 3: Channel to send RPC requests to message loop
    rpc_tx: StoredValue<Option<mpsc::UnboundedSender<RpcRequest>>>,
    next_id: StoredValue<Arc<Mutex<u64>>>,

    // Phase 3: Event handling
    event_handlers: StoredValue<Arc<Mutex<Vec<EventHandler>>>>,

    // Channel for stopping the message loop
    disconnect_tx: StoredValue<Option<oneshot::Sender<()>>>,

    /// System alert state bus
    pub alerts: RwSignal<HashMap<String, SystemAlert>>,

    /// Alert subscription ID for cleanup
    alert_subscription_id: StoredValue<Option<usize>>,

    /// Pending browser-pairing requests rendered by the NotificationCenter
    /// with inline Approve / Reject buttons. Sourced from `pairing.**`
    /// gateway events (see `setup_pairing_subscriptions`).
    pub incoming_pairings: RwSignal<Vec<IncomingPairing>>,

    /// Pairing subscription ID for cleanup
    pairing_subscription_id: StoredValue<Option<usize>>,

    /// Pending operator-approval requests rendered by the NotificationCenter
    /// with inline allow-once / allow-session / deny buttons. Sourced from the
    /// `exec.approvals.pending` RPC; `approval.**` events trigger a refetch
    /// (see `setup_approval_subscriptions`).
    pub pending_approvals: RwSignal<Vec<PendingApprovalView>>,

    /// Approval subscription ID for cleanup.
    approval_subscription_id: StoredValue<Option<usize>>,

    /// Feature flag: enable radial (TheBrain-style) navigation in the Canvas view.
    /// Initialized from localStorage; mutated by the Settings panel toggle.
    pub canvas_radial_navigation: RwSignal<bool>,

    /// Set to Some(_) when auth.connect returns pairing_required.
    /// Cleared automatically after a successful reconnect.
    pub pairing_required: RwSignal<Option<PairingPrompt>>,

    /// Connection role captured from the `connect` response: `Some("operator")`
    /// (config tier) or `Some("guest")` (chat tier); `None` before the first
    /// successful authenticate. Read by `is_operator()` to gate operator-only
    /// settings surfaces (e.g. cluster management) client-side.
    pub role: RwSignal<Option<String>>,
}

/// Derive the Gateway WebSocket URL from the current page location.
/// Since the Panel UI and Gateway share the same port, we use same-origin.
fn derive_gateway_url() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(window) = web_sys::window() {
            let location = window.location();
            if let (Ok(protocol), Ok(host)) = (location.protocol(), location.host()) {
                let ws_protocol = if protocol == "https:" { "wss:" } else { "ws:" };
                return format!("{}//{}/ws", ws_protocol, host);
            }
        }
        "ws://127.0.0.1:18790/ws".to_string()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        "ws://127.0.0.1:18790/ws".to_string()
    }
}

impl Default for DashboardState {
    fn default() -> Self {
        Self::new()
    }
}

impl DashboardState {
    pub fn new() -> Self {
        Self {
            is_connected: RwSignal::new(false),
            reconnect_count: RwSignal::new(0),
            gateway_url: RwSignal::new(derive_gateway_url()),
            connection_error: RwSignal::new(None),
            is_reconnecting: RwSignal::new(false),
            has_connected_once: RwSignal::new(false),
            rpc_tx: StoredValue::new(None),
            next_id: StoredValue::new(Arc::new(Mutex::new(1))),
            event_handlers: StoredValue::new(Arc::new(Mutex::new(Vec::new()))),
            disconnect_tx: StoredValue::new(None),
            alerts: RwSignal::new(HashMap::new()),
            alert_subscription_id: StoredValue::new(None),
            incoming_pairings: RwSignal::new(Vec::new()),
            pairing_subscription_id: StoredValue::new(None),
            pending_approvals: RwSignal::new(Vec::new()),
            approval_subscription_id: StoredValue::new(None),
            canvas_radial_navigation: RwSignal::new(
                crate::api::settings::load_canvas_radial_navigation(),
            ),
            pairing_required: RwSignal::new(None),
            role: RwSignal::new(None),
        }
    }

    /// Reactive predicate: did this connection authenticate as `operator`
    /// (config tier)? Consults the `role` captured from the `connect` response.
    /// Returns false before the first successful authenticate. Used by
    /// operator-only settings surfaces to gate UI up front.
    pub fn is_operator(&self) -> bool {
        role_is_operator(self.role.get().as_deref())
    }

    /// Capture the connection `role` from a `connect` response into the `role`
    /// signal. No-op fields (missing `role`) reset to `None`, keeping the
    /// signal consistent across reconnects.
    fn capture_role(&self, resp: &Value) {
        self.role
            .set(resp.get("role").and_then(|r| r.as_str()).map(String::from));
    }

    /// Subscribe to Gateway events
    /// Returns a subscription ID that can be used to unsubscribe
    pub fn subscribe_events<F>(&self, handler: F) -> usize
    where
        F: Fn(GatewayEvent) + Send + Sync + 'static,
    {
        let handlers = self.event_handlers.with_value(|h| h.clone());
        let mut handlers = handlers.lock().expect("event handlers mutex poisoned");
        let id = handlers.len();
        handlers.push(Arc::new(handler));
        id
    }

    /// Unsubscribe from events
    pub fn unsubscribe_events(&self, id: usize) {
        let handlers = self.event_handlers.with_value(|h| h.clone());
        let mut handlers = handlers.lock().expect("event handlers mutex poisoned");
        if id < handlers.len() {
            // Replace with a no-op handler instead of removing to preserve indices
            handlers[id] = Arc::new(|_| {});
        }
    }

    /// Update alert state
    pub fn update_alert(&self, key: String, alert: SystemAlert) {
        self.alerts.update(|map| {
            map.insert(key, alert);
        });
    }

    /// Get alert state
    pub fn get_alert(&self, key: &str) -> Option<SystemAlert> {
        self.alerts.with(|map| map.get(key).cloned())
    }

    /// Clear alert state
    pub fn clear_alert(&self, key: &str) {
        self.alerts.update(|map| {
            map.remove(key);
        });
    }

    /// Dispatch event to all subscribers
    fn dispatch_event(&self, event: GatewayEvent) {
        let handlers = self.event_handlers.with_value(|h| h.clone());
        let handlers = handlers.lock().expect("event handlers mutex poisoned");
        for handler in handlers.iter() {
            handler(event.clone());
        }
    }

    /// Subscribe to a specific event topic on the Gateway
    pub async fn subscribe_topic(&self, pattern: &str) -> Result<(), String> {
        self.rpc_call(
            "events.subscribe",
            serde_json::json!({
                "topics": [pattern]
            }),
        )
        .await?;
        Ok(())
    }

    /// Unsubscribe from an event topic
    pub async fn unsubscribe_topic(&self, pattern: &str) -> Result<(), String> {
        self.rpc_call(
            "events.unsubscribe",
            serde_json::json!({
                "topics": [pattern]
            }),
        )
        .await?;
        Ok(())
    }

    /// Make an RPC call to the gateway
    pub async fn rpc_call(&self, method: &str, params: Value) -> Result<Value, String> {
        // Generate unique ID
        let id = {
            let next_id = self.next_id.with_value(|n| n.clone());
            let mut id_gen = next_id.lock().expect("RPC ID generator mutex poisoned");
            let id = *id_gen;
            *id_gen += 1;
            id.to_string()
        };

        // Create oneshot channel for response
        let (response_tx, response_rx) = oneshot::channel();

        // Create RPC request
        let request = RpcRequest {
            id,
            method: method.to_string(),
            params,
            response_tx,
        };

        // Send request to message loop
        {
            let rpc_tx = self.rpc_tx.with_value(|tx| tx.clone());
            if let Some(tx) = rpc_tx {
                tx.unbounded_send(request)
                    .map_err(|_| "Failed to send RPC request".to_string())?;
            } else {
                return Err("Not connected".to_string());
            }
        }

        // Wait for response
        response_rx
            .await
            .map_err(|_| "Response channel closed".to_string())?
    }

    /// Authenticate with the gateway after WebSocket connection is established
    async fn authenticate(&self) -> Result<(), String> {
        // Auth is delivered via the `aleph_session` HttpOnly cookie set by
        // `/auth/bootstrap` (loopback handoff from the desktop shell) or
        // `/auth/bootstrap/from_pairing` (cold-browser pairing). The Phase 1
        // `?token=` URL-param fallback was removed in Phase 4 — the cookie
        // is now the only inbound auth surface for the Panel.

        // Try stored device token first
        if let Some(token) = get_local_storage("aleph_device_token") {
            let result = self
                .rpc_call(
                    "connect",
                    serde_json::json!({
                        "token": token,
                        "device_name": "Web Panel"
                    }),
                )
                .await;

            if let Ok(resp) = result {
                // Update stored token if a new one was issued
                if let Some(new_token) = resp.get("token").and_then(|t| t.as_str()) {
                    set_local_storage("aleph_device_token", new_token);
                }
                self.capture_role(&resp);
                return Ok(());
            }
            // Token invalid, clear it and try shared token
            remove_local_storage("aleph_device_token");
            web_sys::console::log_1(&"Device token invalid, trying shared token...".into());
        }

        // Try shared token from localStorage (set during login page)
        if let Some(token) = get_local_storage("aleph_shared_token") {
            let result = self
                .rpc_call(
                    "connect",
                    serde_json::json!({
                        "shared_token": token,
                        "device_name": "Web Panel"
                    }),
                )
                .await;

            match result {
                Ok(resp) => {
                    // Store device token for future use
                    if let Some(device_token) = resp.get("token").and_then(|t| t.as_str()) {
                        set_local_storage("aleph_device_token", device_token);
                    }
                    self.capture_role(&resp);
                    return Ok(());
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Shared token auth failed: {}", e).into());
                    // Clear invalid shared token
                    remove_local_storage("aleph_shared_token");
                }
            }
        }

        // No token available — try a plain connect (works when auth_mode is "none")
        let result = self
            .rpc_call(
                "connect",
                serde_json::json!({
                    "device_name": "Web Panel"
                }),
            )
            .await;

        match result {
            Ok(resp) => {
                if let Some(token) = resp.get("token").and_then(|t| t.as_str()) {
                    set_local_storage("aleph_device_token", token);
                }
                self.capture_role(&resp);
                Ok(())
            }
            Err(e) if e == "pairing_required" => {
                // Gateway requires device pairing — trigger the PairingModal.
                self.pairing_required.set(Some(PairingPrompt::default()));
                Err(e)
            }
            Err(_) => {
                // Auth required but no token — redirect to login
                Err("Authentication required".to_string())
            }
        }
    }

    /// Persist a device token and trigger reconnect after successful pairing.
    ///
    /// Called by PairingModal once `wizard.next` returns `done` with a token.
    pub fn set_pairing_token(&self, token: String) {
        set_local_storage("aleph_device_token", &token);
        // Clear the modal
        self.pairing_required.set(None);
    }

    /// Connect to the gateway
    pub async fn connect(&self) -> Result<(), String> {
        let url = self.gateway_url.get();
        let mut connector = WasmConnector::new();

        match connector.connect(&url).await {
            Ok(()) => {
                // Get the message stream
                let stream = connector.receive();

                // Create channels
                let (rpc_tx, rpc_rx) = mpsc::unbounded::<RpcRequest>();
                let (disconnect_tx, disconnect_rx) = oneshot::channel::<()>();

                // Store channels
                self.rpc_tx.set_value(Some(rpc_tx));
                self.disconnect_tx.set_value(Some(disconnect_tx));

                // Clone state for message loop
                let state = *self;

                // Spawn message loop task that owns the connector
                spawn_local(async move {
                    web_sys::console::log_1(&"Message loop started".into());

                    let mut stream = stream.fuse();
                    let mut rpc_rx = rpc_rx.fuse();
                    let mut disconnect_rx = disconnect_rx.fuse();
                    let mut pending_rpcs: HashMap<String, oneshot::Sender<Result<Value, String>>> =
                        HashMap::new();
                    // Track whether the loop exited because of an explicit
                    // disconnect() call (no auto-reconnect) or an unintentional
                    // drop (auto-reconnect to drive ConnectionPhase::Reconnecting
                    // → ServiceBlockingGate). Captured drop_reason becomes the
                    // connection_error surfaced in the UI.
                    let mut intentional_close = false;
                    let mut drop_reason: Option<String> = None;

                    loop {
                        // Use futures::select! to handle multiple async operations
                        futures::select! {
                            // Handle incoming RPC requests
                            rpc_req = rpc_rx.select_next_some() => {
                                // Build JSON-RPC request
                                let request = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": rpc_req.id.clone(),
                                    "method": rpc_req.method,
                                    "params": rpc_req.params
                                });

                                // Send request
                                match connector.send(request).await {
                                    Ok(()) => {
                                        // Store pending request
                                        pending_rpcs.insert(rpc_req.id, rpc_req.response_tx);
                                    }
                                    Err(e) => {
                                        web_sys::console::error_1(&format!("Failed to send RPC: {:?}", e).into());
                                        let _ = rpc_req.response_tx.send(Err(e.to_string()));
                                    }
                                }
                            }

                            // Handle incoming WebSocket messages
                            msg = stream.select_next_some() => {
                                match msg {
                                    Ok(value) => {
                                        web_sys::console::log_1(&format!("Received message: {:?}", value).into());

                                        // Check if this is an RPC response (has 'id' field)
                                        if let Some(id) = value.get("id").and_then(|id| id.as_str()) {
                                            // Handle RPC response
                                            if let Some(tx) = pending_rpcs.remove(id) {
                                                if let Some(error) = value.get("error") {
                                                    let msg = error.get("message")
                                                        .and_then(|m| m.as_str())
                                                        .unwrap_or("Unknown error");
                                                    let _ = tx.send(Err(msg.to_string()));
                                                } else if let Some(result) = value.get("result") {
                                                    let _ = tx.send(Ok(result.clone()));
                                                }
                                            }
                                        } else {
                                            // This is an event notification
                                            // Parse event format: { "method": "event", "params": { "topic": "...", "data": {...} } }
                                            if let Some(method) = value.get("method").and_then(|m| m.as_str()) {
                                                if method == "event" {
                                                    if let Some(params) = value.get("params") {
                                                        if let Some(topic) = params.get("topic").and_then(|t| t.as_str()) {
                                                            let data = params.get("data").cloned().unwrap_or(Value::Null);

                                                            let event = GatewayEvent {
                                                                topic: topic.to_string(),
                                                                data,
                                                            };

                                                            web_sys::console::log_1(&format!("Event: {} - {:?}", event.topic, event.data).into());

                                                            // Dispatch event to subscribers
                                                            state.dispatch_event(event);
                                                        }
                                                    }
                                                } else if method.starts_with("stream.") {
                                                    // Gateway sends streaming events as {method: "stream.run_accepted", params: {...StreamEvent...}}
                                                    // Convert to GatewayEvent with run.* topic for subscriber filtering
                                                    let data = value.get("params").cloned().unwrap_or(Value::Null);
                                                    let topic = method.replacen("stream.", "run.", 1);
                                                    let event = GatewayEvent {
                                                        topic,
                                                        data,
                                                    };
                                                    web_sys::console::log_1(&format!("Stream event: {} - {:?}", event.topic, event.data).into());
                                                    state.dispatch_event(event);
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        web_sys::console::error_1(&format!("Message loop error: {:?}", e).into());
                                        drop_reason = Some(format!("WebSocket dropped: {}", e));
                                        break;
                                    }
                                }
                            }

                            // Handle disconnect signal
                            _ = disconnect_rx => {
                                web_sys::console::log_1(&"Disconnect signal received".into());
                                let _ = connector.disconnect().await;
                                intentional_close = true;
                                break;
                            }

                            // If all channels are closed, exit (graceful close
                            // by remote, or sender side dropped). Treated as
                            // unintentional — auto-reconnect handles it below.
                            complete => {
                                drop_reason = Some(
                                    "WebSocket closed (channels exhausted)".to_string(),
                                );
                                break;
                            }
                        }
                    }

                    web_sys::console::log_1(&"Message loop stopped".into());

                    // Unintentional drop: flip is_connected so ConnectionPhase
                    // re-derives, then kick off reconnect() from a fresh task
                    // so ServiceBlockingGate engages after the 5-attempt
                    // budget exhausts. We intentionally do NOT set
                    // connection_error here — reconnect() sets it on final
                    // failure, and setting it now would make the chip flash
                    // "Failed" during the retry window (the derive rule treats
                    // any error as terminal). The drop_reason is logged for
                    // dev-console debugging.
                    if !intentional_close {
                        if let Some(reason) = drop_reason.as_deref() {
                            web_sys::console::warn_1(
                                &format!(
                                    "WS dropped unintentionally; auto-reconnecting. reason={}",
                                    reason
                                )
                                .into(),
                            );
                        }
                        state.is_connected.set(false);
                        // Clear the dead rpc_tx so the next rpc_call() won't
                        // block on a sender whose receiver task just exited.
                        state.rpc_tx.set_value(None);
                        spawn_local(async move {
                            let _ = state.reconnect().await;
                        });
                    }
                });

                // Authenticate before marking as connected
                let auth_state = *self;
                let auth_result = auth_state.authenticate().await;
                match auth_result {
                    Ok(()) => {
                        self.is_connected.set(true);
                        self.connection_error.set(None);
                        self.reconnect_count.set(0);
                        self.is_reconnecting.set(false);
                        self.has_connected_once.set(true);

                        // Subscribe to config events automatically
                        let state_for_subscribe = *self;
                        spawn_local(async move {
                            if let Err(e) = state_for_subscribe.subscribe_topic("config.**").await {
                                web_sys::console::error_1(
                                    &format!("Failed to subscribe to config events: {}", e).into(),
                                );
                            }
                        });

                        Ok(())
                    }
                    Err(ref e) if e == "pairing_required" => {
                        // Stay connected at WS level; PairingModal will drive the
                        // wizard.* handshake and call reconnect() when done.
                        Err(e.clone())
                    }
                    Err(e) => {
                        // Auth failed — redirect to the browser-pairing
                        // page (the legacy `/login` token-paste form was
                        // removed in Phase 4 of the auth UX overhaul).
                        #[cfg(target_arch = "wasm32")]
                        {
                            if let Some(window) = web_sys::window() {
                                let _ = window.location().set_href("/pair");
                            }
                        }
                        Err(e)
                    }
                }
            }
            Err(e) => {
                self.is_connected.set(false);
                let error_msg = e.to_string();
                self.connection_error.set(Some(error_msg.clone()));
                Err(error_msg)
            }
        }
    }

    /// Disconnect from the gateway
    pub async fn disconnect(&self) -> Result<(), String> {
        // Cleanup alert subscriptions first
        self.cleanup_alert_subscriptions();

        // Send disconnect signal to message loop (take ownership)
        let mut tx_opt = None;
        self.disconnect_tx.update_value(|v| {
            tx_opt = v.take();
        });
        if let Some(tx) = tx_opt {
            let _ = tx.send(());
        }

        // Clear RPC channel
        self.rpc_tx.set_value(None);

        // Update state
        self.is_connected.set(false);
        self.connection_error.set(None);
        self.is_reconnecting.set(false);
        Ok(())
    }

    /// Attempt to reconnect with exponential backoff
    pub async fn reconnect(&self) -> Result<(), String> {
        let max_attempts = 5;

        self.is_reconnecting.set(true);

        for attempt in 0..max_attempts {
            self.reconnect_count.set(attempt);

            // Exponential backoff: 1s, 2s, 4s, 8s, 16s
            let delay_ms = (1000 * 2_u32.pow(attempt)).min(16000);

            web_sys::console::log_1(
                &format!("Reconnecting in {}ms (attempt {})", delay_ms, attempt + 1).into(),
            );

            TimeoutFuture::new(delay_ms).await;

            match self.connect().await {
                Ok(()) => {
                    web_sys::console::log_1(&"Reconnection successful".into());
                    self.is_reconnecting.set(false);
                    return Ok(());
                }
                Err(e) => {
                    web_sys::console::error_1(
                        &format!("Reconnection attempt {} failed: {}", attempt + 1, e).into(),
                    );

                    if attempt + 1 >= max_attempts {
                        let error_msg =
                            format!("Failed to reconnect after {} attempts", max_attempts);
                        self.connection_error.set(Some(error_msg.clone()));
                        self.is_reconnecting.set(false);
                        return Err(error_msg);
                    }
                }
            }
        }

        self.is_reconnecting.set(false);
        Err("Reconnection failed".to_string())
    }

    /// Setup alert subscriptions
    ///
    /// This method subscribes to alert-related events from the Gateway and
    /// updates the DashboardState.alerts HashMap when events arrive.
    /// It also fetches initial alert states on mount.
    pub async fn setup_alert_subscriptions(&self) -> Result<(), String> {
        // Subscribe to alert events on the Gateway
        self.subscribe_topic("alerts.**").await?;

        web_sys::console::log_1(&"Subscribed to alerts.** events".into());

        // Load initial alert states
        let state_for_init = *self;
        spawn_local(async move {
            if let Err(e) = state_for_init.load_initial_alerts().await {
                web_sys::console::error_1(&format!("Failed to load initial alerts: {}", e).into());
            }
        });

        // Setup event handler for alert events
        let state = *self;
        let subscription_id = self.subscribe_events(move |event: GatewayEvent| {
            web_sys::console::log_1(
                &format!("Alert event received: {} - {:?}", event.topic, event.data).into(),
            );

            // Parse alert data and update state
            if event.topic.starts_with("alerts.") {
                // Extract alert type from topic (e.g., "alerts.system.health" -> "system.health")
                let alert_key = event.topic.strip_prefix("alerts.").unwrap_or(&event.topic);

                // Parse alert data
                if let Some(severity) = event.data.get("severity").and_then(|s| s.as_str()) {
                    let level = match severity {
                        "info" => crate::components::sidebar::AlertLevel::Info,
                        "warning" => crate::components::sidebar::AlertLevel::Warning,
                        "error" | "critical" => crate::components::sidebar::AlertLevel::Critical,
                        _ => {
                            web_sys::console::warn_1(
                                &format!("Unknown alert severity: {}", severity).into(),
                            );
                            crate::components::sidebar::AlertLevel::None
                        }
                    };

                    let count = event
                        .data
                        .get("count")
                        .and_then(|c| c.as_u64())
                        .map(|c| c as u32);

                    let message = event
                        .data
                        .get("message")
                        .and_then(|m| m.as_str())
                        .map(|s| s.to_string());

                    // Create SystemAlert with String key (no memory leak)
                    let alert = crate::components::sidebar::SystemAlert {
                        key: alert_key.to_string(),
                        level,
                        count,
                        message,
                    };

                    // Update alert state
                    state.update_alert(alert.key.clone(), alert);
                } else {
                    // If no severity, clear the alert
                    web_sys::console::warn_1(
                        &format!("Alert event missing severity field: {}", event.topic).into(),
                    );
                    state.clear_alert(alert_key);
                }
            }
        });

        // Store subscription ID for cleanup
        self.alert_subscription_id.set_value(Some(subscription_id));

        Ok(())
    }

    /// Subscribe to `pairing.**` events so the NotificationCenter can
    /// render inline Approve / Reject cards for cold-browser pairings.
    ///
    /// Mirrors `setup_alert_subscriptions` — wildcard topic subscribe,
    /// then a typed handler that mutates `incoming_pairings`.
    pub async fn setup_pairing_subscriptions(&self) -> Result<(), String> {
        self.subscribe_topic("pairing.**").await?;
        web_sys::console::log_1(&"Subscribed to pairing.** events".into());

        let state = *self;
        let subscription_id = self.subscribe_events(move |event: GatewayEvent| {
            let topic = event.topic.as_str();
            // The new browser-pairing flow emits the device_name field on
            // the `pairing.requested` event with the origin_label string
            // (see handle_pairing_start_browser → PairingRequested frame).
            // We don't have a 6-digit code in the event payload yet — pull
            // it from the `code` field if the gateway later includes it,
            // otherwise leave blank (the operator will pick from the
            // pairing.list RPC).
            match topic {
                "pairing.requested" => {
                    let code = event
                        .data
                        .get("code")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let origin_label = event
                        .data
                        .get("origin_label")
                        .and_then(|v| v.as_str())
                        .or_else(|| event.data.get("device_name").and_then(|v| v.as_str()))
                        .unwrap_or("Unknown browser")
                        .to_string();
                    state.incoming_pairings.update(|list| {
                        // De-dupe by code so a re-subscribe doesn't pile up
                        // stale rows.
                        list.retain(|p| p.code != code);
                        list.push(IncomingPairing {
                            code,
                            origin_label,
                            created_at_ms: js_sys::Date::now() as i64,
                        });
                    });
                }
                "pairing.completed" | "pairing.rejected" => {
                    let code = event
                        .data
                        .get("code")
                        .and_then(|v| v.as_str())
                        .or_else(|| event.data.get("device_id").and_then(|v| v.as_str()))
                        .unwrap_or("")
                        .to_string();
                    state.incoming_pairings.update(|list| {
                        list.retain(|p| p.code != code);
                    });
                }
                _ => {}
            }
        });

        self.pairing_subscription_id
            .set_value(Some(subscription_id));
        Ok(())
    }

    /// Subscribe to `approval.**` events so the NotificationCenter can render
    /// inline operator approval cards. The ApprovalRequested event is sparse
    /// (ids only), so `exec.approvals.pending` is the source of truth: any
    /// approval event simply triggers a refetch.
    pub async fn setup_approval_subscriptions(&self) -> Result<(), String> {
        self.subscribe_topic("approval.**").await?;
        web_sys::console::log_1(&"Subscribed to approval.** events".into());

        // Seed with whatever is already pending at connect time.
        if let Ok(list) = ExecApprovalApi::list_pending(self).await {
            self.pending_approvals.set(list);
        }

        let state = *self;
        let subscription_id =
            self.subscribe_events(move |event: GatewayEvent| match event.topic.as_str() {
                "approval.requested" | "approval.resolved" | "approval.expired" => {
                    spawn_local(async move {
                        if let Ok(list) = ExecApprovalApi::list_pending(&state).await {
                            state.pending_approvals.set(list);
                        }
                    });
                }
                _ => {}
            });

        self.approval_subscription_id
            .set_value(Some(subscription_id));
        Ok(())
    }

    /// Load initial alert states from Gateway
    ///
    /// This method fetches the current alert states when the UI first connects,
    /// ensuring that existing alerts are displayed even if no new events arrive.
    ///
    /// # Implementation Note
    ///
    /// Currently uses direct `rpc_call()` methods instead of `AlertsApi` from shared_ui_logic.
    /// This is because the `AlertsApi` in `/Volumes/TBU4/Workspace/Aleph/shared_ui_logic/` uses
    /// a different `RpcClient` implementation that is incompatible with the current architecture.
    ///
    /// **TODO**: Refactor to use `AlertsApi::get_system_health()` and `AlertsApi::get_memory_status()`
    /// once the shared_ui_logic crate is unified and the RpcClient implementations are aligned.
    async fn load_initial_alerts(&self) -> Result<(), String> {
        web_sys::console::log_1(&"Loading initial alert states...".into());

        // Fetch system health
        match self.rpc_call("health", serde_json::json!({})).await {
            Ok(result) => {
                if let Some(status) = result.get("status").and_then(|s| s.as_str()) {
                    let level = match status {
                        "healthy" => crate::components::sidebar::AlertLevel::None,
                        "degraded" => crate::components::sidebar::AlertLevel::Warning,
                        "unhealthy" => crate::components::sidebar::AlertLevel::Critical,
                        _ => crate::components::sidebar::AlertLevel::None,
                    };

                    if level != crate::components::sidebar::AlertLevel::None {
                        let message = result
                            .get("message")
                            .and_then(|m| m.as_str())
                            .map(|s| s.to_string());

                        let alert = crate::components::sidebar::SystemAlert {
                            key: "system.health".to_string(),
                            level,
                            count: None,
                            message,
                        };

                        self.update_alert(alert.key.clone(), alert);
                        web_sys::console::log_1(
                            &format!("Loaded system.health alert: {:?}", level).into(),
                        );
                    }
                }
            }
            Err(e) => {
                web_sys::console::warn_1(&format!("Failed to fetch system health: {}", e).into());
            }
        }

        // Fetch memory status
        match self.rpc_call("memory.stats", serde_json::json!({})).await {
            Ok(result) => {
                if let Some(db_size) = result.get("databaseSizeMb").and_then(|s| s.as_f64()) {
                    // Warn if database is larger than 100MB
                    if db_size > 100.0 {
                        let alert = crate::components::sidebar::SystemAlert {
                            key: "memory.status".to_string(),
                            level: crate::components::sidebar::AlertLevel::Warning,
                            count: None,
                            message: Some(format!("Database size: {:.1} MB", db_size)),
                        };

                        self.update_alert(alert.key.clone(), alert);
                        web_sys::console::log_1(
                            &format!("Loaded memory.status alert: {:.1} MB", db_size).into(),
                        );
                    }
                }
            }
            Err(e) => {
                web_sys::console::warn_1(&format!("Failed to fetch memory stats: {}", e).into());
            }
        }

        web_sys::console::log_1(&"Initial alert states loaded".into());
        Ok(())
    }

    /// Cleanup alert subscriptions
    ///
    /// This method unsubscribes from alert events and clears the subscription ID.
    pub fn cleanup_alert_subscriptions(&self) {
        if let Some(subscription_id) = self.alert_subscription_id.get_value() {
            self.unsubscribe_events(subscription_id);
            self.alert_subscription_id.set_value(None);
            web_sys::console::log_1(&"Unsubscribed from alert events".into());
        }
    }
}

#[component]
pub fn DashboardContext(children: Children) -> impl IntoView {
    let state = DashboardState::new();
    provide_context(state);

    view! {
        <ErrorBoundary
            fallback=|errors| view! {
                <div class="min-h-screen flex items-center justify-center bg-surface text-text-primary p-8">
                    <div class="max-w-md w-full bg-surface-raised border border-danger/20 rounded-2xl p-8">
                        <h2 class="text-2xl font-bold text-danger mb-4 flex items-center gap-2">
                            "System Error"
                        </h2>
                        <div class="space-y-4">
                            <For
                                each=move || errors.get()
                                key=|(id, _)| id.clone()
                                children=move |(_, error)| {
                                    let error_string = error.to_string();
                                    view! {
                                        <div class="bg-danger-subtle border border-danger/20 rounded-xl p-4 text-sm text-danger font-mono">
                                            {error_string}
                                        </div>
                                    }
                                }
                            />
                        </div>
                        <button
                            on:click=|_| {
                                #[cfg(target_arch = "wasm32")]
                                {
                                    let _ = web_sys::window().unwrap().location().reload();
                                }
                            }
                            class="mt-8 w-full py-3 bg-surface-sunken hover:bg-surface-raised rounded-xl transition-colors font-semibold"
                        >
                            "Reload Dashboard"
                        </button>
                    </div>
                </div>
            }
        >
            {children()}
        </ErrorBoundary>
    }
}

#[cfg(test)]
mod tests {
    use super::role_is_operator;

    #[test]
    fn operator_role_is_operator() {
        assert!(role_is_operator(Some("operator")));
    }

    #[test]
    fn guest_role_is_not_operator() {
        assert!(!role_is_operator(Some("guest")));
    }

    #[test]
    fn missing_role_is_not_operator() {
        assert!(!role_is_operator(None));
    }

    #[test]
    fn unknown_role_is_not_operator() {
        assert!(!role_is_operator(Some("node")));
    }
}
