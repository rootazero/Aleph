# Channel Configuration Panel Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add visual configuration UI in the Aleph Control Panel (Leptos/WASM) for all 13 social bot channels via a template-driven architecture.

**Architecture:** A `ChannelDefinition` data model describes each channel's config fields. A generic `ChannelConfigTemplate` Leptos component renders any channel's config form from its definition. A `ChannelsOverview` page displays all channels as a card grid. The sidebar collapses into a single "Channels" entry. Uses existing `config.patch` / `channels.list` RPC — no new backend handlers.

**Tech Stack:** Leptos 0.8 (CSR), leptos_router 0.8, tailwind_fuse, serde_json, WASM

**Design Doc:** `docs/plans/2026-02-27-channel-config-panel-design.md`

---

## Codebase Context

**UI Root:** `core/ui/control_plane/src/`

**Key Existing Files:**
- `components/forms.rs` — Reusable form components: `FormField`, `TextInput`, `SelectInput`, `NumberInput`, `SwitchInput`, `SaveButton`, `ErrorMessageDynamic`
- `components/ui/mod.rs` — UI components: `Button`, `Card`, `Badge`, `StatusBadge`, `Tooltip`
- `components/settings_sidebar.rs` — `SettingsTab` enum + `SETTINGS_GROUPS` constant
- `views/settings/channels/mod.rs` — Currently exports 4 channel views
- `views/settings/mod.rs` — Settings module exports
- `app.rs` — Router with all routes
- `api.rs` — RPC API wrappers (1275 lines), `ConfigApi` has `get`/`set`/`list`
- `context.rs` — `DashboardState` with `rpc_call()`, `subscribe_events()`

**Leptos Patterns:**
- Signals: `RwSignal::new()`, `Signal::derive()`
- Context: `expect_context::<DashboardState>()`
- Async: `spawn_local(async move { ... })`
- Effects: `Effect::new(move || { ... })`
- Navigation: `<A href="...">` (static routes, no dynamic params in codebase)
- Props: `#[prop(into, optional)]`, `Signal<T>`, `MaybeSignal<T>`

**RPC Methods Available:**
- `channels.list` → `{ channels: [...], summary: { total, connected, ... } }`
- `channels.status` → single channel status
- `channel.start` / `channel.stop` → lifecycle control
- `config.get` → `{ section: "channels.telegram" }` → channel config
- `config.patch` → `{ "channels.telegram.bot_token": "xxx" }` → save + hot-reload

**Build Verification:** `cd core/ui/control_plane && cargo check`

---

## Task 1: SecretInput Component

**Files:**
- Create: `core/ui/control_plane/src/components/ui/secret_input.rs`
- Modify: `core/ui/control_plane/src/components/ui/mod.rs`

**Step 1: Create SecretInput component**

Create `core/ui/control_plane/src/components/ui/secret_input.rs`:

```rust
//! Password/secret input with show/hide toggle

use leptos::prelude::*;

/// A secret input field with visibility toggle
#[component]
pub fn SecretInput(
    /// Current value
    value: Signal<String>,
    /// Change handler
    on_change: impl Fn(String) + 'static,
    /// Optional placeholder
    #[prop(optional)]
    placeholder: Option<&'static str>,
    /// Use monospace font
    #[prop(optional)]
    monospace: bool,
) -> impl IntoView {
    let visible = RwSignal::new(false);
    let font_class = if monospace { "font-mono" } else { "" };

    view! {
        <div class="relative">
            <input
                type=move || if visible.get() { "text" } else { "password" }
                value=move || value.get()
                on:input=move |ev| on_change(event_target_value(&ev))
                placeholder=placeholder.unwrap_or("")
                class=format!(
                    "w-full px-3 py-2 pr-10 bg-surface-raised border border-border rounded-lg text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30 focus:border-primary {}",
                    font_class
                )
            />
            <button
                type="button"
                on:click=move |_| visible.update(|v| *v = !*v)
                class="absolute right-2 top-1/2 -translate-y-1/2 p-1 text-text-tertiary hover:text-text-secondary transition-colors"
                title=move || if visible.get() { "Hide" } else { "Show" }
            >
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    {move || if visible.get() {
                        // Eye-off icon
                        view! {
                            <path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94"/>
                            <path d="M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19"/>
                            <line x1="1" y1="1" x2="23" y2="23"/>
                        }.into_any()
                    } else {
                        // Eye icon
                        view! {
                            <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/>
                            <circle cx="12" cy="12" r="3"/>
                        }.into_any()
                    }}
                </svg>
            </button>
        </div>
    }
}
```

**Step 2: Register in mod.rs**

Add to `core/ui/control_plane/src/components/ui/mod.rs`:

```rust
pub mod secret_input;
// ... existing mods ...

pub use secret_input::SecretInput;
// ... existing uses ...
```

**Step 3: Build to verify**

Run: `cd core/ui/control_plane && cargo check`
Expected: Compiles without errors

**Step 4: Commit**

```bash
git add core/ui/control_plane/src/components/ui/secret_input.rs core/ui/control_plane/src/components/ui/mod.rs
git commit -m "panel: add SecretInput component with visibility toggle"
```

---

## Task 2: TagListInput Component

**Files:**
- Create: `core/ui/control_plane/src/components/ui/tag_list_input.rs`
- Modify: `core/ui/control_plane/src/components/ui/mod.rs`

**Step 1: Create TagListInput component**

Create `core/ui/control_plane/src/components/ui/tag_list_input.rs`:

```rust
//! Chip-based tag list editor (add/remove tags)

use leptos::prelude::*;

/// A tag list input with add/remove chips
#[component]
pub fn TagListInput(
    /// Current tags
    tags: Signal<Vec<String>>,
    /// Called with full updated tag list when tags change
    on_change: impl Fn(Vec<String>) + 'static + Copy,
    /// Placeholder for the add input
    #[prop(optional)]
    placeholder: Option<&'static str>,
    /// Help text hint for format
    #[prop(optional)]
    hint: Option<&'static str>,
) -> impl IntoView {
    let input_value = RwSignal::new(String::new());

    let add_tag = move || {
        let val = input_value.get().trim().to_string();
        if !val.is_empty() {
            let mut current = tags.get();
            if !current.contains(&val) {
                current.push(val);
                on_change(current);
            }
            input_value.set(String::new());
        }
    };

    let remove_tag = move |idx: usize| {
        let mut current = tags.get();
        if idx < current.len() {
            current.remove(idx);
            on_change(current);
        }
    };

    view! {
        <div class="space-y-2">
            // Tag chips
            {move || {
                let current_tags = tags.get();
                if current_tags.is_empty() {
                    view! {
                        <div class="text-xs text-text-tertiary italic">"No items added"</div>
                    }.into_any()
                } else {
                    view! {
                        <div class="flex flex-wrap gap-2">
                            {current_tags.iter().enumerate().map(|(idx, tag)| {
                                let tag_display = tag.clone();
                                view! {
                                    <span class="inline-flex items-center gap-1 px-2 py-1 bg-primary-subtle text-primary text-xs rounded-md border border-primary/20">
                                        {tag_display}
                                        <button
                                            type="button"
                                            on:click=move |_| remove_tag(idx)
                                            class="ml-0.5 hover:text-danger transition-colors"
                                            title="Remove"
                                        >
                                            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                                                <line x1="18" y1="6" x2="6" y2="18"/>
                                                <line x1="6" y1="6" x2="18" y2="18"/>
                                            </svg>
                                        </button>
                                    </span>
                                }
                            }).collect_view()}
                        </div>
                    }.into_any()
                }
            }}
            // Add input
            <div class="flex gap-2">
                <input
                    type="text"
                    value=move || input_value.get()
                    on:input=move |ev| input_value.set(event_target_value(&ev))
                    on:keydown=move |ev| {
                        if ev.key() == "Enter" {
                            ev.prevent_default();
                            add_tag();
                        }
                    }
                    placeholder=placeholder.unwrap_or("Add item...")
                    class="flex-1 px-3 py-1.5 bg-surface-raised border border-border rounded-lg text-text-primary text-sm focus:outline-none focus:ring-2 focus:ring-primary/30 focus:border-primary"
                />
                <button
                    type="button"
                    on:click=move |_| add_tag()
                    class="px-3 py-1.5 bg-surface-sunken border border-border rounded-lg text-text-secondary hover:text-text-primary hover:bg-surface-raised text-sm transition-colors"
                >
                    "Add"
                </button>
            </div>
            {hint.map(|h| view! {
                <p class="text-xs text-text-tertiary">{h}</p>
            })}
        </div>
    }
}
```

