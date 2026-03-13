use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::Router;
use leptos_router::hooks::use_location;

// Views
use crate::views::home::Home;
use crate::views::agent_trace::AgentTrace;
use crate::views::memory::Memory;
use crate::views::chat::ChatView;
use crate::views::cron::CronView;
use crate::views::logs::Logs;
use crate::views::settings::*;
use crate::views::wizard::SetupWizard;

// Layout components
use crate::components::top_bar::TopBar;
use crate::components::mode_sidebar::ModeSidebar;
use crate::components::bottom_bar::{BottomBar, PanelMode};
use crate::context::{DashboardContext, DashboardState};
use crate::api::ProvidersApi;

#[component]
pub fn App() -> impl IntoView {
    view! {
        <DashboardContext>
            <AppContent />
        </DashboardContext>
    }
}

#[component]
fn AppContent() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let show_wizard = RwSignal::new(false);

    // Setup WebSocket connection and alert subscriptions on mount
    Effect::new(move || {
        let state = state.clone();
        spawn_local(async move {
            match state.connect().await {
                Ok(()) => {
                    web_sys::console::log_1(&"Connected to Gateway".into());
                    if let Err(e) = state.setup_alert_subscriptions().await {
                        web_sys::console::error_1(&format!("Failed to setup alert subscriptions: {}", e).into());
                    }
                    // Check if first-run wizard is needed
                    match ProvidersApi::needs_setup(&state).await {
                        Ok(needs) => {
                            if needs {
                                show_wizard.set(true);
                            }
                        }
                        Err(e) => {
                            web_sys::console::error_1(&format!("Failed to check setup status: {}", e).into());
                        }
                    }
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to connect to Gateway: {}", e).into());
                }
            }
        });
    });

    // Cleanup on unmount
    on_cleanup(move || {
        spawn_local(async move {
            let _ = state.disconnect().await;
        });
    });

    view! {
        <div class="flex flex-col h-screen bg-surface text-text-primary font-sans selection:bg-primary/30">
            {move || show_wizard.get().then(|| view! {
                <SetupWizard on_close=Callback::new(move |_| show_wizard.set(false)) />
            })}
            <Router>
                // Top bar (fixed)
                <TopBar />

                // Middle: sidebar + main content
                <div class="flex flex-1 overflow-hidden">
                    // Context-aware sidebar
                    <ModeSidebar />

                    // Main content area — reactive routing via use_location()
                    <main class="flex-1 overflow-y-auto relative">
                        <MainContent />
                    </main>
                </div>

                // Bottom navigation bar (fixed)
                <BottomBar />
            </Router>
        </div>
    }
}

/// Main content routing — uses CSS display toggling for mode switching to keep
/// mode containers alive, avoiding reactive scope issues with Effect::new()
/// inside re-evaluating closures. Sub-routing within each mode is handled by
/// dedicated router components.
#[component]
fn MainContent() -> impl IntoView {
    let location = use_location();
    let mode = Memo::new(move |_| PanelMode::from_path(&location.pathname.get()));

    view! {
        <div style:display=move || if mode.get() == PanelMode::Chat { "contents" } else { "none" }>
            <ChatView />
        </div>
        <div style:display=move || if mode.get() == PanelMode::Dashboard { "contents" } else { "none" }>
            <DashboardRouter />
        </div>
        <div style:display=move || if mode.get() == PanelMode::Agents { "contents" } else { "none" }>
            <AgentsRouter />
        </div>
        <div style:display=move || if mode.get() == PanelMode::Settings { "contents" } else { "none" }>
            <SettingsRouter />
        </div>
    }
}

/// Dashboard sub-routing
#[component]
fn DashboardRouter() -> impl IntoView {
    let location = use_location();

    move || {
        let path = location.pathname.get();
        match path.as_str() {
            "/dashboard" => view! { <Home /> }.into_any(),
            "/dashboard/trace" => view! { <AgentTrace /> }.into_any(),
            "/dashboard/memory" => view! { <Memory /> }.into_any(),
            "/dashboard/cron" => view! { <CronView /> }.into_any(),
            "/dashboard/logs" => view! { <Logs /> }.into_any(),
            // Not in dashboard mode — render nothing (div is hidden)
            _ => ().into_any(),
        }
    }
}

/// Settings sub-routing
#[component]
fn SettingsRouter() -> impl IntoView {
    let location = use_location();

    move || {
        let path = location.pathname.get();
        match path.as_str() {
            // Basic
            "/settings" | "/settings/general" => view! { <GeneralView /> }.into_any(),
            "/settings/shortcuts" => view! { <ShortcutsView /> }.into_any(),
            "/settings/behavior" => view! { <BehaviorView /> }.into_any(),

            // AI
            "/settings/search" => view! { <SearchView /> }.into_any(),
            "/settings/providers" => view! { <ProvidersView /> }.into_any(),
            "/settings/embedding-providers" => view! { <EmbeddingProvidersView /> }.into_any(),
            "/settings/generation-providers" => view! { <GenerationProvidersView /> }.into_any(),
            "/settings/memory" => view! { <MemoryView /> }.into_any(),

            // Extensions
            "/settings/routing" => view! { <RoutingRulesView /> }.into_any(),
            "/settings/mcp" => view! { <McpView /> }.into_any(),
            "/settings/plugins" => view! { <PluginsView /> }.into_any(),
            "/settings/skills" => view! { <SkillsView /> }.into_any(),

            // Security
            "/settings/security" => view! { <SecurityView /> }.into_any(),
            "/settings/auth" => view! { <AuthView /> }.into_any(),
            "/settings/policies" => view! { <PoliciesView /> }.into_any(),
            "/settings/vault" => view! { <VaultView /> }.into_any(),

            // Channels
            "/settings/channels" => view! { <ChannelsOverview /> }.into_any(),
            _ if path.starts_with("/settings/channels/") => {
                let platform_type = path.strip_prefix("/settings/channels/")
                    .unwrap_or("")
                    .to_string();
                view! { <ChannelPlatformPage platform_type=platform_type /> }.into_any()
            },

            // Not in settings mode or unknown path — render nothing (div is hidden)
            _ => ().into_any(),
        }
    }
}

/// Agents sub-routing
#[component]
fn AgentsRouter() -> impl IntoView {
    let location = use_location();

    move || {
        let path = location.pathname.get();
        if path.starts_with("/agents") {
            view! { <crate::views::agents::AgentsView /> }.into_any()
        } else {
            ().into_any()
        }
    }
}
