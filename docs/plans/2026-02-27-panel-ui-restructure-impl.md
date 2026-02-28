# Panel UI Restructure Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Restructure the Control Plane UI from a flat sidebar layout to a three-mode panel (Chat/Dashboard/Settings) with bottom navigation bar, context-aware sidebar, and integrated chat.

**Architecture:** Replace the current single flat sidebar (`Sidebar` component) with a new layout: Top Bar + Context Sidebar + Main Content + Bottom Bar. The existing Halo chat module (`views/halo/`) becomes the Chat mode's main content. Dashboard and Settings views are re-grouped under their respective modes. The macOS native `HaloWindow` and `SettingsWindow` are removed.

**Tech Stack:** Leptos 0.8 (CSR), leptos_router, Tailwind CSS (semantic tokens), WASM, Swift/AppKit (macOS native shell)

**Design Doc:** `docs/plans/2026-02-27-panel-ui-restructure-design.md`

---

## Codebase Map

All primary changes are in `core/ui/control_plane/src/`. Secondary changes in `apps/macos-native/Aleph/UI/`.

| File | Role |
|------|------|
| `app.rs` | Root layout + Router |
| `context.rs` | DashboardState (WebSocket, RPC, alerts) |
| `components/sidebar/sidebar.rs` | Current flat sidebar (will be replaced) |
| `components/sidebar/sidebar_item.rs` | Reusable sidebar item (keep) |
| `components/sidebar/types.rs` | SystemAlert types (keep) |
| `components/settings_sidebar.rs` | SettingsTab enum + SETTINGS_GROUPS (keep, extract DashboardTab) |
| `views/halo/` | Chat state/events/view (promote to chat/) |
| `views/home.rs` | Dashboard Overview |
| `views/agent_trace.rs` | Agent Trace |
| `views/system_status.rs` | System Health |
| `views/memory.rs` | Memory Vault |
| `views/settings/` | All settings views (20+ files, untouched) |
| `api/chat.rs` | ChatApi (send/abort/history/clear) |

---

## Task 1: Create Bottom Bar component

**Files:**
- Create: `core/ui/control_plane/src/components/bottom_bar.rs`
- Modify: `core/ui/control_plane/src/components/mod.rs`

**Step 1: Create the BottomBar component**

```rust
// core/ui/control_plane/src/components/bottom_bar.rs
//! Bottom navigation bar — Chat / Dashboard / Settings mode switcher.

use leptos::prelude::*;
use leptos_router::hooks::use_location;
use leptos_router::components::A;

/// Active panel mode derived from current URL path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PanelMode {
    Chat,
    Dashboard,
    Settings,
}

impl PanelMode {
    /// Derive mode from pathname.
    pub fn from_path(path: &str) -> Self {
        if path.starts_with("/dashboard") {
            Self::Dashboard
        } else if path.starts_with("/settings") {
            Self::Settings
        } else {
            Self::Chat // "/" and "/chat/*" both map to Chat
        }
    }
}

#[component]
pub fn BottomBar() -> impl IntoView {
    let location = use_location();
    let mode = Memo::new(move |_| PanelMode::from_path(&location.pathname.get()));

    let items: Vec<(&str, &str, &str, PanelMode)> = vec![
        ("/", "Chat", CHAT_ICON, PanelMode::Chat),
        ("/dashboard", "Dashboard", DASHBOARD_ICON, PanelMode::Dashboard),
        ("/settings", "Settings", SETTINGS_ICON, PanelMode::Settings),
    ];

    view! {
        <nav class="h-12 border-t border-border bg-sidebar flex items-center justify-around flex-shrink-0">
            {items.into_iter().map(|(href, label, icon_svg, item_mode)| {
                let is_active = move || mode.get() == item_mode;
                view! {
                    <A
                        href=href
                        attr:class=move || {
                            if is_active() {
                                "flex flex-col items-center gap-0.5 px-4 py-1 text-sidebar-accent transition-colors"
                            } else {
                                "flex flex-col items-center gap-0.5 px-4 py-1 text-text-tertiary hover:text-text-secondary transition-colors"
                            }
                        }
                    >
                        <svg
                            width="20" height="20" viewBox="0 0 24 24"
                            fill="none" stroke="currentColor" stroke-width="2"
                            stroke-linecap="round" stroke-linejoin="round"
                            inner_html=icon_svg
                        />
                        <span class="text-[10px] font-medium">{label}</span>
                    </A>
                }
            }).collect_view()}
        </nav>
    }
}

// SVG icon paths (Feather icons)
const CHAT_ICON: &str = r#"<path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>"#;
const DASHBOARD_ICON: &str = r#"<rect x="3" y="3" width="7" height="7"/><rect x="14" y="3" width="7" height="7"/><rect x="14" y="14" width="7" height="7"/><rect x="3" y="14" width="7" height="7"/>"#;
const SETTINGS_ICON: &str = r#"<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/>"#;
```

