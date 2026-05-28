use crate::i18n::*;
use crate::state::memory::MemoryState;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::Router;
use leptos_router::hooks::use_location;

// Views
use crate::views::agent_trace::AgentTrace;
use crate::views::canvas::CanvasView;
use crate::views::pairing_modal::PairingModal;
use crate::views::chat::ChatView;
use crate::views::cron::CronView;
use crate::views::home::Home;
use crate::views::logs::Logs;
use crate::views::memory::Memory;
use crate::views::runtimes::RuntimesView;
use crate::views::settings::*;
use crate::views::tasks::TasksView;
use crate::views::teams::TeamsView;
use crate::views::usage::UsageView;
// Layout components
use crate::components::boot_check_gate::BootCheckGate;
use crate::components::command_palette::CommandPalette;
use crate::components::mode_sidebar::{ModeSidebar, PanelMode};
use crate::components::notification_center::NotificationCenter;
use crate::components::service_blocking_gate::ServiceBlockingGate;
use crate::components::tool_renderer::ToolRendererRegistry;
use crate::context::{DashboardContext, DashboardState};
use crate::state::hotkey::{self as hotkey, HotkeyState};
use crate::state::layout::WorkspaceState;
use crate::state::notifications::NotificationsState;
use crate::state::sessions::SessionMap;
use crate::views::chat::ChatState;

#[component]
pub fn App() -> impl IntoView {
    view! {
        <I18nContextProvider>
            <DashboardContext>
                <AppContent />
            </DashboardContext>
        </I18nContextProvider>
    }
}