**Step 2: Register in mod.rs**

Add to `core/ui/control_plane/src/components/ui/mod.rs`:

```rust
pub mod tag_list_input;
pub use tag_list_input::TagListInput;
```

**Step 3: Build to verify**

Run: `cd core/ui/control_plane && cargo check`
Expected: Compiles without errors

**Step 4: Commit**

```bash
git add core/ui/control_plane/src/components/ui/tag_list_input.rs core/ui/control_plane/src/components/ui/mod.rs
git commit -m "panel: add TagListInput chip-based tag editor component"
```

---

## Task 3: ChannelStatusBadge Component

The existing `StatusBadge` in `badge.rs` is for system alerts (`AlertLevel`), not channel connection states. We need a channel-specific status badge for the 5-state model: Disconnected, Connecting, Connected, Error, Disabled.

**Files:**
- Create: `core/ui/control_plane/src/components/ui/channel_status.rs`
- Modify: `core/ui/control_plane/src/components/ui/mod.rs`

**Step 1: Create ChannelStatusBadge component**

Create `core/ui/control_plane/src/components/ui/channel_status.rs`:

```rust
//! Channel connection status badge (5-state)

use leptos::prelude::*;

/// Channel connection status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelStatus {
    Disconnected,
    Connecting,
    Connected,
    Error,
    Disabled,
}

impl ChannelStatus {
    pub fn from_str(s: &str) -> Self {
        match s {
            "connected" => Self::Connected,
            "connecting" => Self::Connecting,
            "error" => Self::Error,
            "disabled" => Self::Disabled,
            _ => Self::Disconnected,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Disconnected => "Disconnected",
            Self::Connecting => "Connecting",
            Self::Connected => "Connected",
            Self::Error => "Error",
            Self::Disabled => "Disabled",
        }
    }

    fn dot_class(&self) -> &'static str {
        match self {
            Self::Disconnected => "bg-text-tertiary",
            Self::Connecting => "bg-warning animate-pulse",
            Self::Connected => "bg-success",
            Self::Error => "bg-danger",
            Self::Disabled => "bg-border",
        }
    }

    fn text_class(&self) -> &'static str {
        match self {
            Self::Disconnected => "text-text-tertiary",
            Self::Connecting => "text-warning",
            Self::Connected => "text-success",
            Self::Error => "text-danger",
            Self::Disabled => "text-text-tertiary",
        }
    }
}

/// Inline status badge: colored dot + label
#[component]
pub fn ChannelStatusBadge(
    status: Signal<ChannelStatus>,
) -> impl IntoView {
    view! {
        <span class="inline-flex items-center gap-1.5">
            <span class=move || format!("w-2 h-2 rounded-full {}", status.get().dot_class()) />
            <span class=move || format!("text-xs font-medium {}", status.get().text_class())>
                {move || status.get().label()}
            </span>
        </span>
    }
}

/// Pill-shaped status badge (for cards)
#[component]
pub fn ChannelStatusPill(
    status: Signal<ChannelStatus>,
) -> impl IntoView {
    let pill_class = move || {
        let s = status.get();
        let (bg, text) = match s {
            ChannelStatus::Connected => ("bg-success-subtle", "text-success"),
            ChannelStatus::Connecting => ("bg-warning-subtle", "text-warning"),
            ChannelStatus::Error => ("bg-danger-subtle", "text-danger"),
            _ => ("bg-surface-sunken", "text-text-tertiary"),
        };
        format!("px-2 py-0.5 rounded-full text-xs font-medium {} {}", bg, text)
    };

    view! {
        <span class=pill_class>
            {move || status.get().label()}
        </span>
    }
}
```

**Step 2: Register in mod.rs**

Add to `core/ui/control_plane/src/components/ui/mod.rs`:

```rust
pub mod channel_status;
pub use channel_status::{ChannelStatus, ChannelStatusBadge, ChannelStatusPill};
```

**Step 3: Build to verify**

Run: `cd core/ui/control_plane && cargo check`
Expected: Compiles without errors

**Step 4: Commit**

```bash
git add core/ui/control_plane/src/components/ui/channel_status.rs core/ui/control_plane/src/components/ui/mod.rs
git commit -m "panel: add ChannelStatusBadge and ChannelStatusPill components"
```

---

## Task 4: ChannelDefinition Data Model + All 13 Definitions

**Files:**
- Create: `core/ui/control_plane/src/views/settings/channels/definitions.rs`

**Step 1: Create definitions file**

Create `core/ui/control_plane/src/views/settings/channels/definitions.rs` with the data model and all 13 channel definitions.

Each definition must match the actual config struct fields from `core/src/gateway/interfaces/{channel}/config.rs`.

