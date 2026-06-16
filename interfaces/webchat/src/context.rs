use crate::api::ExecApprovalApi;
use crate::components::sidebar::SystemAlert;
use crate::state::notifications::PendingApprovalView;
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
    /// Latched true on the first successful connect handshake; never reset.
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

    /// Pending operator-approval requests rendered by the `NotificationCenter`
    /// with inline allow-once / allow-session / deny buttons. Sourced from the
    /// `exec.approvals.pending` RPC; `approval.**` events trigger a refetch
    /// (see `setup_approval_subscriptions`).
    pub pending_approvals: RwSignal<Vec<PendingApprovalView>>,

    /// Approval subscription ID for cleanup.
    approval_subscription_id: StoredValue<Option<usize>>,

    /// Feature flag: enable radial (TheBrain-style) navigation in the Canvas view.
    /// Initialized from localStorage; mutated by the Settings panel toggle.
    pub canvas_radial_navigation: RwSignal<bool>,

    /// Connection role captured from the `connect` response. Under the LAN-trust
    /// model the gateway always reports `Some("operator")`; `None` before the
    /// first successful connect. Read by `is_operator()` to gate operator-only
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

/// Stable per-Panel identity for device-tier pairing (G2/G3). The `device_id`
/// is a random handle generated once and kept in `localStorage`; the
/// `device_name` is a friendly label derived from the user agent. Sent in the
/// `connect` handshake so the gateway can look up / assign this device's
/// permission tier. The local (loopback) desktop App is always operator
/// regardless of this id, so it never lands in the device store.
#[cfg(target_arch = "wasm32")]
fn panel_device_identity() -> (String, String) {
    const KEY: &str = "aleph_device_id";
    let storage = web_sys::window().and_then(|w| w.local_storage().ok().flatten());
    let device_id = storage
        .as_ref()
        .and_then(|s| s.get_item(KEY).ok().flatten())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| {
            let id = format!(
                "panel-{:x}-{:x}",
                js_sys::Date::now() as u64,
                (js_sys::Math::random() * 1.0e9) as u64
            );
            if let Some(s) = storage.as_ref() {
                let _ = s.set_item(KEY, &id);
            }
            id
        });
    let device_name = web_sys::window()
        .map(|w| w.navigator())
        .and_then(|n| n.user_agent().ok())
        .map(|ua| friendly_device_name(&ua))
        .unwrap_or_else(|| "Panel".to_string());
    (device_id, device_name)
}

#[cfg(not(target_arch = "wasm32"))]
fn panel_device_identity() -> (String, String) {
    (String::new(), "Panel".to_string())
}

/// Best-effort "Browser on OS" label from a user-agent string.
#[cfg(target_arch = "wasm32")]
fn friendly_device_name(ua: &str) -> String {
    let browser = if ua.contains("Edg") {
        "Edge"
    } else if ua.contains("Chrome") || ua.contains("CriOS") {
        "Chrome"
    } else if ua.contains("Firefox") {
        "Firefox"
    } else if ua.contains("Safari") {
        "Safari"
    } else {
        "Browser"
    };
    let os = if ua.contains("Mac OS") {
        "macOS"
    } else if ua.contains("Windows") {
        "Windows"
    } else if ua.contains("Android") {
        "Android"
    } else if ua.contains("iPhone") || ua.contains("iPad") {
        "iOS"
    } else if ua.contains("Linux") || ua.contains("X11") {
        "Linux"
    } else {
        "device"
    };
    format!("{browser} on {os}")
}

impl Default for DashboardState {
    fn default() -> Self {
        Self::new()
    }
}