**Step 2: Register in mod.rs**

Add to `core/ui/control_plane/src/components/mod.rs`:
```rust
pub mod bottom_bar;
pub use bottom_bar::{BottomBar, PanelMode};
```

**Step 3: Build and verify compilation**

Run: `cd core && cargo build --features control-plane 2>&1 | tail -20`
Expected: compiles (BottomBar not yet used, just registered)

**Step 4: Commit**

```bash
git add core/ui/control_plane/src/components/bottom_bar.rs core/ui/control_plane/src/components/mod.rs
git commit -m "panel: add BottomBar component with Chat/Dashboard/Settings mode switching"
```

---

## Task 2: Create Top Bar component

**Files:**
- Create: `core/ui/control_plane/src/components/top_bar.rs`
- Modify: `core/ui/control_plane/src/components/mod.rs`

**Step 1: Create the TopBar component**

```rust
// core/ui/control_plane/src/components/top_bar.rs
//! Top bar — logo, title, global actions.

use leptos::prelude::*;
use leptos_router::hooks::use_location;
use super::bottom_bar::PanelMode;

#[component]
pub fn TopBar() -> impl IntoView {
    let location = use_location();
    let mode = Memo::new(move |_| PanelMode::from_path(&location.pathname.get()));

    view! {
        <header class="h-12 border-b border-border bg-sidebar flex items-center justify-between px-4 flex-shrink-0">
            // Left: Logo
            <div class="flex items-center gap-3">
                <div class="w-7 h-7 bg-primary rounded-lg flex items-center justify-center">
                    <span class="text-text-inverse font-bold text-base">"A"</span>
                </div>
                <h1 class="text-sm font-semibold tracking-tight">"Aleph"</h1>
            </div>

            // Right: contextual actions
            <div class="flex items-center gap-2">
                <Show when=move || mode.get() == PanelMode::Chat>
                    <NewChatButton />
                </Show>
            </div>
        </header>
    }
}

#[component]
fn NewChatButton() -> impl IntoView {
    // Navigate to "/" to start a new chat
    view! {
        <a
            href="/"
            class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium
                   text-text-secondary hover:text-text-primary hover:bg-surface-sunken transition-colors"
        >
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                 stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <line x1="12" y1="5" x2="12" y2="19" />
                <line x1="5" y1="12" x2="19" y2="12" />
            </svg>
            "New Chat"
        </a>
    }
}
```

**Step 2: Register in mod.rs**

Add to `core/ui/control_plane/src/components/mod.rs`:
```rust
pub mod top_bar;
pub use top_bar::TopBar;
```

**Step 3: Build and verify**

Run: `cd core && cargo build --features control-plane 2>&1 | tail -20`

**Step 4: Commit**

```bash
git add core/ui/control_plane/src/components/top_bar.rs core/ui/control_plane/src/components/mod.rs
git commit -m "panel: add TopBar component with logo and contextual actions"
```

