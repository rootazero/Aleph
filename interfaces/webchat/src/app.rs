use crate::i18n::{use_i18n, I18nContextProvider};
use crate::state::memory::MemoryState;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::Router;
use leptos_router::hooks::{use_location, use_navigate};

// Views
use crate::views::agent_trace::AgentTrace;
use crate::views::chat::ChatView;
use crate::views::cron::CronView;
use crate::views::extensions::{ExtensionsView, StoreState};
use crate::views::home::Home;
use crate::views::logs::Logs;
use crate::views::memory_hub::MemoryHub;
use crate::views::runtimes::RuntimesView;
use crate::views::settings::{
    AcpHarnessesView, AppearanceView, BehaviorView, BrowserView, ChannelPlatformPage,
    ChannelsOverview, EmbeddingProvidersView, ExecutionView, GeneralView, GenerationProvidersView,
    McpView, MemoryView, MoaView, NetworkView, PluginsView, PoliciesView, ProvidersView,
    RerankingProvidersView, RouteView, RoutingRulesView, SearchView, SecurityView, Settings,
    SkillsView,
};
use crate::views::subagent_tree::SubagentTree;
use crate::views::tasks::TasksView;
use crate::views::teams::{TeamsTabState, TeamsView};
use crate::views::usage::UsageView;
// Layout components
use crate::api::BehaviorConfigApi;
use crate::components::boot_check_gate::BootCheckGate;
use crate::components::command_palette::CommandPalette;
use crate::components::mode_sidebar::{ModeSidebar, PanelMode};
use crate::components::notification_center::NotificationCenter;
use crate::components::service_blocking_gate::ServiceBlockingGate;
use crate::components::token_wall::TokenWall;
use crate::context::{DashboardContext, DashboardState};
use crate::platform::phone::agents::PhoneAgents;
use crate::platform::phone::chat::PhoneChat;
use crate::platform::phone::dashboard::PhoneDashboard;
use crate::platform::phone::extensions::PhoneExtensions;
use crate::platform::phone::memory::PhoneMemory;
use crate::platform::phone::more::PhoneMore;
use crate::platform::phone::settings::appearance::PhoneAppearance;
use crate::platform::phone::settings::connection::PhoneConnection;
use crate::platform::phone::settings::embeddings::PhoneEmbeddings;
use crate::platform::phone::settings::model_route::PhoneModelRoute;
use crate::platform::phone::settings::providers::PhoneProviders;
use crate::platform::phone::settings::{
    screen_for_path as phone_settings_screen, title_for_path as phone_settings_title, NativeScreen,
    PhoneSettings, PhoneSettingsScreen,
};
use crate::platform::phone::shell::PhoneShell;
use crate::platform::phone::teams::PhoneTeams;
use crate::state::hotkey::{self as hotkey, HotkeyState};
use crate::state::layout::{LayoutMode, WorkspaceState};
use crate::state::notifications::NotificationsState;
use crate::state::sessions::SessionMap;
use crate::state::typewriter::TypewriterClock;
use crate::state::viewport::{FormFactor, FormFactorState};
use crate::views::chat::events::subscribe_run_events;
use crate::views::chat::ChatState;
use crate::views::voice::{ImmersiveVoiceView, VoiceMode};