```rust
//! Channel definition data model and static definitions for all 13 channels.
//!
//! Each ChannelDefinition describes a channel's config fields so the
//! ChannelConfigTemplate can render the form automatically.

/// Field input type for form rendering
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FieldKind {
    /// Plain text input
    Text,
    /// Masked input with show/hide toggle
    Secret,
    /// URL input (renders like text but with format hint)
    Url,
    /// Numeric input
    Number { min: i32, max: i32 },
    /// Boolean switch toggle
    Toggle,
    /// Tag list editor (add/remove chips)
    TagList,
    /// Dropdown select from static options
    Select,
}

/// Single configuration field descriptor
#[derive(Debug, Clone)]
pub struct FieldDef {
    /// TOML key (e.g., "bot_token")
    pub key: &'static str,
    /// Display label (e.g., "Bot Token")
    pub label: &'static str,
    /// Input type
    pub kind: FieldKind,
    /// Placeholder text
    pub placeholder: &'static str,
    /// Help text below the field
    pub help: &'static str,
    /// Whether the field is required
    pub required: bool,
    /// Default value as string (for display; empty = no default)
    pub default_value: &'static str,
    /// For Select fields: (value, label) pairs
    pub options: &'static [(&'static str, &'static str)],
}

/// Complete channel definition
#[derive(Debug, Clone)]
pub struct ChannelDefinition {
    /// Unique identifier (e.g., "telegram")
    pub id: &'static str,
    /// Display name (e.g., "Telegram")
    pub name: &'static str,
    /// Short description
    pub description: &'static str,
    /// SVG path data for the channel icon
    pub icon_svg: &'static str,
    /// Brand hex color (e.g., "#26A5E4")
    pub brand_color: &'static str,
    /// TOML config section (e.g., "channels.telegram")
    pub config_section: &'static str,
    /// Configuration fields
    pub fields: &'static [FieldDef],
    /// External documentation URL
    pub docs_url: &'static str,
}

impl ChannelDefinition {
    /// Find a channel definition by id
    pub fn find(id: &str) -> Option<&'static ChannelDefinition> {
        ALL_CHANNELS.iter().find(|c| c.id == id)
    }
}

// ============================================================================
// Channel Definitions
// ============================================================================

pub static ALL_CHANNELS: &[ChannelDefinition] = &[
    TELEGRAM,
    DISCORD,
    WHATSAPP,
    IMESSAGE,
    SLACK,
    EMAIL,
    MATRIX,
    SIGNAL,
    MATTERMOST,
    IRC,
    WEBHOOK,
    XMPP,
    NOSTR,
];

// --- Telegram ---
pub static TELEGRAM: ChannelDefinition = ChannelDefinition {
    id: "telegram",
    name: "Telegram",
    description: "Connect via Telegram Bot API",
    icon_svg: r#"<path d="M21.2 4.4L2.9 11.3c-1.2.5-1.2 1.2-.2 1.5l4.7 1.5 1.8 5.6c.2.6.1.8.7.8.4 0 .6-.2.9-.4l2.1-2.1 4.4 3.3c.8.4 1.4.2 1.6-.8L22.4 5.6c.3-1.2-.5-1.7-1.2-1.2zM8.5 13.5l9.4-5.9c.4-.3.8-.1.5.2l-7.8 7-.3 3.2-1.8-4.5z"/>"#,
    brand_color: "#26A5E4",
    config_section: "channels.telegram",
    fields: &[
        FieldDef { key: "bot_token", label: "Bot Token", kind: FieldKind::Secret, placeholder: "123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11", help: "Get from @BotFather on Telegram", required: true, default_value: "", options: &[] },
        FieldDef { key: "bot_username", label: "Bot Username", kind: FieldKind::Text, placeholder: "@my_bot", help: "Optional; auto-detected from token", required: false, default_value: "", options: &[] },
        FieldDef { key: "dm_allowed", label: "Allow DMs", kind: FieldKind::Toggle, placeholder: "", help: "Respond to direct messages", required: false, default_value: "true", options: &[] },
        FieldDef { key: "groups_allowed", label: "Allow Groups", kind: FieldKind::Toggle, placeholder: "", help: "Respond in group chats", required: false, default_value: "true", options: &[] },
        FieldDef { key: "send_typing", label: "Send Typing Indicator", kind: FieldKind::Toggle, placeholder: "", help: "Show typing status while processing", required: false, default_value: "true", options: &[] },
        FieldDef { key: "polling_interval_secs", label: "Polling Interval", kind: FieldKind::Number { min: 1, max: 60 }, placeholder: "1", help: "Seconds between poll requests", required: false, default_value: "1", options: &[] },
        FieldDef { key: "allowed_users", label: "Allowed User IDs", kind: FieldKind::TagList, placeholder: "12345678", help: "Telegram user IDs (empty = all allowed)", required: false, default_value: "", options: &[] },
        FieldDef { key: "allowed_groups", label: "Allowed Group IDs", kind: FieldKind::TagList, placeholder: "-100123456789", help: "Telegram group IDs (empty = all allowed)", required: false, default_value: "", options: &[] },
    ],
    docs_url: "https://core.telegram.org/bots",
};

// --- Discord ---
// Note: Discord uses its own complex view (DiscordChannelView), not the template.
// This definition is only for the Overview card.
pub static DISCORD: ChannelDefinition = ChannelDefinition {
    id: "discord",
    name: "Discord",
    description: "Connect via Discord Bot API",
    icon_svg: r#"<path d="M18.59 5.89c-1.23-.57-2.54-.99-3.92-1.23-.17.3-.37.71-.5 1.03-1.46-.22-2.91-.22-4.34 0-.14-.32-.34-.73-.51-1.03-1.38.24-2.69.66-3.92 1.23C2.18 10.73 1.34 15.44 1.76 20.09A18.07 18.07 0 0 0 7.2 22.5c.44-.6.83-1.24 1.17-1.91-.64-.24-1.26-.54-1.84-.89.15-.11.3-.23.45-.34a12.84 12.84 0 0 0 10.04 0c.15.12.3.23.45.34-.58.35-1.2.65-1.84.89.34.67.73 1.31 1.17 1.91a18 18 0 0 0 5.44-2.41c.49-5.15-.84-9.82-3.65-13.61zM8.35 17.24c-1.18 0-2.15-1.09-2.15-2.42s.95-2.42 2.15-2.42 2.17 1.09 2.15 2.42c0 1.33-.95 2.42-2.15 2.42zm6.3 0c-1.18 0-2.15-1.09-2.15-2.42s.95-2.42 2.15-2.42 2.17 1.09 2.15 2.42c0 1.33-.95 2.42-2.15 2.42z"/>"#,
    brand_color: "#5865F2",
    config_section: "channels.discord",
    fields: &[], // Uses dedicated DiscordChannelView
    docs_url: "https://discord.com/developers/docs",
};

// --- WhatsApp ---
pub static WHATSAPP: ChannelDefinition = ChannelDefinition {
    id: "whatsapp",
    name: "WhatsApp",
    description: "Connect via WhatsApp Bridge",
    icon_svg: r#"<path d="M17.47 14.38c-.29-.14-1.7-.84-1.96-.94-.27-.1-.46-.14-.65.14-.2.29-.75.94-.92 1.13-.17.2-.34.22-.63.07-.29-.14-1.22-.45-2.32-1.43-.86-.77-1.44-1.71-1.61-2-.17-.29-.02-.45.13-.59.13-.13.29-.34.44-.51.14-.17.2-.29.29-.48.1-.2.05-.37-.02-.51-.07-.15-.65-1.56-.89-2.14-.24-.56-.48-.49-.65-.49-.17 0-.37-.02-.56-.02-.2 0-.51.07-.78.37-.27.29-1.02 1-1.02 2.43 0 1.43 1.04 2.82 1.19 3.01.14.2 2.05 3.13 4.97 4.39.7.3 1.24.48 1.66.61.7.22 1.33.19 1.83.12.56-.08 1.7-.7 1.94-1.37.24-.68.24-1.26.17-1.38-.07-.12-.27-.2-.56-.34zM12 2C6.48 2 2 6.48 2 12c0 1.77.46 3.43 1.27 4.88L2 22l5.23-1.37A9.93 9.93 0 0 0 12 22c5.52 0 10-4.48 10-10S17.52 2 12 2z"/>"#,
    brand_color: "#25D366",
    config_section: "channels.whatsapp",
    fields: &[
        FieldDef { key: "phone_number", label: "Phone Number", kind: FieldKind::Text, placeholder: "+1234567890", help: "Phone number linked to WhatsApp", required: false, default_value: "", options: &[] },
        FieldDef { key: "send_typing", label: "Send Typing Indicator", kind: FieldKind::Toggle, placeholder: "", help: "", required: false, default_value: "true", options: &[] },
        FieldDef { key: "mark_read", label: "Mark Messages as Read", kind: FieldKind::Toggle, placeholder: "", help: "Send read receipts automatically", required: false, default_value: "true", options: &[] },
        FieldDef { key: "bridge_binary", label: "Bridge Binary Path", kind: FieldKind::Text, placeholder: "/usr/local/bin/whatsapp-bridge", help: "Path to the WhatsApp bridge binary (optional)", required: false, default_value: "", options: &[] },
        FieldDef { key: "max_restarts", label: "Max Restarts", kind: FieldKind::Number { min: 0, max: 20 }, placeholder: "5", help: "Maximum bridge restart attempts", required: false, default_value: "5", options: &[] },
        FieldDef { key: "allowed_chats", label: "Allowed Chats", kind: FieldKind::TagList, placeholder: "+1234567890", help: "Phone numbers or group IDs (empty = all)", required: false, default_value: "", options: &[] },
    ],
    docs_url: "",
};

// --- iMessage ---
pub static IMESSAGE: ChannelDefinition = ChannelDefinition {
    id: "imessage",
    name: "iMessage",
    description: "macOS iMessage integration (requires Full Disk Access)",
    icon_svg: r#"<path d="M20 2H4c-1.1 0-2 .9-2 2v18l4-4h14c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2z"/>"#,
    brand_color: "#34C759",
    config_section: "channels.imessage",
    fields: &[
        FieldDef { key: "enabled", label: "Enabled", kind: FieldKind::Toggle, placeholder: "", help: "Enable iMessage integration", required: false, default_value: "false", options: &[] },
        FieldDef { key: "db_path", label: "Database Path", kind: FieldKind::Text, placeholder: "~/Library/Messages/chat.db", help: "Path to iMessage database", required: false, default_value: "~/Library/Messages/chat.db", options: &[] },
        FieldDef { key: "poll_interval_ms", label: "Poll Interval (ms)", kind: FieldKind::Number { min: 100, max: 10000 }, placeholder: "1000", help: "Database polling interval in milliseconds", required: false, default_value: "1000", options: &[] },
        FieldDef { key: "require_mention", label: "Require Mention", kind: FieldKind::Toggle, placeholder: "", help: "Only respond when mentioned by name", required: false, default_value: "true", options: &[] },
        FieldDef { key: "bot_name", label: "Bot Name", kind: FieldKind::Text, placeholder: "Aleph", help: "Name to respond to in group chats", required: false, default_value: "", options: &[] },
        FieldDef { key: "include_attachments", label: "Include Attachments", kind: FieldKind::Toggle, placeholder: "", help: "Process image/file attachments", required: false, default_value: "true", options: &[] },
        FieldDef { key: "allow_from", label: "DM Allowlist", kind: FieldKind::TagList, placeholder: "+1234567890", help: "Phone numbers/emails (empty = use DM policy)", required: false, default_value: "", options: &[] },
        FieldDef { key: "group_allow_from", label: "Group Allowlist", kind: FieldKind::TagList, placeholder: "chat123456", help: "Group chat identifiers (empty = use group policy)", required: false, default_value: "", options: &[] },
    ],
    docs_url: "",
};

// --- Slack ---
pub static SLACK: ChannelDefinition = ChannelDefinition {
    id: "slack",
    name: "Slack",
    description: "Connect via Socket Mode + REST API",
    icon_svg: r#"<path d="M14.5 2c-.83 0-1.5.67-1.5 1.5v5c0 .83.67 1.5 1.5 1.5h5c.83 0 1.5-.67 1.5-1.5S20.33 7 19.5 7H16V3.5c0-.83-.67-1.5-1.5-1.5zm-5 0C8.67 2 8 2.67 8 3.5V7H4.5C3.67 7 3 7.67 3 8.5S3.67 10 4.5 10h5c.83 0 1.5-.67 1.5-1.5v-5C11 2.67 10.33 2 9.5 2zm5 12c-.83 0-1.5.67-1.5 1.5V17h-3.5c-.83 0-1.5.67-1.5 1.5s.67 1.5 1.5 1.5h5c.83 0 1.5-.67 1.5-1.5v-5c0-.83-.67-1.5-1.5-1.5zm-10 0c-.83 0-1.5.67-1.5 1.5s.67 1.5 1.5 1.5H8v3.5c0 .83.67 1.5 1.5 1.5s1.5-.67 1.5-1.5v-5c0-.83-.67-1.5-1.5-1.5h-5z"/>"#,
    brand_color: "#4A154B",
    config_section: "channels.slack",
    fields: &[
        FieldDef { key: "app_token", label: "App Token", kind: FieldKind::Secret, placeholder: "xapp-1-...", help: "Socket Mode app-level token (starts with xapp-)", required: true, default_value: "", options: &[] },
        FieldDef { key: "bot_token", label: "Bot Token", kind: FieldKind::Secret, placeholder: "xoxb-...", help: "OAuth bot token (starts with xoxb-)", required: true, default_value: "", options: &[] },
        FieldDef { key: "send_typing", label: "Send Typing Indicator", kind: FieldKind::Toggle, placeholder: "", help: "", required: false, default_value: "true", options: &[] },
        FieldDef { key: "dm_allowed", label: "Allow DMs", kind: FieldKind::Toggle, placeholder: "", help: "Respond to direct messages", required: false, default_value: "true", options: &[] },
        FieldDef { key: "allowed_channels", label: "Allowed Channels", kind: FieldKind::TagList, placeholder: "general", help: "Channel names (empty = all channels)", required: false, default_value: "", options: &[] },
    ],
    docs_url: "https://api.slack.com/apps",
};

// --- Email ---
pub static EMAIL: ChannelDefinition = ChannelDefinition {
    id: "email",
    name: "Email",
    description: "Connect via IMAP + SMTP",
    icon_svg: r#"<rect x="2" y="4" width="20" height="16" rx="2"/><polyline points="22,7 12,13 2,7"/>"#,
    brand_color: "#EA4335",
    config_section: "channels.email",
    fields: &[
        FieldDef { key: "imap_host", label: "IMAP Host", kind: FieldKind::Text, placeholder: "imap.gmail.com", help: "IMAP server hostname", required: true, default_value: "", options: &[] },
        FieldDef { key: "imap_port", label: "IMAP Port", kind: FieldKind::Number { min: 1, max: 65535 }, placeholder: "993", help: "", required: false, default_value: "993", options: &[] },
        FieldDef { key: "smtp_host", label: "SMTP Host", kind: FieldKind::Text, placeholder: "smtp.gmail.com", help: "SMTP server hostname", required: true, default_value: "", options: &[] },
        FieldDef { key: "smtp_port", label: "SMTP Port", kind: FieldKind::Number { min: 1, max: 65535 }, placeholder: "587", help: "", required: false, default_value: "587", options: &[] },
        FieldDef { key: "username", label: "Username", kind: FieldKind::Text, placeholder: "bot@example.com", help: "Login username for IMAP/SMTP", required: true, default_value: "", options: &[] },
        FieldDef { key: "password", label: "Password", kind: FieldKind::Secret, placeholder: "", help: "Login password or app-specific password", required: true, default_value: "", options: &[] },
        FieldDef { key: "from_address", label: "From Address", kind: FieldKind::Text, placeholder: "bot@example.com", help: "Sender email address (must contain @)", required: true, default_value: "", options: &[] },
        FieldDef { key: "use_tls", label: "Use TLS", kind: FieldKind::Toggle, placeholder: "", help: "Enable TLS encryption", required: false, default_value: "true", options: &[] },
        FieldDef { key: "poll_interval_secs", label: "Poll Interval (secs)", kind: FieldKind::Number { min: 5, max: 3600 }, placeholder: "30", help: "Seconds between inbox checks", required: false, default_value: "30", options: &[] },
        FieldDef { key: "folders", label: "Folders", kind: FieldKind::TagList, placeholder: "INBOX", help: "IMAP folders to monitor (default: INBOX)", required: false, default_value: "", options: &[] },
        FieldDef { key: "allowed_senders", label: "Allowed Senders", kind: FieldKind::TagList, placeholder: "user@example.com", help: "Email addresses (empty = all allowed)", required: false, default_value: "", options: &[] },
    ],
    docs_url: "",
};

// --- Matrix ---
pub static MATRIX: ChannelDefinition = ChannelDefinition {
    id: "matrix",
    name: "Matrix",
    description: "Connect via Client-Server API v3",
    icon_svg: r#"<path d="M1 3h2v18H1V3zm20 0h2v18h-2V3zM5 3h1v1H5V3zm1 1h1v1H6V4zm1 1h1v1H7V5zm1 1h2v1H8V6zm3 0h2v1h-2V6zm3 0h1v1h-1V6zm1-1h1v1h-1V5zm1-1h1v1h-1V4zm1-1h1v1h-1V3zM5 21h1v-1H5v1zm1-1h1v-1H6v1zm1-1h1v-1H7v1zm1-1h2v-1H8v1zm3 0h2v-1h-2v1zm3 0h1v-1h-1v1zm1 1h1v-1h-1v1zm1 1h1v-1h-1v1zm1 1h1v-1h-1v1z"/>"#,
    brand_color: "#0DBD8B",
    config_section: "channels.matrix",
    fields: &[
        FieldDef { key: "homeserver_url", label: "Homeserver URL", kind: FieldKind::Url, placeholder: "https://matrix.org", help: "Matrix homeserver URL", required: true, default_value: "", options: &[] },
        FieldDef { key: "access_token", label: "Access Token", kind: FieldKind::Secret, placeholder: "", help: "Bot account access token", required: true, default_value: "", options: &[] },
        FieldDef { key: "display_name", label: "Display Name", kind: FieldKind::Text, placeholder: "Aleph Bot", help: "Bot display name in rooms", required: false, default_value: "", options: &[] },
        FieldDef { key: "send_typing", label: "Send Typing Indicator", kind: FieldKind::Toggle, placeholder: "", help: "", required: false, default_value: "true", options: &[] },
        FieldDef { key: "allowed_rooms", label: "Allowed Rooms", kind: FieldKind::TagList, placeholder: "!roomid:matrix.org", help: "Room IDs (empty = all joined rooms)", required: false, default_value: "", options: &[] },
    ],
    docs_url: "https://spec.matrix.org/latest/client-server-api/",
};

// --- Signal ---
pub static SIGNAL: ChannelDefinition = ChannelDefinition {
    id: "signal",
    name: "Signal",
    description: "Connect via signal-cli REST API",
    icon_svg: r#"<path d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm-2 15l-5-5 1.41-1.41L10 14.17l7.59-7.59L19 8l-9 9z"/>"#,
    brand_color: "#3A76F0",
    config_section: "channels.signal",
    fields: &[
        FieldDef { key: "api_url", label: "API URL", kind: FieldKind::Url, placeholder: "http://localhost:8080", help: "signal-cli REST API endpoint", required: false, default_value: "http://localhost:8080", options: &[] },
        FieldDef { key: "phone_number", label: "Phone Number", kind: FieldKind::Text, placeholder: "+1234567890", help: "Registered Signal phone number (must start with +)", required: true, default_value: "", options: &[] },
        FieldDef { key: "send_typing", label: "Send Typing Indicator", kind: FieldKind::Toggle, placeholder: "", help: "", required: false, default_value: "true", options: &[] },
        FieldDef { key: "poll_interval_secs", label: "Poll Interval (secs)", kind: FieldKind::Number { min: 1, max: 60 }, placeholder: "2", help: "Seconds between message checks", required: false, default_value: "2", options: &[] },
        FieldDef { key: "allowed_users", label: "Allowed Users", kind: FieldKind::TagList, placeholder: "+1234567890", help: "Phone numbers (empty = all allowed)", required: false, default_value: "", options: &[] },
    ],
    docs_url: "https://github.com/bbernhard/signal-cli-rest-api",
};

// --- Mattermost ---
pub static MATTERMOST: ChannelDefinition = ChannelDefinition {
    id: "mattermost",
    name: "Mattermost",
    description: "Connect via WebSocket + REST API v4",
    icon_svg: r#"<path d="M12 2C6.48 2 2 6.48 2 12c0 2.17.7 4.19 1.88 5.83L2 22l4.17-1.88C7.81 21.3 9.83 22 12 22c5.52 0 10-4.48 10-10S17.52 2 12 2z"/>"#,
    brand_color: "#0058CC",
    config_section: "channels.mattermost",
    fields: &[
        FieldDef { key: "server_url", label: "Server URL", kind: FieldKind::Url, placeholder: "https://mattermost.example.com", help: "Mattermost server URL (https://)", required: true, default_value: "", options: &[] },
        FieldDef { key: "bot_token", label: "Bot Token", kind: FieldKind::Secret, placeholder: "", help: "Bot account personal access token", required: true, default_value: "", options: &[] },
        FieldDef { key: "send_typing", label: "Send Typing Indicator", kind: FieldKind::Toggle, placeholder: "", help: "", required: false, default_value: "true", options: &[] },
        FieldDef { key: "allowed_channels", label: "Allowed Channels", kind: FieldKind::TagList, placeholder: "town-square", help: "Channel names (empty = all channels)", required: false, default_value: "", options: &[] },
    ],
    docs_url: "https://api.mattermost.com/",
};

// --- IRC ---
pub static IRC: ChannelDefinition = ChannelDefinition {
    id: "irc",
    name: "IRC",
    description: "Connect via RFC 2812 raw TCP",
    icon_svg: r#"<path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>"#,
    brand_color: "#6B7280",
    config_section: "channels.irc",
    fields: &[
        FieldDef { key: "server", label: "Server", kind: FieldKind::Text, placeholder: "irc.libera.chat", help: "IRC server hostname", required: true, default_value: "", options: &[] },
        FieldDef { key: "port", label: "Port", kind: FieldKind::Number { min: 1, max: 65535 }, placeholder: "6667", help: "Server port (6697 for TLS)", required: false, default_value: "6667", options: &[] },
        FieldDef { key: "nick", label: "Nickname", kind: FieldKind::Text, placeholder: "aleph-bot", help: "Bot IRC nickname", required: true, default_value: "", options: &[] },
        FieldDef { key: "password", label: "Server Password", kind: FieldKind::Secret, placeholder: "", help: "IRC server password (optional)", required: false, default_value: "", options: &[] },
        FieldDef { key: "use_tls", label: "Use TLS", kind: FieldKind::Toggle, placeholder: "", help: "Enable TLS encryption", required: false, default_value: "false", options: &[] },
        FieldDef { key: "realname", label: "Real Name", kind: FieldKind::Text, placeholder: "Aleph Bot", help: "IRC GECOS/real name field", required: false, default_value: "Aleph Bot", options: &[] },
        FieldDef { key: "channels", label: "Channels", kind: FieldKind::TagList, placeholder: "#general", help: "IRC channels to join (must start with # or &)", required: true, default_value: "", options: &[] },
    ],
    docs_url: "",
};

// --- Webhook ---
pub static WEBHOOK: ChannelDefinition = ChannelDefinition {
    id: "webhook",
    name: "Webhook",
    description: "Generic bidirectional HTTP webhook",
    icon_svg: r#"<path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/>"#,
    brand_color: "#8B5CF6",
    config_section: "channels.webhook",
    fields: &[
        FieldDef { key: "secret", label: "HMAC Secret", kind: FieldKind::Secret, placeholder: "", help: "HMAC-SHA256 secret for signature verification", required: true, default_value: "", options: &[] },
        FieldDef { key: "callback_url", label: "Callback URL", kind: FieldKind::Url, placeholder: "https://example.com/callback", help: "URL to POST outbound messages to", required: true, default_value: "", options: &[] },
        FieldDef { key: "path", label: "Receive Path", kind: FieldKind::Text, placeholder: "/webhook/generic", help: "URL path for inbound webhooks (must start with /)", required: false, default_value: "/webhook/generic", options: &[] },
        FieldDef { key: "allowed_senders", label: "Allowed Senders", kind: FieldKind::TagList, placeholder: "sender-id", help: "Sender IDs (empty = all allowed)", required: false, default_value: "", options: &[] },
    ],
    docs_url: "",
};

// --- XMPP ---
pub static XMPP: ChannelDefinition = ChannelDefinition {
    id: "xmpp",
    name: "XMPP",
    description: "Connect via RFC 6120/6121 + XEP-0045 MUC",
    icon_svg: r#"<circle cx="12" cy="12" r="10"/><path d="M8 14s1.5 2 4 2 4-2 4-2"/><line x1="9" y1="9" x2="9.01" y2="9"/><line x1="15" y1="9" x2="15.01" y2="9"/>"#,
    brand_color: "#002B5C",
    config_section: "channels.xmpp",
    fields: &[
        FieldDef { key: "jid", label: "JID", kind: FieldKind::Text, placeholder: "bot@example.com", help: "XMPP Jabber ID (must contain @)", required: true, default_value: "", options: &[] },
        FieldDef { key: "password", label: "Password", kind: FieldKind::Secret, placeholder: "", help: "XMPP account password", required: true, default_value: "", options: &[] },
        FieldDef { key: "server", label: "Server Override", kind: FieldKind::Text, placeholder: "", help: "Override server from JID (optional)", required: false, default_value: "", options: &[] },
        FieldDef { key: "port", label: "Port", kind: FieldKind::Number { min: 1, max: 65535 }, placeholder: "5222", help: "", required: false, default_value: "5222", options: &[] },
        FieldDef { key: "use_tls", label: "Use TLS", kind: FieldKind::Toggle, placeholder: "", help: "Enable STARTTLS", required: false, default_value: "true", options: &[] },
        FieldDef { key: "nick", label: "MUC Nickname", kind: FieldKind::Text, placeholder: "aleph", help: "Nickname in multi-user chat rooms", required: false, default_value: "aleph", options: &[] },
        FieldDef { key: "muc_rooms", label: "MUC Rooms", kind: FieldKind::TagList, placeholder: "room@conference.example.com", help: "Multi-user chat room JIDs (must contain @)", required: false, default_value: "", options: &[] },
    ],
    docs_url: "https://xmpp.org/extensions/",
};

// --- Nostr ---
pub static NOSTR: ChannelDefinition = ChannelDefinition {
    id: "nostr",
    name: "Nostr",
    description: "Connect via NIP-01 relay + NIP-04 DM",
    icon_svg: r#"<circle cx="12" cy="12" r="10"/><path d="M12 6v6l4 2"/>"#,
    brand_color: "#8B5CF6",
    config_section: "channels.nostr",
    fields: &[
        FieldDef { key: "private_key", label: "Private Key", kind: FieldKind::Secret, placeholder: "64 hex characters", help: "Schnorr private key (64 hex chars, 32 bytes)", required: true, default_value: "", options: &[] },
        FieldDef { key: "relays", label: "Relays", kind: FieldKind::TagList, placeholder: "wss://relay.damus.io", help: "Nostr relay URLs (ws:// or wss://)", required: true, default_value: "", options: &[] },
        FieldDef { key: "allowed_pubkeys", label: "Allowed Pubkeys", kind: FieldKind::TagList, placeholder: "64 hex character pubkey", help: "Public keys to respond to (empty = all)", required: false, default_value: "", options: &[] },
    ],
    docs_url: "https://github.com/nostr-protocol/nips",
};
```