---

## Task 3: Create context-aware sidebars (Chat, Dashboard, Settings)

**Files:**
- Create: `core/ui/control_plane/src/components/chat_sidebar.rs`
- Create: `core/ui/control_plane/src/components/dashboard_sidebar.rs`
- Create: `core/ui/control_plane/src/components/mode_sidebar.rs`
- Modify: `core/ui/control_plane/src/components/mod.rs`

### Step 1: Create ChatSidebar

```rust
// core/ui/control_plane/src/components/chat_sidebar.rs
//! Chat mode sidebar — session list grouped by project.

use leptos::prelude::*;

/// Placeholder chat sidebar until session API is wired up.
#[component]
pub fn ChatSidebar() -> impl IntoView {
    view! {
        <div class="flex flex-col h-full">
            // Search
            <div class="p-3">
                <div class="flex items-center gap-2 px-3 py-2 rounded-lg bg-surface-sunken border border-border text-sm">
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-text-tertiary flex-shrink-0">
                        <circle cx="11" cy="11" r="8" />
                        <line x1="21" y1="21" x2="16.65" y2="16.65" />
                    </svg>
                    <span class="text-text-tertiary">"Search chats..."</span>
                </div>
            </div>

            // Session list (placeholder)
            <div class="flex-1 overflow-y-auto px-3 py-2 space-y-1">
                <p class="text-xs text-text-tertiary px-3 py-4 text-center">
                    "Start a new conversation"
                </p>
            </div>
        </div>
    }
}
```

### Step 2: Create DashboardSidebar

```rust
// core/ui/control_plane/src/components/dashboard_sidebar.rs
//! Dashboard mode sidebar — sub-navigation for dashboard views.

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_location;
use crate::components::sidebar::SidebarItem;

#[component]
pub fn DashboardSidebar() -> impl IntoView {
    view! {
        <div class="flex flex-col h-full">
            <div class="px-4 py-3">
                <h2 class="text-xs font-medium text-text-tertiary uppercase tracking-wider">"Dashboard"</h2>
            </div>
            <nav class="flex-1 overflow-y-auto px-3 space-y-0.5">
                <SidebarItem href="/dashboard" label="Overview">
                    <path d="M3 9l9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
                    <polyline points="9 22 9 12 15 12 15 22" />
                </SidebarItem>
                <SidebarItem href="/dashboard/trace" label="Agent Trace" alert_key="agent.trace">
                    <polyline points="22 12 18 12 15 21 9 3 6 12 2 12" />
                </SidebarItem>
                <SidebarItem href="/dashboard/health" label="System Health" alert_key="system.health">
                    <rect x="4" y="4" width="16" height="16" rx="2" ry="2" />
                    <rect x="9" y="9" width="6" height="6" />
                    <line x1="9" y1="1" x2="9" y2="4" />
                    <line x1="15" y1="1" x2="15" y2="4" />
                    <line x1="9" y1="20" x2="9" y2="23" />
                    <line x1="15" y1="20" x2="15" y2="23" />
                    <line x1="20" y1="9" x2="23" y2="9" />
                    <line x1="20" y1="15" x2="23" y2="15" />
                    <line x1="1" y1="9" x2="4" y2="9" />
                    <line x1="1" y1="15" x2="4" y2="15" />
                </SidebarItem>
                <SidebarItem href="/dashboard/memory" label="Memory Vault" alert_key="memory.status">
                    <ellipse cx="12" cy="5" rx="9" ry="3" />
                    <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" />
                    <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5" />
                </SidebarItem>
            </nav>
        </div>
    }
}
```

### Step 3: Create ModeSidebar (dispatcher)