impl DashboardState {
    #[must_use]
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
            pending_approvals: RwSignal::new(Vec::new()),
            approval_subscription_id: StoredValue::new(None),
            canvas_radial_navigation: RwSignal::new(
                crate::api::settings::load_canvas_radial_navigation(),
            ),
            role: RwSignal::new(None),
        }
    }

    /// Reactive predicate: did this connection report the `operator` role?
    /// Consults the `role` captured from the `connect` response.
    /// Returns false before the first successful connect. Used by
    /// operator-only settings surfaces to gate UI up front.
    #[must_use]
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
        let handlers = self.event_handlers.with_value(std::clone::Clone::clone);
        let mut handlers = handlers.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let id = handlers.len();
        handlers.push(Arc::new(handler));
        id
    }

    /// Unsubscribe from events
    pub fn unsubscribe_events(&self, id: usize) {
        let handlers = self.event_handlers.with_value(std::clone::Clone::clone);
        let mut handlers = handlers.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
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
    #[must_use]
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
        let handlers = self.event_handlers.with_value(std::clone::Clone::clone);
        let handlers = handlers.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
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
            let next_id = self.next_id.with_value(std::clone::Clone::clone);
            let mut id_gen = next_id.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
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
            let rpc_tx = self.rpc_tx.with_value(std::clone::Clone::clone);
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

    /// Complete the session handshake after the WebSocket connection is
    /// established.
    ///
    /// LAN-trust model: there is no authentication. The handshake is a single
    /// `connect` frame carrying this Panel's stable `device_id`; the gateway
    /// replies with the session baseline (`role`/`state_version`/`keepalive`).
    /// `role` reflects the device tier — `operator` for the local (loopback)
    /// App, or this remote device's granted tier (`operator` = Config,
    /// otherwise Chat). We read `role` so `is_operator()` gates config UI.
    async fn handshake(&self) -> Result<(), String> {
        let (device_id, device_name) = panel_device_identity();
        let resp = self
            .rpc_call(
                "connect",
                serde_json::json!({
                    "device_id": device_id,
                    "device_name": device_name,
                }),
            )
            .await?;
        self.capture_role(&resp);
        Ok(())
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
                                        web_sys::console::error_1(&format!("Failed to send RPC: {e:?}").into());
                                        let _ = rpc_req.response_tx.send(Err(e.to_string()));
                                    }
                                }
                            }

                            // Handle incoming WebSocket messages
                            msg = stream.select_next_some() => {
                                match msg {
                                    Ok(value) => {
                                        web_sys::console::log_1(&format!("Received message: {value:?}").into());

                                        // Check if this is an RPC response (has 'id' field)
                                        if let Some(id) = value.get("id").and_then(|id| id.as_str()) {
                                            // Handle RPC response
                                            if let Some(tx) = pending_rpcs.remove(id) {
                                                if let Some(error) = value.get("error") {
                                                    let msg = error.get("message")
                                                        .and_then(|m| m.as_str())
                                                        .unwrap_or("Unknown error");
                                                    let msg = crate::components::permission::friendly_error(msg);
                                                    let _ = tx.send(Err(msg));
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
                                        web_sys::console::error_1(&format!("Message loop error: {e:?}").into());
                                        drop_reason = Some(format!("WebSocket dropped: {e}"));
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
                                    "WS dropped unintentionally; auto-reconnecting. reason={reason}"
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

                // Complete the session handshake before marking as connected.
                let handshake_state = *self;
                match handshake_state.handshake().await {
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
                                    &format!("Failed to subscribe to config events: {e}").into(),
                                );
                            }
                        });

                        Ok(())
                    }
                    Err(e) => {
                        // Handshake failed (transport-level) — surface the error
                        // so the boot/service gate offers a Retry path.
                        self.is_connected.set(false);
                        self.connection_error.set(Some(e.clone()));
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
                            format!("Failed to reconnect after {max_attempts} attempts");
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
    /// updates the DashboardState.alerts `HashMap` when events arrive.
    /// It also fetches initial alert states on mount.
    pub async fn setup_alert_subscriptions(&self) -> Result<(), String> {
        // Subscribe to alert events on the Gateway
        self.subscribe_topic("alerts.**").await?;

        web_sys::console::log_1(&"Subscribed to alerts.** events".into());

        // Load initial alert states
        let state_for_init = *self;
        spawn_local(async move {
            if let Err(e) = state_for_init.load_initial_alerts().await {
                web_sys::console::error_1(&format!("Failed to load initial alerts: {e}").into());
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
                                &format!("Unknown alert severity: {severity}").into(),
                            );
                            crate::components::sidebar::AlertLevel::None
                        }
                    };

                    let count = event
                        .data
                        .get("count")
                        .and_then(serde_json::Value::as_u64)
                        .map(|c| c as u32);

                    let message = event
                        .data
                        .get("message")
                        .and_then(|m| m.as_str())
                        .map(std::string::ToString::to_string);

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

    /// Subscribe to `approval.**` events so the `NotificationCenter` can render
    /// inline operator approval cards. The `ApprovalRequested` event is sparse
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
    /// Currently uses direct `rpc_call()` methods instead of `AlertsApi` from `shared_ui_logic`.
    /// This is because the `AlertsApi` in `/Volumes/TBU4/Workspace/Aleph/shared_ui_logic/` uses
    /// a different `RpcClient` implementation that is incompatible with the current architecture.
    ///
    /// **TODO**: Refactor to use `AlertsApi::get_system_health()` and `AlertsApi::get_memory_status()`
    /// once the `shared_ui_logic` crate is unified and the `RpcClient` implementations are aligned.
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
                            .map(std::string::ToString::to_string);

                        let alert = crate::components::sidebar::SystemAlert {
                            key: "system.health".to_string(),
                            level,
                            count: None,
                            message,
                        };

                        self.update_alert(alert.key.clone(), alert);
                        web_sys::console::log_1(
                            &format!("Loaded system.health alert: {level:?}").into(),
                        );
                    }
                }
            }
            Err(e) => {
                web_sys::console::warn_1(&format!("Failed to fetch system health: {e}").into());
            }
        }

        // Fetch memory status
        match self.rpc_call("memory.stats", serde_json::json!({})).await {
            Ok(result) => {
                if let Some(db_size) = result.get("databaseSizeMb").and_then(serde_json::Value::as_f64) {
                    // Warn if database is larger than 100MB
                    if db_size > 100.0 {
                        let alert = crate::components::sidebar::SystemAlert {
                            key: "memory.status".to_string(),
                            level: crate::components::sidebar::AlertLevel::Warning,
                            count: None,
                            message: Some(format!("Database size: {db_size:.1} MB")),
                        };

                        self.update_alert(alert.key.clone(), alert);
                        web_sys::console::log_1(
                            &format!("Loaded memory.status alert: {db_size:.1} MB").into(),
                        );
                    }
                }
            }
            Err(e) => {
                web_sys::console::warn_1(&format!("Failed to fetch memory stats: {e}").into());
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
#[must_use]
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
                                    if let Some(w) = web_sys::window() {
                                        let _ = w.location().reload();
                                    }
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