#[component]
#[must_use]
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
    // Bound (not inlined) so it can also be threaded into `HotkeyState` below
    // for the `f` repair hotkey — `ChatState` is `Copy`, so this is free.
    let chat_state = ChatState::new();
    provide_context(chat_state);

    // Shared `tool_id → status` index over the foreground transcript. Provided
    // here (not inside the message list) because the right-pane tool inspector
    // is a sibling subtree, and both must read the same memo — otherwise every
    // mounted tool row re-scans the whole transcript on every streamed token.
    provide_context(crate::views::chat::state::ToolIndex::new(chat_state));

    // Immersive voice mode switch. Provided at the shell root so the composer
    // mini-orb (which flips it on) and the full-screen overlay (mounted at the
    // shell root, below) both read the same signal. The overlay reuses this
    // same ChatState, so a voice turn lands in the live chat transcript. Also
    // threaded into `HotkeyState` below so the ⌘⇧V / Esc bindings reach it.
    let voice_mode = VoiceMode::new();
    provide_context(voice_mode);

    // Multi-agent session registry. Empty at boot; the chat sidebar's
    // auto-select-default-agent path is what opens the first conversation.
    // The left sidebar's session list drives switching (activate) and shows
    // the running dot — there is no top tab strip.
    let session_map = SessionMap::new();
    provide_context(session_map);

    // Form-factor (Wide/Phone/Tablet) — read by SettingsRouter to swap the
    // wide `/settings` page for the iOS-native PhoneSettings at <640px.
    provide_context(FormFactorState::new());

    // Workspace pane state — UI-TARS-parity. ChatOnly is the default so
    // legacy users see zero UI change; the LayoutToggle in the composer
    // opens Split mode on demand. Persisted in localStorage.
    provide_context(WorkspaceState::new());

    // Global run.* dispatcher: subscribed once here (not per-mount) so a
    // run_id routes to the active singleton or a backgrounded live[conv]
    // session even when its ChatView/PhoneChat isn't mounted — background
    // sessions no longer freeze while another tab is in view.
    {
        let ws = expect_context::<WorkspaceState>();
        // The dispatcher outlives every component, so it cannot look the i18n
        // context up on demand — hand it the (Copy) handle once here.
        let _root_run_sub = subscribe_run_events(&state, session_map, chat_state, ws, use_i18n());
        // Root-level subscription is app-lifetime; no on_cleanup needed.
    }

    // Typewriter reveal clock — paces the Panel's streamed-text reveal at the
    // configured `behavior.typing_speed` (chars/sec). Provided here so the chat
    // view (reader) and the behavior settings page (writer-on-save) share one
    // clock. The animation ticker advances the reveal between token arrivals;
    // the boot fetch wires the *live* typing_speed/output_mode (previously the
    // slider was a dead knob — see state/typewriter.rs). `f` repair-hotkey
    // pattern aside, this is pure presentation (R10).
    let typewriter = TypewriterClock::new();
    provide_context(typewriter);
    spawn_local(async move {
        if let Ok(cfg) = BehaviorConfigApi::get(&state).await {
            typewriter.cps.set(cfg.typing_speed);
            typewriter.instant.set(cfg.output_mode == "instant");
        }
    });
    // ~30fps heartbeat so an in-flight reveal keeps advancing between deltas.
    // Bumping a u64 with no streaming bubble mounted has zero subscribers, so
    // it is effectively free when idle.
    set_interval(
        move || typewriter.tick.update(|t| *t = t.wrapping_add(1)),
        std::time::Duration::from_millis(33),
    );

    // Shared 1s clock — drives the live elapsed timer on long-running tool
    // rows (ToolCard). Only running rows subscribe to it, so idle history
    // never re-renders on this tick.
    let sec_tick = RwSignal::new(crate::views::chat::timeline::now_millis());
    provide_context(crate::state::run_clock::SecondTick(sec_tick));
    set_interval(
        move || sec_tick.set(crate::views::chat::timeline::now_millis()),
        std::time::Duration::from_secs(1),
    );

    // Hotkey state — owns the ⌘K command-palette open signal and carries the
    // VoiceMode switch for the ⌘⇧V / Esc bindings. Installed *before* the
    // keydown listener below so the listener can read it.
    let hk = HotkeyState::new(voice_mode, chat_state);
    provide_context(hk);
    hotkey::install(hk);

    // NotificationCenter UI state — open/closed + dismissed key set. The
    // data layer (alert subscriptions on DashboardState) is already wired
    // in `setup_alert_subscriptions()`; this is purely the popover surface.
    provide_context(NotificationsState::new());

    // Extensions (Aleph Hub) store — lifted above both columns so the
    // left-column category nav (ExtensionsSidebar) and the main-area grid
    // (BrowsePane) share one `category` selection. Mirrors ChatState's
    // split-column sharing (see above).
    provide_context(StoreState::new());

    // Process-wide user_id -> display_name projection (P2 Task 8) — read by
    // project-room message attribution and the roster picker. Provided once
    // here (not per-consumer) so a bubble and a roster row that both resolve
    // the same id share one `users.list` fetch instead of each re-fetching.
    provide_context(crate::state::user_directory::UserDirectoryState::new());

    // Project rooms tab state — lifted above both columns, mirrors
    // `teams_tab_state` below: the left-column `ProjectsSidebar` and the
    // main-area `ProjectsView` share one `selected_project_id` signal. This
    // is the "current project" — global, NOT per-conversation (landmine:
    // FEATURE_LOCATOR §4.7 — a per-session signal here would reset every
    // time the user switches chat tabs, which "which project page am I
    // looking at" must never do). The room conversation's OWN project
    // binding is the opposite: it lives on `ChatState::room_project_id` and
    // rides `SessionSnapshot` per-conversation — see that field's doc.
    provide_context(crate::components::project_page::ProjectsTabState::new());

    // Teams tab state — lifted above both columns so the left-column
    // `TeamsSidebar` and the main-area `TeamsView` share the same sub-tab
    // and selected-team signals. Previously `TeamsView` provided this
    // context itself, but `TeamsSidebar` is rendered in `ModeSidebar` (a
    // sibling owner), so switching to Teams panicked with a missing context.
    let teams_tab_state = TeamsTabState {
        sub_tab: RwSignal::new(crate::views::teams::TeamsSubTab::Overview),
        teams: RwSignal::new(Vec::new()),
        selected_team_id: RwSignal::new(None),
    };
    provide_context(teams_tab_state);

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

    // Tablet auto-collapses the sidebar so the main content gets full width;
    // the `.ff-tablet` shell class (added below) makes a *revealed* sidebar a
    // slide-over overlay rather than an in-flow column. The Effect's prev-value
    // tracks the previous band: it skips its Wide branch on the initial run so
    // a Wide boot honors the persisted localStorage preference, while a Tablet
    // boot still collapses. Manual toggles within a band don't re-fire it (it
    // only depends on `form_factor`), so they are preserved.
    {
        let mem_for_ff = expect_context::<MemoryState>();
        let ff = expect_context::<FormFactorState>();
        Effect::new(move |prev_band: Option<FormFactor>| -> FormFactor {
            let now = ff.form_factor.get();
            match prev_band {
                None => {
                    if now == FormFactor::Tablet {
                        mem_for_ff.sidebar_collapsed.set(true);
                    }
                }
                Some(was) if was != now => match now {
                    FormFactor::Tablet => mem_for_ff.sidebar_collapsed.set(true),
                    FormFactor::Wide => mem_for_ff.sidebar_collapsed.set(false),
                    FormFactor::Phone => {}
                },
                Some(_) => {}
            }
            now
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
                            &format!("Failed to setup alert subscriptions: {e}").into(),
                        );
                    }
                    if let Err(e) = state.setup_approval_subscriptions().await {
                        web_sys::console::error_1(
                            &format!("Failed to setup approval subscriptions: {e}").into(),
                        );
                    }
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to connect to Gateway: {e}").into());
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
    let ff_for_shell = expect_context::<FormFactorState>();

    // Desktop shell chrome is mounted only OUTSIDE the phone band.
    //
    // Every phone screen is a `fixed h-dvh z-[70]` overlay, so the sidebar
    // (z-auto, in flow), the fixed collapse toggle (z-60), the macOS drag band
    // (z-50) and the alerts bell (z-50 / popover z-55) were covered purely by
    // z-order while still being mounted, laid out and painted underneath — one
    // stacking accident away from showing through the phone's translucent glass
    // chrome, and paying full layout/effect cost for UI no phone user can reach
    // (the bell in particular is unreachable: nothing on phone rises above 70).
    // Gating them here removes the whole class by construction rather than
    // relying on the overlay staying opaque and on top.
    let not_phone = move || ff_for_shell.form_factor.get() != FormFactor::Phone;

    view! {
        // Two-column shell (Codex) floating on the drifting light-field.
        <div
            class="aleph-shell flex h-screen text-text-primary font-sans selection:bg-primary/30"
            class:sidebar-collapsed=move || mem_for_shell.sidebar_collapsed.get()
            class:ff-tablet=move || ff_for_shell.form_factor.get() == FormFactor::Tablet
        >
            // No global title-bar drag strip on the shell root — the macOS
            // Overlay title bar is carved out from the chrome buttons by
            // attaching `data-tauri-drag-region=""` to the structural
            // elements that SHOULD drag: the sidebar brand row (left
            // column, covers the area under the traffic lights), and a
            // dedicated `aleph-main-drag-band` parked at the top of the
            // right `<main>` column (covers every tab uniformly — chat,
            // dashboard, memory, agents, teams, settings). Each chrome
            // button explicitly opts out via `-webkit-app-region:
            // no-drag` (utility class `aleph-no-drag` or via the
            // button's own CSS rules), so the drag surface yields to
            // the buttons rather than the other way around.

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
            <Show when=not_phone>
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
            </Show>
            <Router>
                // Left column — context-aware sidebar, full window height.
                // Phone has its own tab bar; see `not_phone` above.
                <Show when=not_phone>
                    <ModeSidebar />
                </Show>

                // Main content area — `relative` is the positioning
                // ancestor for the absolutely-floating drag band below.
                // Transparent, so the light-field shows through.
                <main class="flex-1 relative min-h-0 overflow-y-auto overflow-x-hidden">
                    // Window-drag band — floats over the top of `<main>`
                    // WITHOUT taking layout space (CSS `position:
                    // absolute`, `background: transparent`). On macOS
                    // it occupies the overlay traffic-light strip
                    // (height `--aleph-band-h` = 30px) as a window-
                    // drag handle; on web / Win / Linux it collapses
                    // to height 0. Because the band is out of flow,
                    // tab content fills the full `<main>` height —
                    // no awkward downward shift and no visible white
                    // strip on tabs with contrasting backgrounds
                    // (e.g. the memory canvas).
                    //
                    // `z-50` keeps the band's chrome chips above
                    // tab content; the band itself is transparent
                    // so any text scrolling beneath is fine.
                    <Show when=not_phone>
                        <div
                            class="aleph-main-drag-band z-50"
                            data-tauri-drag-region=""
                        >
                            <ChatBandChrome />
                        </div>
                    </Show>
                    <MainContent />
                </main>

                // ⌘K / Ctrl+K command palette overlay. Mounted inside <Router>
                // so it can call `use_navigate()`; the overlay itself sits
                // above all shell chrome via z-index.
                <CommandPalette />

                // Aggregate alert surface — bell + popover anchored top-right.
                // Reads DashboardState.alerts (already wired via alerts.**).
                // Desktop/tablet only: its bell is z-50 and its popover z-55,
                // both below every phone screen's z-70 overlay, so on phone it
                // has never been clickable. Rendering it there bought nothing
                // and put live desktop chrome under the phone's glass bars. A
                // phone-native alerts entry point is separate work.
                <Show when=not_phone>
                    <NotificationCenter />
                </Show>

                // Runtime recovery overlay — engages when the panel was live
                // but lost the Gateway and exhausted automatic reconnects.
                // Inside <Router> so its "Open logs" button can navigate.
                <ServiceBlockingGate />
            </Router>

            // First-boot gate — blocks the shell with a "Connecting…" or
            // "Cannot reach core" overlay until the first connection succeeds.
            // Outside <Router> because it never navigates.
            <BootCheckGate />

            // Login wall — full-screen token box for a remote Panel that
            // connected without a valid Gateway token. Loopback / authorized
            // connections never render it. Mounted at the shell root so it
            // overlays everything until the device authorizes.
            <TokenWall />

            // Immersive voice overlay — full-screen `.voice-stage` (fixed,
            // z-40) hosting the mic/VAD/TTS session loop. Mounted at the shell
            // root so it floats over every tab; `ChatView` stays mounted
            // beneath it (MainContent toggles display, never unmounts), so the
            // `stream.*` subscription that feeds the assistant transcript keeps
            // running while the overlay is up. Renders nothing until opened.
            <ImmersiveVoiceView />
        </div>
    }
}