```rust
// core/ui/control_plane/src/components/mode_sidebar.rs
//! Context-aware sidebar that switches content based on current panel mode.

use leptos::prelude::*;
use leptos_router::hooks::use_location;
use super::bottom_bar::PanelMode;
use super::chat_sidebar::ChatSidebar;
use super::dashboard_sidebar::DashboardSidebar;
use super::settings_sidebar::SETTINGS_GROUPS;
use crate::components::settings_sidebar::SettingsTab;

#[component]
pub fn ModeSidebar() -> impl IntoView {
    let location = use_location();
    let mode = Memo::new(move |_| PanelMode::from_path(&location.pathname.get()));

    view! {
        <aside class="w-64 border-r border-border bg-sidebar flex flex-col flex-shrink-0 overflow-hidden">
            {move || match mode.get() {
                PanelMode::Chat => view! { <ChatSidebar /> }.into_any(),
                PanelMode::Dashboard => view! { <DashboardSidebar /> }.into_any(),
                PanelMode::Settings => view! { <SettingsSidebar /> }.into_any(),
            }}
        </aside>
    }
}

/// Settings mode sidebar — reuses existing SettingsTab definitions.
#[component]
fn SettingsSidebar() -> impl IntoView {
    let location = use_location();

    view! {
        <div class="flex flex-col h-full overflow-y-auto">
            {SETTINGS_GROUPS.iter().map(|group| {
                view! {
                    <div class="px-4 py-2 space-y-0.5">
                        <h3 class="px-3 py-1 text-xs font-medium text-text-tertiary uppercase tracking-wider">
                            {group.label}
                        </h3>
                        {group.tabs.iter().map(|tab| {
                            let path = tab.path();
                            let tab_label = tab.label();
                            let icon_svg = tab.icon_svg();
                            let is_active = move || location.pathname.get() == path;

                            view! {
                                <a
                                    href=path
                                    class=move || {
                                        if is_active() {
                                            "flex items-center gap-3 px-3 py-2 rounded-lg text-sm transition-all duration-200 bg-sidebar-active text-sidebar-accent font-medium"
                                        } else {
                                            "flex items-center gap-3 px-3 py-2 rounded-lg text-sm transition-all duration-200 hover:bg-sidebar-active/50 text-text-secondary hover:text-text-primary"
                                        }
                                    }
                                >
                                    <svg width="18" height="18" viewBox="0 0 24 24" fill="none"
                                         stroke="currentColor" stroke-width="2" stroke-linecap="round"
                                         stroke-linejoin="round"
                                         class=move || {
                                             if is_active() { "text-sidebar-accent flex-shrink-0" }
                                             else { "text-text-tertiary flex-shrink-0" }
                                         }
                                         inner_html=icon_svg
                                    />
                                    <span>{tab_label}</span>
                                </a>
                            }
                        }).collect_view()}
                    </div>
                }
            }).collect_view()}
        </div>
    }
}
```

### Step 4: Register all in mod.rs

Add to `core/ui/control_plane/src/components/mod.rs`:
```rust
pub mod chat_sidebar;
pub mod dashboard_sidebar;
pub mod mode_sidebar;
pub use mode_sidebar::ModeSidebar;
```

### Step 5: Build and verify

Run: `cd core && cargo build --features control-plane 2>&1 | tail -20`

### Step 6: Commit

```bash
git add core/ui/control_plane/src/components/chat_sidebar.rs \
        core/ui/control_plane/src/components/dashboard_sidebar.rs \
        core/ui/control_plane/src/components/mode_sidebar.rs \
        core/ui/control_plane/src/components/mod.rs
git commit -m "panel: add context-aware sidebars for Chat, Dashboard, Settings modes"
```

---

## Task 4: Promote Halo chat to Chat view and add chat route

The existing `views/halo/` module has a complete chat implementation (HaloState, events, MessageBubble, InputArea). We rename it to `views/chat/` and make it the default view.