**Step 2: Build to verify**

Run: `cd core/ui/control_plane && cargo check`
Expected: Compiles (definitions.rs is not yet imported from mod.rs — will be wired in Task 7)

**Step 3: Commit**

```bash
git add core/ui/control_plane/src/views/settings/channels/definitions.rs
git commit -m "panel: add ChannelDefinition model and all 13 channel definitions"
```

---

## Task 5: ChannelCard Component

**Files:**
- Create: `core/ui/control_plane/src/components/ui/channel_card.rs`
- Modify: `core/ui/control_plane/src/components/ui/mod.rs`

**Step 1: Create ChannelCard component**

Create `core/ui/control_plane/src/components/ui/channel_card.rs`:

```rust
//! Channel overview card for the Channels Overview page

use leptos::prelude::*;
use leptos_router::components::A;
use super::channel_status::{ChannelStatus, ChannelStatusPill};

/// A card representing a single channel in the overview grid
#[component]
pub fn ChannelCard(
    /// Channel identifier (used for navigation)
    id: &'static str,
    /// Display name
    name: &'static str,
    /// Short description
    description: &'static str,
    /// SVG path data for icon
    icon_svg: &'static str,
    /// Brand color hex (e.g., "#26A5E4")
    brand_color: &'static str,
    /// Connection status signal
    status: Signal<ChannelStatus>,
) -> impl IntoView {
    let href = format!("/settings/channels/{}", id);
    let is_configured = Signal::derive(move || {
        matches!(status.get(), ChannelStatus::Connected | ChannelStatus::Connecting | ChannelStatus::Error)
    });

    view! {
        <A
            href=href
            attr:class="block p-4 bg-surface-raised border border-border rounded-xl hover:border-primary/40 hover:shadow-sm transition-all group"
        >
            <div class="flex items-start justify-between mb-3">
                <div class="w-10 h-10 rounded-lg flex items-center justify-center" style=format!("background-color: {}15", brand_color)>
                    <svg
                        width="20" height="20" viewBox="0 0 24 24"
                        fill="none" stroke=brand_color stroke-width="2"
                        stroke-linecap="round" stroke-linejoin="round"
                    >
                        <g inner_html=icon_svg />
                    </svg>
                </div>
                <ChannelStatusPill status=status />
            </div>
            <h3 class="text-sm font-semibold text-text-primary mb-0.5 group-hover:text-primary transition-colors">
                {name}
            </h3>
            <p class="text-xs text-text-tertiary line-clamp-2">{description}</p>
            <div class="mt-3 text-xs font-medium">
                {move || if is_configured.get() {
                    view! { <span class="text-primary">"Configure"</span> }.into_any()
                } else {
                    view! { <span class="text-text-secondary group-hover:text-primary transition-colors">"Set up"</span> }.into_any()
                }}
            </div>
        </A>
    }
}
```