/// Chrome buttons that live INSIDE the `<main>` top drag band so they sit
/// on the traffic-light y-baseline (window-y ≈ 15 on macOS).
///
/// Two affordances, both chat-tab-only:
///   • `LayoutToggle` — sits at the chat-surface top-right. Right offset
///     is `right-[44px]` in `ChatOnly` (4 px left of the
///     `NotificationCenter` bell) and `right-[calc(var(--aleph-workspace-w)+8px)]`
///     in `Split` so it tracks the chat / workspace boundary: the pane is
///     `--aleph-workspace-w` of main, so the toggle parks 8 px inside the
///     chat surface, glued to the pane's leading edge at any window width.
///   • Workspace label ("WORKSPACE · idle / tool") — Split-only, left
///     offset is `left-[calc(100% - var(--aleph-workspace-w) + 16px)]` so
///     the text sits 16 px inside the workspace pane's leading edge,
///     matching the previous `WorkspaceHeader.px-4` placement.
///
/// Both offsets read the same `--aleph-workspace-w` token that sizes the
/// pane (see `tailwind.css` `:root`), so they never drift from the real
/// boundary — change the width in one place and all three follow.
///
/// Both children opt out of the drag region (`aleph-no-drag` +
/// `data-tauri-drag-region="false"`); the surrounding band space still
/// drags the window on macOS Overlay-titlebar windows.
#[component]
fn ChatBandChrome() -> impl IntoView {
    let location = use_location();
    let mode = Memo::new(move |_| PanelMode::from_path(&location.pathname.get()));
    // WorkspaceState may not be in scope during early boot races; fall
    // back to nothing rather than panicking.
    let Some(workspace) = use_context::<WorkspaceState>() else {
        return ().into_any();
    };
    // No band label. Both pane bodies are self-labelled now — team mode by its
    // "deliverables | tasks" tabs, single-agent by the artifacts header ("产物 ·
    // N") — so a generic "WORKSPACE" floating in the band was a second, vaguer
    // title for the same column. It also *overlapped* the real one: the band
    // label sits at `aleph-chrome-top` while the pane header sits at the pane's
    // own top inset, and on web those are the same row, so the two strings drew
    // on top of each other. Deleting the weaker one is the fix; the pane keeps
    // the title that carries a count.

    view! {
        <Show when=move || mode.get() == PanelMode::Chat>
            // LayoutToggle — right-edge tracks the chat / workspace
            // boundary. `pointer-events-auto` re-enables clicks because
            // the band itself is `pointer-events:none` on web / Win /
            // Linux (it only captures drag input on macOS). `aleph-no-drag`
            // is belt-and-braces — Tauri's `data-tauri-drag-region="false"`
            // already toggles `-webkit-app-region`, but the utility class
            // makes the opt-out resilient if the child re-parents.
            <div
                class=move || {
                    let base = "absolute aleph-chrome-top z-[45] \
                                pointer-events-auto aleph-no-drag aleph-ws-toggle";
                    if workspace.mode.get() == LayoutMode::Split {
                        format!("{base} right-[calc(var(--aleph-workspace-w)_+_8px)]")
                    } else {
                        format!("{base} right-[44px]")
                    }
                }
                data-tauri-drag-region="false"
            >
                <crate::components::layout_toggle::LayoutToggle />
            </div>
        </Show>
    }
    .into_any()
}