**Files:**
- Rename: `core/ui/control_plane/src/views/halo/` → `core/ui/control_plane/src/views/chat/`
- Modify: `core/ui/control_plane/src/views/mod.rs`
- Modify: `core/ui/control_plane/src/views/chat/mod.rs` (was halo/mod.rs)
- Modify: `core/ui/control_plane/src/views/chat/view.rs` (was halo/view.rs)

### Step 1: Rename the directory

```bash
mv core/ui/control_plane/src/views/halo core/ui/control_plane/src/views/chat
```

### Step 2: Update views/mod.rs

Replace in `core/ui/control_plane/src/views/mod.rs`:
```rust
pub mod home;
pub mod system_status;
pub mod agent_trace;
pub mod memory;
pub mod settings;
pub mod chat;
```

### Step 3: Update chat/mod.rs

In `core/ui/control_plane/src/views/chat/mod.rs`, update module comment:
```rust
pub mod events;
pub mod state;
pub mod view;

pub use state::HaloState;
pub use view::HaloView as ChatView;
```

### Step 4: Update chat/view.rs module path comment

In `core/ui/control_plane/src/views/chat/view.rs`, update the module comment on line 1:
```rust
// core/ui/control_plane/src/views/chat/view.rs
```

### Step 5: Update chat/state.rs module path comment

```rust
// core/ui/control_plane/src/views/chat/state.rs
```

### Step 6: Update chat/events.rs module path comment

```rust
// core/ui/control_plane/src/views/chat/events.rs
```

### Step 7: Build and verify

Run: `cd core && cargo build --features control-plane 2>&1 | tail -20`
Expected: May have import errors in app.rs — that's OK, we fix app.rs in the next task.

### Step 8: Commit

```bash
git add -A core/ui/control_plane/src/views/chat/ core/ui/control_plane/src/views/mod.rs
git rm -r --cached core/ui/control_plane/src/views/halo/ 2>/dev/null || true
git commit -m "panel: rename halo/ to chat/ — promote chat as first-class panel mode"
```

---

## Task 5: Restructure app.rs — new layout + route hierarchy

This is the core task. Replace the flat layout with the new three-layer structure.

**Files:**
- Modify: `core/ui/control_plane/src/app.rs`

### Step 1: Rewrite app.rs

Replace the entire content of `core/ui/control_plane/src/app.rs`:

```rust
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
                            <Route path=path!("/chat/:session_id") view=ChatView />

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
```

### Step 2: Build and fix compilation errors

Run: `cd core && cargo build --features control-plane 2>&1 | tail -40`

Fix any remaining import errors. Key changes:
- `HaloView` → `ChatView` (via re-export in chat/mod.rs)
- Removed `use crate::components::Sidebar` (replaced by ModeSidebar)
- Removed `use crate::views::halo::HaloView` (now `crate::views::chat::ChatView`)

### Step 3: Verify the build compiles cleanly

Run: `cd core && cargo build --features control-plane 2>&1 | tail -5`
Expected: `Finished` or warnings only

### Step 4: Commit

```bash
git add core/ui/control_plane/src/app.rs
git commit -m "panel: restructure app layout — TopBar + ModeSidebar + BottomBar with route hierarchy"
```

---

## Task 6: Update Dashboard route paths in existing sidebar references

The old sidebar used `/` for Home, `/trace`, `/status`, `/memory`. Now they live under `/dashboard/*`. Any internal links within dashboard views that point to old paths need updating.

**Files:**
- Check: `core/ui/control_plane/src/views/home.rs` — any internal links
- Check: `core/ui/control_plane/src/views/system_status.rs` — any internal links
- Check: `core/ui/control_plane/src/views/agent_trace.rs` — any internal links
- Check: `core/ui/control_plane/src/views/memory.rs` — any internal links
- Modify: `core/ui/control_plane/src/components/sidebar/sidebar_item.rs` — verify path matching

### Step 1: Search for hardcoded old paths

Run: `grep -rn 'href="/trace"\|href="/status"\|href="/memory"\|href="/"' core/ui/control_plane/src/views/`

