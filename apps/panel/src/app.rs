use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::Router;
use leptos_router::hooks::use_location;

// Views
use crate::views::home::Home;
use crate::views::system_status::SystemStatus;
use crate::views::agent_trace::AgentTrace;
use crate::views::memory::Memory;
use crate::views::chat::ChatView;
use crate::views::cron::CronView;
use crate::views::logs::Logs;
use crate::views::settings::*;
use crate::views::settings::channels::config_template::ChannelConfigTemplate;
use crate::views::settings::channels::definitions;

// Layout components
use crate::components::top_bar::TopBar;
use crate::components::mode_sidebar::ModeSidebar;
use crate::components::bottom_bar::{BottomBar, PanelMode};
use crate::context::{DashboardContext, DashboardState};

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

    // Setup WebSocket connection and alert subscriptions on mount
    Effect::new(move || {
        spawn_local(async move {
            match state.connect().await {
                Ok(()) => {
                    web_sys::console::log_1(&"Connected to Gateway".into());
                    if let Err(e) = state.setup_alert_subscriptions().await {
                        web_sys::console::error_1(&format!("Failed to setup alert subscriptions: {}", e).into());
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

// ---------------------------------------------------------------------------
// Thin wrapper components: one per template-driven channel
// ---------------------------------------------------------------------------

#[component]
fn TelegramConfigPage() -> impl IntoView {
    view! { <ChannelConfigTemplate definition=definitions::TELEGRAM /> }
}

#[component]
fn WhatsAppConfigPage() -> impl IntoView {
    view! { <ChannelConfigTemplate definition=definitions::WHATSAPP /> }
}

#[component]
fn IMessageConfigPage() -> impl IntoView {
    view! { <ChannelConfigTemplate definition=definitions::IMESSAGE /> }
}

#[component]
fn SlackConfigPage() -> impl IntoView {
    view! { <ChannelConfigTemplate definition=definitions::SLACK /> }
}

#[component]
fn EmailConfigPage() -> impl IntoView {
    view! { <ChannelConfigTemplate definition=definitions::EMAIL /> }
}

#[component]
fn MatrixConfigPage() -> impl IntoView {
    view! { <ChannelConfigTemplate definition=definitions::MATRIX /> }
}

#[component]
fn SignalConfigPage() -> impl IntoView {
    view! { <ChannelConfigTemplate definition=definitions::SIGNAL /> }
}

#[component]
fn MattermostConfigPage() -> impl IntoView {
    view! { <ChannelConfigTemplate definition=definitions::MATTERMOST /> }
}

#[component]
fn IrcConfigPage() -> impl IntoView {
    view! { <ChannelConfigTemplate definition=definitions::IRC /> }
}

#[component]
fn WebhookConfigPage() -> impl IntoView {
    view! { <ChannelConfigTemplate definition=definitions::WEBHOOK /> }
}

#[component]
fn XmppConfigPage() -> impl IntoView {
    view! { <ChannelConfigTemplate definition=definitions::XMPP /> }
}

#[component]
fn NostrConfigPage() -> impl IntoView {
    view! { <ChannelConfigTemplate definition=definitions::NOSTR /> }
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
            "/dashboard/health" => view! { <SystemStatus /> }.into_any(),
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
            "/settings/agent" => view! { <AgentView /> }.into_any(),
            "/settings/routing" => view! { <RoutingRulesView /> }.into_any(),
            "/settings/mcp" => view! { <McpView /> }.into_any(),
            "/settings/plugins" => view! { <PluginsView /> }.into_any(),
            "/settings/skills" => view! { <SkillsView /> }.into_any(),

            // Security
            "/settings/security" => view! { <SecurityView /> }.into_any(),
            "/settings/policies" => view! { <PoliciesView /> }.into_any(),
            "/settings/vault" => view! { <VaultView /> }.into_any(),

            // Channels
            "/settings/channels" => view! { <ChannelsOverview /> }.into_any(),
            "/settings/channels/discord" => view! { <DiscordChannelView /> }.into_any(),
            "/settings/channels/telegram" => view! { <TelegramConfigPage /> }.into_any(),
            "/settings/channels/whatsapp" => view! { <WhatsAppConfigPage /> }.into_any(),
            "/settings/channels/imessage" => view! { <IMessageConfigPage /> }.into_any(),
            "/settings/channels/slack" => view! { <SlackConfigPage /> }.into_any(),
            "/settings/channels/email" => view! { <EmailConfigPage /> }.into_any(),
            "/settings/channels/matrix" => view! { <MatrixConfigPage /> }.into_any(),
            "/settings/channels/signal" => view! { <SignalConfigPage /> }.into_any(),
            "/settings/channels/mattermost" => view! { <MattermostConfigPage /> }.into_any(),
            "/settings/channels/irc" => view! { <IrcConfigPage /> }.into_any(),
            "/settings/channels/webhook" => view! { <WebhookConfigPage /> }.into_any(),
            "/settings/channels/xmpp" => view! { <XmppConfigPage /> }.into_any(),
            "/settings/channels/nostr" => view! { <NostrConfigPage /> }.into_any(),

            // Not in settings mode or unknown path — render nothing (div is hidden)
            _ => ().into_any(),
        }
    }
}