**Step 2: Register in mod.rs**

Add to `core/ui/control_plane/src/components/ui/mod.rs`:

```rust
pub mod channel_card;
pub use channel_card::ChannelCard;
```

**Step 3: Build to verify**

Run: `cd core/ui/control_plane && cargo check`
Expected: Compiles without errors

**Step 4: Commit**

```bash
git add core/ui/control_plane/src/components/ui/channel_card.rs core/ui/control_plane/src/components/ui/mod.rs
git commit -m "panel: add ChannelCard component for overview grid"
```

---

## Task 6: ChannelConfigTemplate Component

This is the core template that renders any channel's config form from its `ChannelDefinition`.

**Files:**
- Create: `core/ui/control_plane/src/views/settings/channels/config_template.rs`

**Step 1: Create config template**

Create `core/ui/control_plane/src/views/settings/channels/config_template.rs`:

```rust
//! Generic channel configuration template component.
//!
//! Renders a complete config form from a ChannelDefinition,
//! with load/save via config.get/config.patch RPC and
//! connect/disconnect via channel.start/channel.stop.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use serde_json::{json, Value};

use crate::context::DashboardState;
use crate::components::forms::*;
use crate::components::ui::SecretInput;
use crate::components::ui::TagListInput;
use crate::components::ui::channel_status::{ChannelStatus, ChannelStatusBadge};
use super::definitions::{ChannelDefinition, FieldKind};

/// Generic channel configuration page
#[component]
pub fn ChannelConfigTemplate(
    /// The channel definition to render
    definition: &'static ChannelDefinition,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let config_section = definition.config_section;

    // Reactive state
    let field_values = RwSignal::new(serde_json::Map::new());
    let loading = RwSignal::new(true);
    let saving = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);
    let success = RwSignal::new(Option::<String>::None);
    let channel_status = RwSignal::new(ChannelStatus::Disconnected);

    // Load config on mount
    Effect::new(move || {
        if state.is_connected.get() {
            spawn_local(async move {
                loading.set(true);
                let params = json!({ "section": config_section });
                match state.rpc_call("config.get", params).await {
                    Ok(val) => {
                        if let Some(obj) = val.as_object() {
                            field_values.set(obj.clone());
                        }
                        error.set(None);
                    }
                    Err(e) => {
                        error.set(Some(format!("Failed to load config: {}", e)));
                    }
                }
                loading.set(false);
            });

            // Also fetch channel status
            let channel_id = definition.id;
            spawn_local(async move {
                let params = json!({ "channel_id": channel_id });
                if let Ok(val) = state.rpc_call("channels.status", params).await {
                    if let Some(status_str) = val.get("status").and_then(|s| s.as_str()) {
                        channel_status.set(ChannelStatus::from_str(status_str));
                    }
                }
            });
        }
    });

    // Save handler
    let save_config = move || {
        saving.set(true);
        success.set(None);
        error.set(None);

        spawn_local(async move {
            let values = field_values.get();
            let mut patch = serde_json::Map::new();
            for (key, value) in values.iter() {
                let full_key = format!("{}.{}", config_section, key);
                patch.insert(full_key, value.clone());
            }

            match state.rpc_call("config.patch", Value::Object(patch)).await {
                Ok(_) => {
                    success.set(Some("Configuration saved".to_string()));
                }
                Err(e) => {
                    error.set(Some(format!("Failed to save: {}", e)));
                }
            }
            saving.set(false);
        });
    };

    // Connect/Disconnect handlers
    let connect = move || {
        let channel_id = definition.id;
        spawn_local(async move {
            channel_status.set(ChannelStatus::Connecting);
            let params = json!({ "channel_id": channel_id });
            match state.rpc_call("channel.start", params).await {
                Ok(_) => channel_status.set(ChannelStatus::Connected),
                Err(_) => channel_status.set(ChannelStatus::Error),
            }
        });
    };

    let disconnect = move || {
        let channel_id = definition.id;
        spawn_local(async move {
            let params = json!({ "channel_id": channel_id });
            let _ = state.rpc_call("channel.stop", params).await;
            channel_status.set(ChannelStatus::Disconnected);
        });
    };

    let status_signal = Signal::derive(move || channel_status.get());

    view! {
        <div class="flex-1 p-6 overflow-y-auto bg-surface">
            <div class="max-w-3xl space-y-6">
                // Back link
                <A href="/settings/channels" attr:class="inline-flex items-center gap-1 text-sm text-text-secondary hover:text-primary transition-colors">
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="15 18 9 12 15 6"/></svg>
                    "Back to Channels"
                </A>

                // Header
                <div>
                    <div class="flex items-center gap-3 mb-1">
                        <div class="w-10 h-10 rounded-lg flex items-center justify-center" style=format!("background-color: {}15", definition.brand_color)>
                            <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke={definition.brand_color} stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                <g inner_html=definition.icon_svg />
                            </svg>
                        </div>
                        <div>
                            <h1 class="text-2xl font-semibold text-text-primary">{definition.name}</h1>
                            <p class="text-sm text-text-secondary">{definition.description}</p>
                        </div>
                    </div>
                </div>

                // Connection Status
                <div class="p-4 bg-surface-raised border border-border rounded-xl">
                    <div class="flex items-center justify-between">
                        <div class="flex items-center gap-3">
                            <ChannelStatusBadge status=status_signal />
                        </div>
                        <div class="flex gap-2">
                            <button
                                on:click=move |_| connect()
                                class="px-3 py-1.5 bg-success-subtle text-success border border-success/20 rounded-lg text-xs font-medium hover:bg-success/20 transition-colors"
                            >
                                "Connect"
                            </button>
                            <button
                                on:click=move |_| disconnect()
                                class="px-3 py-1.5 bg-surface-sunken text-text-secondary border border-border rounded-lg text-xs font-medium hover:bg-surface-raised transition-colors"
                            >
                                "Disconnect"
                            </button>
                        </div>
                    </div>
                </div>

                // Error/Success messages
                <ErrorMessageDynamic error=Signal::derive(move || error.get()) />
                {move || success.get().map(|msg| view! {
                    <div class="p-4 bg-success-subtle border border-success/30 rounded-lg text-success text-sm">{msg}</div>
                })}

                // Loading state
                {move || if loading.get() {
                    view! { <div class="text-sm text-text-secondary">"Loading configuration..."</div> }.into_any()
                } else {
                    view! {
                        // Configuration fields
                        <SettingsSection title="Configuration">
                            {definition.fields.iter().map(|field| {
                                render_field(field, field_values)
                            }).collect_view()}
                        </SettingsSection>
                    }.into_any()
                }}

                // Actions
                <div class="flex items-center gap-3">
                    <SaveButton
                        on_click=save_config
                        loading=Signal::derive(move || saving.get())
                        text="Save Configuration"
                    />
                    {(!definition.docs_url.is_empty()).then(|| {
                        let url = definition.docs_url;
                        view! {
                            <a
                                href=url
                                target="_blank"
                                rel="noopener noreferrer"
                                class="px-4 py-2 text-sm text-text-secondary hover:text-primary transition-colors"
                            >
                                "Documentation"
                                <svg class="inline ml-1 -mt-0.5" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                                    <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>
                                    <polyline points="15 3 21 3 21 9"/>
                                    <line x1="10" y1="14" x2="21" y2="3"/>
                                </svg>
                            </a>
                        }
                    })}
                </div>
            </div>
        </div>
    }
}

/// Render a single field based on its FieldKind
fn render_field(
    field: &'static crate::views::settings::channels::definitions::FieldDef,
    field_values: RwSignal<serde_json::Map<String, Value>>,
) -> impl IntoView {
    let key = field.key;
    let label = field.label;
    let help = field.help;
    let placeholder = field.placeholder;
    let required = field.required;

    // Build the label with required indicator
    let full_label = if required {
        format!("{} *", label)
    } else {
        label.to_string()
    };

    // Helper to get string value from field_values
    let get_string_val = move || -> String {
        field_values.get()
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };

    let get_bool_val = move || -> bool {
        field_values.get()
            .get(key)
            .and_then(|v| v.as_bool())
            .unwrap_or(field.default_value == "true")
    };

    let get_number_val = move || -> i32 {
        field_values.get()
            .get(key)
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| field.default_value.parse().unwrap_or(0)) as i32
    };

    let get_tags_val = move || -> Vec<String> {
        field_values.get()
            .get(key)
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| {
                if let Some(s) = v.as_str() {
                    Some(s.to_string())
                } else if let Some(n) = v.as_i64() {
                    Some(n.to_string())
                } else {
                    None
                }
            }).collect())
            .unwrap_or_default()
    };

    let set_value = move |val: Value| {
        field_values.update(|map| {
            map.insert(key.to_string(), val);
        });
    };

    match field.kind {
        FieldKind::Text | FieldKind::Url => {
            let val_signal = Signal::derive(get_string_val);
            view! {
                <FormField label=Box::leak(full_label.into_boxed_str()) help_text={if help.is_empty() { None } else { Some(help) }}>
                    <TextInput
                        value=val_signal
                        on_change=move |v| set_value(Value::String(v))
                        placeholder=Some(placeholder)
                        monospace={matches!(field.kind, FieldKind::Url)}
                    />
                </FormField>
            }.into_any()
        }
        FieldKind::Secret => {
            let val_signal = Signal::derive(get_string_val);
            view! {
                <FormField label=Box::leak(full_label.into_boxed_str()) help_text={if help.is_empty() { None } else { Some(help) }}>
                    <SecretInput
                        value=val_signal
                        on_change=move |v| set_value(Value::String(v))
                        placeholder=Some(placeholder)
                        monospace=true
                    />
                </FormField>
            }.into_any()
        }
        FieldKind::Number { min, max } => {
            let val_signal = Signal::derive(get_number_val);
            view! {
                <FormField label=Box::leak(full_label.into_boxed_str()) help_text={if help.is_empty() { None } else { Some(help) }}>
                    <NumberInput
                        value=val_signal
                        on_change=move |v| set_value(Value::Number(serde_json::Number::from(v)))
                        min=min
                        max=max
                    />
                </FormField>
            }.into_any()
        }
        FieldKind::Toggle => {
            let val_signal = Signal::derive(get_bool_val);
            view! {
                <FormField label=Box::leak(full_label.into_boxed_str()) help_text={if help.is_empty() { None } else { Some(help) }}>
                    <SwitchInput
                        checked=val_signal
                        on_change=move |v| set_value(Value::Bool(v))
                    />
                </FormField>
            }.into_any()
        }
        FieldKind::TagList => {
            let val_signal = Signal::derive(get_tags_val);
            view! {
                <FormField label=Box::leak(full_label.into_boxed_str()) help_text={if help.is_empty() { None } else { Some(help) }}>
                    <TagListInput
                        tags=val_signal
                        on_change=move |tags| {
                            let arr: Vec<Value> = tags.into_iter().map(Value::String).collect();
                            set_value(Value::Array(arr));
                        }
                        placeholder=Some(placeholder)
                    />
                </FormField>
            }.into_any()
        }
        FieldKind::Select => {
            let val_signal = Signal::derive(get_string_val);
            let options: Vec<(&'static str, &'static str)> = field.options.to_vec();
            view! {
                <FormField label=Box::leak(full_label.into_boxed_str()) help_text={if help.is_empty() { None } else { Some(help) }}>
                    <SelectInput
                        value=val_signal
                        on_change=move |v| set_value(Value::String(v))
                        options=options
                    />
                </FormField>
            }.into_any()
        }
    }
}
```