/// Main content routing — uses CSS display toggling for mode switching to keep
/// mode containers alive, avoiding reactive scope issues with Effect::new()
/// inside re-evaluating closures. Sub-routing within each mode is handled by
/// dedicated router components.
#[component]
fn MainContent() -> impl IntoView {
    let location = use_location();
    let mode = Memo::new(move |_| PanelMode::from_path(&location.pathname.get()));
    let form_factor = expect_context::<FormFactorState>();

    view! {
        <div style:display=move || if mode.get() == PanelMode::Chat { "contents" } else { "none" }>
            {move || if form_factor.form_factor.get() == FormFactor::Phone {
                view! { <PhoneChat /> }.into_any()
            } else {
                view! { <ChatView /> }.into_any()
            }}
        </div>
        <div style:display=move || if mode.get() == PanelMode::Dashboard { "contents" } else { "none" }>
            {move || if form_factor.form_factor.get() == FormFactor::Phone {
                view! { <PhoneDashboard /> }.into_any()
            } else {
                view! { <DashboardRouter /> }.into_any()
            }}
        </div>
        <div style:display=move || if mode.get() == PanelMode::Memory { "contents" } else { "none" }>
            {move || if form_factor.form_factor.get() == FormFactor::Phone {
                view! { <PhoneMemory /> }.into_any()
            } else {
                view! { <MemoryHub /> }.into_any()
            }}
        </div>
        <div style:display=move || if mode.get() == PanelMode::Agents { "contents" } else { "none" }>
            {move || if form_factor.form_factor.get() == FormFactor::Phone {
                view! { <PhoneAgents /> }.into_any()
            } else {
                view! { <AgentsRouter /> }.into_any()
            }}
        </div>
        <div style:display=move || if mode.get() == PanelMode::Teams { "contents" } else { "none" }>
            {move || if form_factor.form_factor.get() == FormFactor::Phone {
                view! { <PhoneTeams /> }.into_any()
            } else {
                view! { <TeamsView /> }.into_any()
            }}
        </div>
        // Project rooms (P2 Task 8) — desktop/wide only, like the workspace
        // pane itself. No phone screen exists for this yet (out of scope for
        // this task); on a narrow viewport the route still resolves but
        // simply shows nothing, exactly like `/more` on desktop.
        <div style:display=move || if mode.get() == PanelMode::Projects { "contents" } else { "none" }>
            {move || if form_factor.form_factor.get() == FormFactor::Phone {
                ().into_any()
            } else {
                view! { <crate::components::project_page::ProjectsView /> }.into_any()
            }}
        </div>
        <div style:display=move || if mode.get() == PanelMode::Extensions { "contents" } else { "none" }>
            {move || if form_factor.form_factor.get() == FormFactor::Phone {
                view! { <PhoneExtensions /> }.into_any()
            } else {
                view! { <ExtensionsView /> }.into_any()
            }}
        </div>
        <div style:display=move || if mode.get() == PanelMode::Settings { "block" } else { "none" }>
            <SettingsRouter />
        </div>
        <div style:display=move || if mode.get() == PanelMode::More { "contents" } else { "none" }>
            {move || if form_factor.form_factor.get() == FormFactor::Phone {
                view! { <PhoneMore /> }.into_any()
            } else {
                // /more is phone-only; desktop never routes here.
                ().into_any()
            }}
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
            "/dashboard/memory" => view! { <MemoryVaultRedirect /> }.into_any(),
            "/dashboard/cron" => view! { <CronView /> }.into_any(),
            "/dashboard/tasks" => view! { <TasksView /> }.into_any(),
            "/dashboard/logs" => view! { <Logs /> }.into_any(),
            "/dashboard/trace" => view! { <AgentTrace /> }.into_any(),
            "/dashboard/subagents" => view! { <SubagentTree /> }.into_any(),
            "/dashboard/runtimes" => view! { <RuntimesView /> }.into_any(),
            "/dashboard/usage" => view! { <UsageView /> }.into_any(),
            // Not in dashboard mode — render nothing (div is hidden)
            _ => ().into_any(),
        }
    }
}