Fix any found references by updating to `/dashboard/trace`, `/dashboard/health`, `/dashboard/memory`, `/dashboard`.

### Step 2: Also check SidebarItem's `is_active` logic

Read `core/ui/control_plane/src/components/sidebar/sidebar_item.rs` and verify the active-state detection works with the new `/dashboard/*` paths. If it uses exact match on `location.pathname`, it should work. If it uses `starts_with("/")` for Home, it needs to be changed to `== "/dashboard"`.

### Step 3: Build and verify

Run: `cd core && cargo build --features control-plane 2>&1 | tail -5`

### Step 4: Commit

```bash
git add -A core/ui/control_plane/src/
git commit -m "panel: update internal links to new /dashboard/* route paths"
```

---

## Task 7: Clean up old sidebar (optional removal or keep as dead code initially)

The old `components/sidebar/sidebar.rs` contains the flat `Sidebar` component with `DashboardSection` and `SettingsGroupSection`. It's no longer imported in `app.rs` (replaced by `ModeSidebar`). Remove or mark as deprecated.

**Files:**
- Modify: `core/ui/control_plane/src/components/sidebar/sidebar.rs` — remove or keep
- Modify: `core/ui/control_plane/src/components/sidebar/mod.rs` — remove old Sidebar export if unused
- Modify: `core/ui/control_plane/src/components/mod.rs` — remove old `pub use sidebar::Sidebar`

### Step 1: Check if old Sidebar is still referenced

Run: `grep -rn 'use.*Sidebar\b' core/ui/control_plane/src/ --include='*.rs' | grep -v sidebar`

If only `mod.rs` re-exports it and `app.rs` no longer uses it, it's safe to remove.

### Step 2: Remove old Sidebar component

Delete the body of `DashboardSection`, `SettingsGroupSection`, `ThemeToggle`, `LogoSection` from `sidebar.rs`. Replace with a comment noting the component was replaced by `ModeSidebar`.

Or simply: remove `sidebar.rs` entirely if `SidebarItem` (in `sidebar_item.rs`) is the only thing still needed from the `sidebar/` module.

Keep `sidebar_item.rs` and `types.rs` — they're still used by `DashboardSidebar`.

### Step 3: Update sidebar/mod.rs

```rust
pub mod sidebar_item;
pub mod types;

pub use sidebar_item::SidebarItem;
pub use types::SystemAlert;
```

### Step 4: Update components/mod.rs

Remove `pub use sidebar::Sidebar` (keep `pub use sidebar::SidebarItem`).

### Step 5: Build and verify

Run: `cd core && cargo build --features control-plane 2>&1 | tail -5`

### Step 6: Commit

```bash
git add -A core/ui/control_plane/src/components/sidebar/
git add core/ui/control_plane/src/components/mod.rs
git commit -m "panel: remove old flat Sidebar, keep SidebarItem for reuse"
```

---

## Task 8: Move ThemeToggle to BottomBar or TopBar

The `ThemeToggle` component was in the old sidebar. It should be relocated — either to the BottomBar (left-aligned) or TopBar (right-aligned).

**Files:**
- Modify: `core/ui/control_plane/src/components/bottom_bar.rs` — add ThemeToggle
- OR: Create `core/ui/control_plane/src/components/theme_toggle.rs` — extract standalone

### Step 1: Extract ThemeToggle as standalone component

Copy the `ThemeToggle` component and `ThemeMode` enum from the old `sidebar.rs` into a new file:

Create: `core/ui/control_plane/src/components/theme_toggle.rs`

### Step 2: Add to TopBar right side

Import and render `<ThemeToggle />` in `TopBar` next to the new chat button.

### Step 3: Register in mod.rs

### Step 4: Build and verify

### Step 5: Commit

```bash
git add core/ui/control_plane/src/components/theme_toggle.rs \
        core/ui/control_plane/src/components/top_bar.rs \
        core/ui/control_plane/src/components/mod.rs
git commit -m "panel: extract ThemeToggle into standalone component, add to TopBar"
```