**Step 2: Build to verify**

Run: `cd core/ui/control_plane && cargo check`
Expected: May have warnings about unused imports until wired in Task 7

**Step 3: Commit**

```bash
git add core/ui/control_plane/src/views/settings/channels/config_template.rs
git commit -m "panel: add ChannelConfigTemplate generic form renderer"
```

---

## Task 7: ChannelsOverview Page

**Files:**
- Create: `core/ui/control_plane/src/views/settings/channels/overview.rs`

**Step 1: Create overview page**

Create `core/ui/control_plane/src/views/settings/channels/overview.rs`:

```rust
//! Channels Overview page — card grid showing all channels with status

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::json;

use crate::context::DashboardState;
use crate::components::ui::ChannelCard;
use crate::components::ui::channel_status::ChannelStatus;
use super::definitions::ALL_CHANNELS;

/// Channels Overview page
#[component]
pub fn ChannelsOverview() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // Map of channel_id -> status string from RPC
    let statuses = RwSignal::new(std::collections::HashMap::<String, String>::new());

    // Fetch channel statuses on mount
    Effect::new(move || {
        if state.is_connected.get() {
            spawn_local(async move {
                match state.rpc_call("channels.list", json!({})).await {
                    Ok(val) => {
                        if let Some(channels) = val.get("channels").and_then(|c| c.as_array()) {
                            let mut map = std::collections::HashMap::new();
                            for ch in channels {
                                if let (Some(id), Some(status)) = (
                                    ch.get("channel_type").and_then(|v| v.as_str()),
                                    ch.get("status").and_then(|v| v.as_str()),
                                ) {
                                    map.insert(id.to_string(), status.to_string());
                                }
                            }
                            statuses.set(map);
                        }
                    }
                    Err(_) => {} // Silently ignore — cards show "Disconnected"
                }
            });
        }
    });

    view! {
        <div class="flex-1 p-6 overflow-y-auto bg-surface">
            <div class="max-w-5xl">
                // Header
                <div class="mb-6">
                    <h1 class="text-2xl font-semibold text-text-primary mb-1">"Channels"</h1>
                    <p class="text-sm text-text-secondary">
                        "Manage your messaging integrations. Click a channel to configure it."
                    </p>
                </div>

                // Card grid
                <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4">
                    {ALL_CHANNELS.iter().map(|def| {
                        let channel_id = def.id.to_string();
                        let status_signal = Signal::derive(move || {
                            let map = statuses.get();
                            map.get(&channel_id)
                                .map(|s| ChannelStatus::from_str(s))
                                .unwrap_or(ChannelStatus::Disconnected)
                        });

                        view! {
                            <ChannelCard
                                id=def.id
                                name=def.name
                                description=def.description
                                icon_svg=def.icon_svg
                                brand_color=def.brand_color
                                status=status_signal
                            />
                        }
                    }).collect_view()}
                </div>
            </div>
        </div>
    }
}
```