/// Settings sub-routing.
///
/// Two form factors, one path table. Wide renders the desktop page body
/// directly; Phone asks [`phone_settings_screen`] which of its screens serves
/// the path — a hand-built iOS screen where one exists, otherwise the *same*
/// desktop body wrapped in `PhoneShell` so it gets a title, a `‹ Settings` back
/// button and the tab bar. Wrapping is what makes the 17 settings pages without
/// a native phone screen usable at all on a phone: before it, they rendered the
/// 256 px desktop sidebar into a 390 px viewport with no way back.
#[component]
fn SettingsRouter() -> impl IntoView {
    let location = use_location();
    let form_factor = expect_context::<FormFactorState>();
    let i18n = use_i18n();

    move || {
        let path = location.pathname.get();
        if form_factor.form_factor.get() != FormFactor::Phone {
            return desktop_settings_body(&path);
        }
        match phone_settings_screen(&path) {
            // Not in settings mode — render nothing (div is hidden).
            PhoneSettingsScreen::NotSettings => ().into_any(),
            PhoneSettingsScreen::Landing => view! { <PhoneSettings /> }.into_any(),
            PhoneSettingsScreen::Native(NativeScreen::Appearance) => {
                view! { <PhoneAppearance /> }.into_any()
            }
            PhoneSettingsScreen::Native(NativeScreen::Connection) => {
                view! { <PhoneConnection /> }.into_any()
            }
            PhoneSettingsScreen::Native(NativeScreen::Embeddings) => {
                view! { <PhoneEmbeddings /> }.into_any()
            }
            PhoneSettingsScreen::Native(NativeScreen::ModelRoute) => {
                view! { <PhoneModelRoute /> }.into_any()
            }
            PhoneSettingsScreen::Native(NativeScreen::Providers) => {
                view! { <PhoneProviders /> }.into_any()
            }
            PhoneSettingsScreen::Wrapped => view! {
                <PhoneShell title=phone_settings_title(&path, i18n) back="/settings" wrapped=true>
                    {desktop_settings_body(&path)}
                </PhoneShell>
            }
            .into_any(),
        }
    }
}

