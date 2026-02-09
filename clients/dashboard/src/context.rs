//! Global Dashboard Context
//!
//! Provides a single shared instance for dashboard state management.

use leptos::*;

/// Global dashboard context holding the shared state
#[derive(Clone)]
pub struct DashboardState {
    /// Connection status signal
    pub is_connected: RwSignal<bool>,
    /// Reconnect count signal
    pub reconnect_count: RwSignal<u32>,
    /// Gateway URL
    pub gateway_url: RwSignal<String>,
}

impl DashboardState {
    /// Create a new dashboard state
    pub fn new() -> Self {
        Self {
            is_connected: create_rw_signal(false),
            reconnect_count: create_rw_signal(0),
            gateway_url: create_rw_signal("ws://127.0.0.1:18789".to_string()),
        }
    }
}

/// Dashboard context provider component
#[component]
pub fn DashboardContext(children: Children) -> impl IntoView {
    // Create the global state
    let state = DashboardState::new();

    // Provide the context to all child components
    provide_context(state);

    view! {
        <ErrorBoundary
            fallback=|errors| view! {
                <div class="min-h-screen flex items-center justify-center bg-gray-900">
                    <div class="card max-w-2xl">
                        <h2 class="card-header text-red-500">"⚠️ Error"</h2>
                        <div class="space-y-2">
                            <For
                                each=move || errors.get().into_iter().enumerate()
                                key=|(index, _)| *index
                                children=move |(_, (_, error))| {
                                    let error_string = format!("{}", error);
                                    view! {
                                        <div class="bg-red-900/20 border border-red-500 rounded p-3 text-sm">
                                            {error_string}
                                        </div>
                                    }
                                }
                            />
                        </div>
                        <div class="mt-4 text-sm text-gray-400">
                            "The dashboard encountered an error. Please check the console for more details."
                        </div>
                    </div>
                </div>
            }
        >
            {children()}
        </ErrorBoundary>
    }
}
