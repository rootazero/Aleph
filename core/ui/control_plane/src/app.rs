use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::*;
use leptos_router::path;

// Views
use crate::views::home::Home;
use crate::views::system_status::SystemStatus;
use crate::views::agent_trace::AgentTrace;
use crate::views::memory::Memory;
use crate::views::chat::ChatView;
use crate::views::settings::*;
use crate::views::settings::channels::config_template::ChannelConfigTemplate;
use crate::views::settings::channels::definitions;

// Layout components
use crate::components::top_bar::TopBar;
use crate::components::mode_sidebar::ModeSidebar;
use crate::components::bottom_bar::BottomBar;
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

                    // Main content area
                    <main class="flex-1 overflow-y-auto relative">
                        <Routes fallback=|| view! { <div class="p-8">"404 - Not Found"</div> }>
                            // Chat routes (default)
                            <Route path=path!("/") view=ChatView />

                            // Dashboard routes
                            <Route path=path!("/dashboard") view=Home />
                            <Route path=path!("/dashboard/trace") view=AgentTrace />
                            <Route path=path!("/dashboard/health") view=SystemStatus />
                            <Route path=path!("/dashboard/memory") view=Memory />

                            // Settings routes
                            <Route path=path!("/settings") view=Settings />
                            <Route path=path!("/settings/general") view=GeneralView />
                            <Route path=path!("/settings/shortcuts") view=ShortcutsView />
                            <Route path=path!("/settings/behavior") view=BehaviorView />
                            <Route path=path!("/settings/search") view=SearchView />
                            <Route path=path!("/settings/providers") view=ProvidersView />
                            <Route path=path!("/settings/embedding-providers") view=EmbeddingProvidersView />
                            <Route path=path!("/settings/generation-providers") view=GenerationProvidersView />
                            <Route path=path!("/settings/agent") view=AgentView />
                            <Route path=path!("/settings/routing") view=RoutingRulesView />
                            <Route path=path!("/settings/mcp") view=McpView />
                            <Route path=path!("/settings/plugins") view=PluginsView />
                            <Route path=path!("/settings/skills") view=SkillsView />
                            <Route path=path!("/settings/memory") view=MemoryView />
                            <Route path=path!("/settings/security") view=SecurityView />
                            <Route path=path!("/settings/policies") view=PoliciesView />
                            // Channels
                            <Route path=path!("/settings/channels") view=ChannelsOverview />
                            <Route path=path!("/settings/channels/discord") view=DiscordChannelView />
                            <Route path=path!("/settings/channels/telegram") view=TelegramConfigPage />
                            <Route path=path!("/settings/channels/whatsapp") view=WhatsAppConfigPage />
                            <Route path=path!("/settings/channels/imessage") view=IMessageConfigPage />
                            <Route path=path!("/settings/channels/slack") view=SlackConfigPage />
                            <Route path=path!("/settings/channels/email") view=EmailConfigPage />
                            <Route path=path!("/settings/channels/matrix") view=MatrixConfigPage />
                            <Route path=path!("/settings/channels/signal") view=SignalConfigPage />
                            <Route path=path!("/settings/channels/mattermost") view=MattermostConfigPage />
                            <Route path=path!("/settings/channels/irc") view=IrcConfigPage />
                            <Route path=path!("/settings/channels/webhook") view=WebhookConfigPage />
                            <Route path=path!("/settings/channels/xmpp") view=XmppConfigPage />
                            <Route path=path!("/settings/channels/nostr") view=NostrConfigPage />
                        </Routes>
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