/// The desktop page body for a `/settings…` path — the single path table both
/// form factors read. Returns nothing for non-settings paths (the container div
/// is hidden then anyway).
///
/// Single-tier Gateway-token model: any connection that can reach these settings
/// is already authorized (operator) — loopback or a token-validated remote — so
/// there is no per-page ConfigGate; the login wall (TokenWall) is the one and
/// only gate.
fn desktop_settings_body(path: &str) -> AnyView {
    match path {
        "/settings" => view! { <Settings /> }.into_any(),
        "/settings/general" => view! { <GeneralView /> }.into_any(),
        "/settings/appearance" => view! { <AppearanceView /> }.into_any(),
        "/settings/behavior" => view! { <BehaviorView /> }.into_any(),

        // AI
        "/settings/search" => view! { <SearchView /> }.into_any(),
        "/settings/providers" => view! { <ProvidersView /> }.into_any(),
        "/settings/embedding-providers" => view! { <EmbeddingProvidersView /> }.into_any(),
        "/settings/reranking-providers" => view! { <RerankingProvidersView /> }.into_any(),
        "/settings/generation-providers" => view! { <GenerationProvidersView /> }.into_any(),
        "/settings/moa" => view! { <MoaView /> }.into_any(),
        "/settings/model-route" => view! { <RouteView /> }.into_any(),
        "/settings/memory" => view! { <MemoryView /> }.into_any(),

        // Browser
        "/settings/browser" => view! { <BrowserView /> }.into_any(),
        "/settings/network" => view! { <NetworkView /> }.into_any(),

        // Extensions
        "/settings/routing" => view! { <RoutingRulesView /> }.into_any(),
        "/settings/mcp" => view! { <McpView /> }.into_any(),
        "/settings/plugins" => view! { <PluginsView /> }.into_any(),
        "/settings/skills" => view! { <SkillsView /> }.into_any(),
        "/settings/acp" => view! { <AcpHarnessesView /> }.into_any(),

        // Security
        "/settings/security" => view! { <SecurityView /> }.into_any(),
        "/settings/policies" => view! { <PoliciesView /> }.into_any(),
        "/settings/execution" => view! { <ExecutionView /> }.into_any(),
        // Channels
        "/settings/channels" => view! { <ChannelsOverview /> }.into_any(),
        _ if path.starts_with("/settings/channels/") => {
            let platform_type = path
                .strip_prefix("/settings/channels/")
                .unwrap_or("")
                .to_string();
            let platform_type = StoredValue::new(platform_type);
            view! { <ChannelPlatformPage platform_type=platform_type.get_value() /> }.into_any()
        }

        // Not in settings mode or unknown path — render nothing (div is hidden)
        _ => ().into_any(),
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

/// Back-compat redirect: the Vault now lives inside the Memory Hub. Anyone
/// hitting the old `/dashboard/memory` is sent to `/memory?view=table`.
#[component]
fn MemoryVaultRedirect() -> impl IntoView {
    let navigate = use_navigate();
    Effect::new(move |_| {
        navigate(
            "/memory?view=table",
            leptos_router::NavigateOptions::default(),
        );
    });
    ().into_any()
}
