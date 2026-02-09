use leptos::prelude::*;
use shared_ui_logic::connection::wasm::WasmConnector;
use shared_ui_logic::connection::connector::AlephConnector;
use gloo_timers::future::TimeoutFuture;

#[derive(Clone, Copy)]
pub struct DashboardState {
    pub is_connected: RwSignal<bool>,
    pub reconnect_count: RwSignal<u32>,
    pub gateway_url: RwSignal<String>,
    pub connection_error: RwSignal<Option<String>>,
    pub is_reconnecting: RwSignal<bool>,
}

impl DashboardState {
    pub fn new() -> Self {
        Self {
            is_connected: RwSignal::new(false),
            reconnect_count: RwSignal::new(0),
            gateway_url: RwSignal::new("ws://127.0.0.1:18789".to_string()),
            connection_error: RwSignal::new(None),
            is_reconnecting: RwSignal::new(false),
        }
    }

    /// Connect to the gateway
    pub async fn connect(&self) -> Result<(), String> {
        let url = self.gateway_url.get();
        let mut connector = WasmConnector::new();

        match connector.connect(&url).await {
            Ok(()) => {
                self.is_connected.set(true);
                self.connection_error.set(None);
                self.reconnect_count.set(0);
                self.is_reconnecting.set(false);
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
        // For now, just update the state
        // In a real implementation, we'd need to store the connector
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