**Step 2: Build to verify**

Run: `cd core/ui/control_plane && cargo check`

**Step 3: Commit**

```bash
git add core/ui/control_plane/src/views/settings/channels/overview.rs
git commit -m "panel: add ChannelsOverview card grid page"
```

---

## Task 8: Wire Everything — mod.rs, Sidebar, Router

This task connects all the new components by updating the module exports, sidebar navigation, and router.

**Files:**
- Modify: `core/ui/control_plane/src/views/settings/channels/mod.rs`
- Modify: `core/ui/control_plane/src/views/settings/mod.rs`
- Modify: `core/ui/control_plane/src/components/settings_sidebar.rs`
- Modify: `core/ui/control_plane/src/app.rs`

**Step 1: Update channels/mod.rs**

Replace `core/ui/control_plane/src/views/settings/channels/mod.rs` with:

```rust
pub mod telegram;
pub mod discord;
pub mod whatsapp;
pub mod imessage;
pub mod definitions;
pub mod config_template;
pub mod overview;

pub use telegram::TelegramChannelView;
pub use discord::DiscordChannelView;
pub use whatsapp::WhatsAppChannelView;
pub use imessage::IMessageChannelView;
pub use config_template::ChannelConfigTemplate;
pub use overview::ChannelsOverview;
```