---

## Task 9: Update macOS native app — remove HaloWindow and SettingsWindow

**Files:**
- Delete: `apps/macos-native/Aleph/UI/HaloWindow.swift`
- Delete: `apps/macos-native/Aleph/UI/SettingsWindow.swift`
- Modify: `apps/macos-native/Aleph/AppDelegate.swift`
- Modify: `apps/macos-native/Aleph/UI/MenuBarController.swift`
- Modify: `apps/macos-native/Aleph/UI/GlobalShortcuts.swift`

### Step 1: Remove HaloWindow.swift and SettingsWindow.swift

```bash
rm apps/macos-native/Aleph/UI/HaloWindow.swift
rm apps/macos-native/Aleph/UI/SettingsWindow.swift
```

### Step 2: Update AppDelegate.swift

Remove:
- `private let haloWindow = HaloWindow()`
- `private let settingsWindow = SettingsWindow()`
- `haloWindow.configure(serverPort: ...)` / `settingsWindow.configure(serverPort: ...)`
- Notification observers for `.showHalo` → remove or change to open Panel
- Notification observers for `.showSettings` → remove or change to open Panel at settings route

Add:
- A `PanelWindow` (simple NSWindow + WKWebView) that loads `http://127.0.0.1:{port}/`
- `.showHalo` notification → show Panel and navigate to `/` (chat)
- `.showSettings` notification → show Panel and navigate to `/settings`

### Step 3: Update MenuBarController.swift

Change:
- "Show Halo" → "Show Chat" (still Cmd+Opt+/)
- "Settings..." → "Show Settings" (still Cmd+,)
- Both now post notifications that open the Panel window at different routes

### Step 4: Update GlobalShortcuts.swift

The shortcut action should post `.showChat` (or reuse `.showHalo` renamed) notification.

### Step 5: Update project.yml

Remove HaloWindow.swift and SettingsWindow.swift from the Xcode project sources if listed.

### Step 6: Build macOS app to verify

Run: `cd apps/macos-native && xcodebuild build -scheme Aleph -destination 'platform=macOS' 2>&1 | tail -20`

### Step 7: Commit

```bash
git add -A apps/macos-native/
git commit -m "macos: remove HaloWindow and SettingsWindow, add unified PanelWindow"
```

---

## Task 10: Final integration test and cleanup

### Step 1: Build the full project

```bash
cd core && cargo build --features control-plane 2>&1 | tail -10
```

### Step 2: Run the server and verify UI

```bash
cargo run --bin aleph-server --features control-plane
```

Open `http://127.0.0.1:18790/` in browser. Verify:
- Default view is Chat
- Bottom bar shows Chat/Dashboard/Settings
- Clicking Dashboard shows Overview with sub-navigation sidebar
- Clicking Settings shows settings sidebar with all groups
- Clicking Chat returns to chat view

### Step 3: Clean up any unused imports/dead code

```bash
cargo clippy --features control-plane 2>&1 | grep "unused"
```

### Step 4: Final commit

```bash
git add -A
git commit -m "panel: complete UI restructure — unified three-mode panel layout"
```

---

## Summary

| Task | What | Files Changed |
|------|------|---------------|
| 1 | BottomBar component | +1 new, 1 modified |
| 2 | TopBar component | +1 new, 1 modified |
| 3 | Context-aware sidebars | +3 new, 1 modified |
| 4 | Rename halo → chat | ~4 renamed/modified |
| 5 | Restructure app.rs layout + routes | 1 modified (core change) |
| 6 | Update old route paths | 1-4 modified |
| 7 | Remove old Sidebar | 2-3 modified |
| 8 | Extract ThemeToggle | +1 new, 2 modified |
| 9 | macOS native cleanup | 2 deleted, 3 modified |
| 10 | Integration test | cleanup only |