#[component]
fn AppContent() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // MemoryState must be provided here (parent of MainContent) so the
    // aleph-shell class binding and the Esc key listener can both read it.
    provide_context(MemoryState::new());

    // Chat state lives above both the chat sidebar (left column) and the
    // chat view (main area) so they share one session / agent selection.
    provide_context(ChatState::new());

    // Multi-tab session registry (per-agent). Empty at boot; the chat
    // sidebar's auto-select-default-agent path is what opens the first
    // tab. Cmd+1..9 / Cmd+W hotkeys are installed lazily by SessionTabs.
    provide_context(SessionMap::new());

    // Workspace pane state — UI-TARS-parity. ChatOnly is the default so
    // legacy users see zero UI change; the LayoutToggle in the composer
    // opens Split mode on demand. Persisted in localStorage.
    provide_context(WorkspaceState::new());

    // Tool-renderer dispatch table. Built once with the default palette
    // (code / search / json-fallback); future renderers register by
    // extending the constructor.
    provide_context(ToolRendererRegistry::with_builtins());

    // Hotkey state — owns the ⌘K command-palette open signal. Installed
    // *before* the keydown listener below so the listener can read it.
    let hk = HotkeyState::new();
    provide_context(hk);
    hotkey::install(hk);

    // NotificationCenter UI state — open/closed + dismissed key set. The
    // data layer (alert subscriptions on DashboardState) is already wired
    // in `setup_alert_subscriptions()`; this is purely the popover surface.
    provide_context(NotificationsState::new());

    // Esc key: uncollapse sidebar when collapsed. Coexists with the
    // hotkey-installed Esc handler that closes the palette; both only act
    // when their respective state is "open", so they never conflict.
    {
        use leptos::ev::keydown;
        let mem_for_key = expect_context::<MemoryState>();
        window_event_listener(keydown, move |ev: web_sys::KeyboardEvent| {
            if ev.key() == "Escape" && mem_for_key.sidebar_collapsed.get() {
                mem_for_key.sidebar_collapsed.set(false);
            }
        });
    }

    // Setup WebSocket connection and alert subscriptions on mount
    Effect::new(move || {
        let state = state;
        spawn_local(async move {
            match state.connect().await {
                Ok(()) => {
                    web_sys::console::log_1(&"Connected to Gateway".into());
                    if let Err(e) = state.setup_alert_subscriptions().await {
                        web_sys::console::error_1(
                            &format!("Failed to setup alert subscriptions: {}", e).into(),
                        );
                    }
                    if let Err(e) = state.setup_pairing_subscriptions().await {
                        web_sys::console::error_1(
                            &format!("Failed to setup pairing subscriptions: {}", e).into(),
                        );
                    }
                }
                Err(e) => {
                    web_sys::console::error_1(
                        &format!("Failed to connect to Gateway: {}", e).into(),
                    );
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

    let mem_for_shell = expect_context::<MemoryState>();

    view! {
        // Two-column shell (Codex) floating on the drifting light-field.
        <div
            class="aleph-shell flex h-screen text-text-primary font-sans selection:bg-primary/30"
            class:sidebar-collapsed=move || mem_for_shell.sidebar_collapsed.get()
        >
            // No global title-bar drag strip — the macOS Overlay title bar
            // is now carved out from the chrome buttons by attaching
            // `data-tauri-drag-region=""` to the structural elements that
            // SHOULD drag (sidebar brand row, workspace pane header,
            // chat-surface top strip). Each chrome button explicitly opts
            // out via `-webkit-app-region: no-drag` (utility class
            // `aleph-no-drag` or via the button's own CSS rules), so the
            // drag surface yields to the buttons rather than the other
            // way around.

            // Fixed top-left collapse button — anchored to the window so it
            // stays clickable when the sidebar slides off-screen. Visibility
            // (see tailwind.css):
            //   • macOS Tauri: always visible, vertically centred with the
            //     overlay traffic lights.
            //   • Web + Tauri Win/Linux: only visible when the sidebar is
            //     collapsed; while expanded, the inline brand-row button in
            //     SidebarBrand handles it.
            // `data-tauri-drag-region="false"` opts out of the parent drag
            // strip so clicks aren't swallowed by the window-drag handler.
            <button
                type="button"
                class="aleph-sidebar-toggle"
                data-tauri-drag-region="false"
                on:click={
                    let mem = mem_for_shell;
                    move |_| {
                        let s = &mem.sidebar_collapsed;
                        s.set(!s.get());
                    }
                }
                title="Toggle sidebar (Esc)"
                aria-label="Toggle sidebar"
            >
                <svg
                    width="16" height="16" viewBox="0 0 24 24" fill="none"
                    stroke="currentColor" stroke-width="1.8"
                    stroke-linecap="round" stroke-linejoin="round"
                >
                    <rect x="3" y="5" width="18" height="14" rx="2.5" />
                    <line x1="9" y1="5" x2="9" y2="19" />
                </svg>
            </button>
            <Router>
                // Left column — context-aware sidebar, full window height
                <ModeSidebar />

                // Main content area — transparent, so the light-field shows through
                <main class="flex-1 overflow-y-auto relative">
                    <MainContent />
                </main>

                // ⌘K / Ctrl+K command palette overlay. Mounted inside <Router>
                // so it can call `use_navigate()`; the overlay itself sits
                // above all shell chrome via z-index.
                <CommandPalette />

                // Aggregate alert surface — bell + popover anchored top-right.
                // Reads DashboardState.alerts (already wired via alerts.**).
                <NotificationCenter />

                // Runtime recovery overlay — engages when the panel was live
                // but lost the Gateway and exhausted automatic reconnects.
                // Inside <Router> so its "Open logs" button can navigate.
                <ServiceBlockingGate />
            </Router>

            // First-boot gate — blocks the shell with a "Connecting…" or
            // "Cannot reach core" overlay until the first auth succeeds.
            // Outside <Router> because it never navigates.
            <BootCheckGate />

            // Pairing modal overlays everything when pairing_required is triggered
            <PairingModal />
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
        <div style:display=move || if mode.get() == PanelMode::Memory { "contents" } else { "none" }>
            <CanvasView />
        </div>
        <div style:display=move || if mode.get() == PanelMode::Agents { "contents" } else { "none" }>
            <AgentsRouter />
        </div>
        <div style:display=move || if mode.get() == PanelMode::Teams { "contents" } else { "none" }>
            <TeamsView />
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
            "/dashboard/memory" => view! { <Memory /> }.into_any(),
            "/dashboard/cron" => view! { <CronView /> }.into_any(),
            "/dashboard/tasks" => view! { <TasksView /> }.into_any(),
            "/dashboard/logs" => view! { <Logs /> }.into_any(),
            "/dashboard/trace" => view! { <AgentTrace /> }.into_any(),
            "/dashboard/runtimes" => view! { <RuntimesView /> }.into_any(),
            "/dashboard/usage" => view! { <UsageView /> }.into_any(),
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
            "/settings" => view! { <Settings /> }.into_any(),
            "/settings/general" => view! { <GeneralView /> }.into_any(),
            "/settings/behavior" => view! { <BehaviorView /> }.into_any(),

            // AI
            "/settings/search" => view! { <SearchView /> }.into_any(),
            "/settings/providers" => view! { <ProvidersView /> }.into_any(),
            "/settings/embedding-providers" => view! { <EmbeddingProvidersView /> }.into_any(),
            "/settings/reranking-providers" => view! { <RerankingProvidersView /> }.into_any(),
            "/settings/generation-providers" => view! { <GenerationProvidersView /> }.into_any(),
            "/settings/memory" => view! { <MemoryView /> }.into_any(),

            // Browser
            "/settings/browser" => view! { <BrowserView /> }.into_any(),
            "/settings/runtime" => view! { <RuntimeView /> }.into_any(),

            // Extensions
            "/settings/routing" => view! { <RoutingRulesView /> }.into_any(),
            "/settings/mcp" => view! { <McpView /> }.into_any(),
            "/settings/plugins" => view! { <PluginsView /> }.into_any(),
            "/settings/skills" => view! { <SkillsView /> }.into_any(),
            "/settings/clawhub" => view! { <ClawHubView /> }.into_any(),
            "/settings/acp" => view! { <AcpHarnessesView /> }.into_any(),

            // Security
            "/settings/security" => view! { <SecurityView /> }.into_any(),
            "/settings/auth" => view! { <AuthView /> }.into_any(),
            "/settings/policies" => view! { <PoliciesView /> }.into_any(),
            "/settings/execution" => view! { <ExecutionView /> }.into_any(),
            // Channels
            "/settings/channels" => view! { <ChannelsOverview /> }.into_any(),
            _ if path.starts_with("/settings/channels/") => {
                let platform_type = path
                    .strip_prefix("/settings/channels/")
                    .unwrap_or("")
                    .to_string();
                view! { <ChannelPlatformPage platform_type=platform_type /> }.into_any()
            }

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