**Step 2: Update views/settings/mod.rs**

Add imports for the new views. Change:

```rust
pub use channels::TelegramChannelView;
pub use channels::DiscordChannelView;
pub use channels::WhatsAppChannelView;
pub use channels::IMessageChannelView;
```

To:

```rust
pub use channels::TelegramChannelView;
pub use channels::DiscordChannelView;
pub use channels::WhatsAppChannelView;
pub use channels::IMessageChannelView;
pub use channels::ChannelsOverview;
pub use channels::ChannelConfigTemplate;
```

**Step 3: Update settings_sidebar.rs**

In `core/ui/control_plane/src/components/settings_sidebar.rs`:

1. Add `Channels` variant to `SettingsTab` enum (keep old variants for backward compat, but they'll be unused):

```rust
pub enum SettingsTab {
    // Basic
    General,
    Shortcuts,
    Behavior,
    // AI
    Providers,
    EmbeddingProviders,
    GenerationProviders,
    Memory,
    // Extensions
    Mcp,
    Plugins,
    Skills,
    // Channels — single overview entry
    Channels,
    // Keep old variants for Discord special-case route
    Telegram,
    Discord,
    WhatsApp,
    IMessage,
    // Advanced
    Agent,
    Search,
    Policies,
    RoutingRules,
    Security,
}
```

2. Add path, label, icon for `Channels`:

In `path()`: `Self::Channels => "/settings/channels",`

In `label()`: `Self::Channels => "Channels",`

In `icon_svg()`: `Self::Channels => r#"<path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>"#,`

3. Update `SETTINGS_GROUPS` to use single entry:

```rust
SettingsGroup {
    label: "Channels",
    tabs: &[SettingsTab::Channels],
},
```

**Step 4: Update app.rs routes**

In `core/ui/control_plane/src/app.rs`, replace the 4 individual channel routes:

```rust
<Route path=path!("/settings/channels/telegram") view=TelegramChannelView />
<Route path=path!("/settings/channels/discord") view=DiscordChannelView />
<Route path=path!("/settings/channels/whatsapp") view=WhatsAppChannelView />
<Route path=path!("/settings/channels/imessage") view=IMessageChannelView />
```

With:

```rust
// Channels overview
<Route path=path!("/settings/channels") view=ChannelsOverview />
// Discord keeps its own complex view
<Route path=path!("/settings/channels/discord") view=DiscordChannelView />
// All other channels use the template via static wrapper routes
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
```

Also add the imports at top of app.rs:

```rust
use crate::views::settings::ChannelsOverview;
use crate::views::settings::channels::config_template::ChannelConfigTemplate;
use crate::views::settings::channels::definitions;
```

And add the wrapper components (at bottom of app.rs or in a separate file — whichever fits the style):

```rust
// Thin wrapper components — each passes its static ChannelDefinition to the template
#[component] fn TelegramConfigPage() -> impl IntoView { view! { <ChannelConfigTemplate definition=&definitions::TELEGRAM /> } }
#[component] fn WhatsAppConfigPage() -> impl IntoView { view! { <ChannelConfigTemplate definition=&definitions::WHATSAPP /> } }
#[component] fn IMessageConfigPage() -> impl IntoView { view! { <ChannelConfigTemplate definition=&definitions::IMESSAGE /> } }
#[component] fn SlackConfigPage() -> impl IntoView { view! { <ChannelConfigTemplate definition=&definitions::SLACK /> } }
#[component] fn EmailConfigPage() -> impl IntoView { view! { <ChannelConfigTemplate definition=&definitions::EMAIL /> } }
#[component] fn MatrixConfigPage() -> impl IntoView { view! { <ChannelConfigTemplate definition=&definitions::MATRIX /> } }
#[component] fn SignalConfigPage() -> impl IntoView { view! { <ChannelConfigTemplate definition=&definitions::SIGNAL /> } }
#[component] fn MattermostConfigPage() -> impl IntoView { view! { <ChannelConfigTemplate definition=&definitions::MATTERMOST /> } }
#[component] fn IrcConfigPage() -> impl IntoView { view! { <ChannelConfigTemplate definition=&definitions::IRC /> } }
#[component] fn WebhookConfigPage() -> impl IntoView { view! { <ChannelConfigTemplate definition=&definitions::WEBHOOK /> } }
#[component] fn XmppConfigPage() -> impl IntoView { view! { <ChannelConfigTemplate definition=&definitions::XMPP /> } }
#[component] fn NostrConfigPage() -> impl IntoView { view! { <ChannelConfigTemplate definition=&definitions::NOSTR /> } }
```

**Step 5: Build to verify**

Run: `cd core/ui/control_plane && cargo check`
Expected: Compiles without errors. May have warnings about unused old channel view imports.

**Step 6: Commit**

```bash
git add core/ui/control_plane/src/views/settings/channels/mod.rs \
        core/ui/control_plane/src/views/settings/mod.rs \
        core/ui/control_plane/src/components/settings_sidebar.rs \
        core/ui/control_plane/src/app.rs
git commit -m "panel: wire channels overview, template routes, and sidebar"
```

---

## Task 9: Final Build Verification + Cleanup

**Step 1: Full build check**

Run: `cd core/ui/control_plane && cargo check`
Expected: Clean compile, no errors

**Step 2: Fix any warnings**

Address unused import warnings or dead code warnings if any.

**Step 3: Verify WASM build target**

Run: `cd core/ui/control_plane && cargo check --target wasm32-unknown-unknown`
Expected: Clean compile for WASM target

**Step 4: Final commit (if any fixes)**

```bash
git add -A core/ui/control_plane/
git commit -m "panel: fix warnings and finalize channel config panel"
```

---

## Summary

| Task | Component | Files Created | Files Modified |
|------|-----------|--------------|----------------|
| 1 | SecretInput | `components/ui/secret_input.rs` | `components/ui/mod.rs` |
| 2 | TagListInput | `components/ui/tag_list_input.rs` | `components/ui/mod.rs` |
| 3 | ChannelStatusBadge | `components/ui/channel_status.rs` | `components/ui/mod.rs` |
| 4 | ChannelDefinitions (13) | `views/settings/channels/definitions.rs` | — |
| 5 | ChannelCard | `components/ui/channel_card.rs` | `components/ui/mod.rs` |
| 6 | ChannelConfigTemplate | `views/settings/channels/config_template.rs` | — |
| 7 | ChannelsOverview | `views/settings/channels/overview.rs` | — |
| 8 | Wire everything | — | `channels/mod.rs`, `settings/mod.rs`, `settings_sidebar.rs`, `app.rs` |
| 9 | Build verification | — | (cleanup only) |

**Total new files:** 7
**Total modified files:** 5
**Estimated total lines:** ~1500-1700
