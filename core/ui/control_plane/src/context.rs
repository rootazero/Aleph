use leptos::prelude::*;
use leptos::task::spawn_local;
use shared_ui_logic::connection::wasm::WasmConnector;
use shared_ui_logic::connection::connector::AlephConnector;
use gloo_timers::future::TimeoutFuture;
use std::sync::Arc;
use std::sync::Mutex;
use std::collections::HashMap;
use futures::{StreamExt, FutureExt};
use futures::channel::{oneshot, mpsc};
use serde_json::Value;
use crate::components::sidebar::{SidebarMode, SystemAlert};

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

#[derive(Clone, Copy)]
pub struct DashboardState {
    pub is_connected: RwSignal<bool>,
    pub reconnect_count: RwSignal<u32>,
    pub gateway_url: RwSignal<String>,
    pub connection_error: RwSignal<Option<String>>,
    pub is_reconnecting: RwSignal<bool>,

    // Phase 3: Channel to send RPC requests to message loop
    rpc_tx: StoredValue<Option<mpsc::UnboundedSender<RpcRequest>>>,
    next_id: StoredValue<Arc<Mutex<u64>>>,

    // Phase 3: Event handling
    event_handlers: StoredValue<Arc<Mutex<Vec<EventHandler>>>>,

    // Channel for stopping the message loop
    disconnect_tx: StoredValue<Option<oneshot::Sender<()>>>,

    /// System alert state bus
    pub alerts: RwSignal<HashMap<String, SystemAlert>>,

    /// Sidebar mode override (user manual setting)
    pub sidebar_mode_override: RwSignal<Option<SidebarMode>>,

    /// Alert subscription ID for cleanup
    alert_subscription_id: StoredValue<Option<usize>>,
}

impl DashboardState {
    pub fn new() -> Self {
        Self {
            is_connected: RwSignal::new(false),
            reconnect_count: RwSignal::new(0),
            gateway_url: RwSignal::new("ws://127.0.0.1:18789".to_string()),
            connection_error: RwSignal::new(None),
            is_reconnecting: RwSignal::new(false),
            rpc_tx: StoredValue::new(None),
            next_id: StoredValue::new(Arc::new(Mutex::new(1))),
            event_handlers: StoredValue::new(Arc::new(Mutex::new(Vec::new()))),
            disconnect_tx: StoredValue::new(None),
            alerts: RwSignal::new(HashMap::new()),
            sidebar_mode_override: RwSignal::new(None),
            alert_subscription_id: StoredValue::new(None),
        }
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
        self.rpc_call("events.subscribe", serde_json::json!({
            "pattern": pattern
        })).await?;
        Ok(())
    }

    /// Unsubscribe from an event topic
    pub async fn unsubscribe_topic(&self, pattern: &str) -> Result<(), String> {
        self.rpc_call("events.unsubscribe", serde_json::json!({
            "pattern": pattern
        })).await?;
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
                tx.unbounded_send(request).map_err(|_| "Failed to send RPC request".to_string())?;
            } else {
                return Err("Not connected".to_string());
            }
        }

        // Wait for response
        response_rx.await.map_err(|_| "Response channel closed".to_string())?
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
                let (rpc_tx, mut rpc_rx) = mpsc::unbounded::<RpcRequest>();
                let (disconnect_tx, mut disconnect_rx) = oneshot::channel::<()>();

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
                    let mut pending_rpcs: HashMap<String, oneshot::Sender<Result<Value, String>>> = HashMap::new();

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
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        web_sys::console::error_1(&format!("Message loop error: {:?}", e).into());
                                        break;
                                    }
                                }
                            }

                            // Handle disconnect signal
                            _ = disconnect_rx => {
                                web_sys::console::log_1(&"Disconnect signal received".into());
                                let _ = connector.disconnect().await;
                                break;
                            }

                            // If all channels are closed, exit
                            complete => break,
                        }
                    }

                    web_sys::console::log_1(&"Message loop stopped".into());
                });

                self.is_connected.set(true);
                self.connection_error.set(None);
                self.reconnect_count.set(0);
                self.is_reconnecting.set(false);

                // Subscribe to config events automatically
                let state_for_subscribe = *self;
                spawn_local(async move {
                    if let Err(e) = state_for_subscribe.subscribe_topic("config.**").await {
                        web_sys::console::error_1(&format!("Failed to subscribe to config events: {}", e).into());
                    } else {
                        web_sys::console::log_1(&"Subscribed to config.** events".into());
                    }
                });

                Ok(())
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

            web_sys::console::log_1(&format!("Reconnecting in {}ms (attempt {})", delay_ms, attempt + 1).into());

            TimeoutFuture::new(delay_ms).await;

            match self.connect().await {
                Ok(()) => {
                    web_sys::console::log_1(&"Reconnection successful".into());
                    self.is_reconnecting.set(false);
                    return Ok(());
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Reconnection attempt {} failed: {}", attempt + 1, e).into());

                    if attempt + 1 >= max_attempts {
                        let error_msg = format!("Failed to reconnect after {} attempts", max_attempts);
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
            web_sys::console::log_1(&format!("Alert event received: {} - {:?}", event.topic, event.data).into());

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
                            web_sys::console::warn_1(&format!("Unknown alert severity: {}", severity).into());
                            crate::components::sidebar::AlertLevel::None
                        }
                    };

                    let count = event.data.get("count")
                        .and_then(|c| c.as_u64())
                        .map(|c| c as u32);

                    let message = event.data.get("message")
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
                    web_sys::console::warn_1(&format!("Alert event missing severity field: {}", event.topic).into());
                    state.clear_alert(alert_key);
                }
            }
        });

        // Store subscription ID for cleanup
        self.alert_subscription_id.set_value(Some(subscription_id));

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
                        let message = result.get("message")
                            .and_then(|m| m.as_str())
                            .map(|s| s.to_string());

                        let alert = crate::components::sidebar::SystemAlert {
                            key: "system.health".to_string(),
                            level,
                            count: None,
                            message,
                        };

                        self.update_alert(alert.key.clone(), alert);
                        web_sys::console::log_1(&format!("Loaded system.health alert: {:?}", level).into());
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
                        web_sys::console::log_1(&format!("Loaded memory.status alert: {:.1} MB", db_size).into());
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
                <div class="min-h-screen flex items-center justify-center bg-slate-950 text-slate-50 p-8">
                    <div class="max-w-md w-full bg-slate-900 border border-red-500/20 rounded-3xl p-8 shadow-2xl">
                        <h2 class="text-2xl font-bold text-red-500 mb-4 flex items-center gap-2">
                            "⚠️ System Error"
                        </h2>
                        <div class="space-y-4">
                            <For
                                each=move || errors.get()
                                key=|(id, _)| id.clone()
                                children=move |(_, error)| {
                                    let error_string = error.to_string();
                                    view! {
                                        <div class="bg-red-500/10 border border-red-500/20 rounded-xl p-4 text-sm text-red-400 font-mono">
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
                            class="mt-8 w-full py-3 bg-slate-800 hover:bg-slate-700 rounded-xl transition-colors font-semibold"
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
