use leptos::prelude::*;

#[derive(Clone, Copy)]
pub struct DashboardState {
    pub is_connected: RwSignal<bool>,
    pub reconnect_count: RwSignal<u32>,
    pub gateway_url: RwSignal<String>,
}

impl DashboardState {
    pub fn new() -> Self {
        Self {
            is_connected: RwSignal::new(false),
            reconnect_count: RwSignal::new(0),
            gateway_url: RwSignal::new("ws://127.0.0.1:18789".to_string()),
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
